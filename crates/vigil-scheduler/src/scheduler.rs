//! The interval scheduler: a min-heap of due times feeding spawned, rate-limited checks.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::task::{Id, JoinSet};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use vigil_core::WatchId;

use crate::backoff::{next_delay, random_jitter};
use crate::limits::Limits;

/// Shortest wait between two checks of one watch, whatever its interval says.
const MIN_DELAY: Duration = Duration::from_secs(1);

/// What the scheduler needs to know about a watch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// The watch.
    pub id: WatchId,
    /// Key for the per-host limit.
    pub host: String,
    /// Time between checks.
    pub interval: Duration,
    /// Upper bound of the random delay added to every run.
    pub jitter: Duration,
    /// When it was last checked, unix milliseconds; `None` if never.
    pub last_checked_at: Option<i64>,
    /// Failed checks in a row.
    pub consecutive_errors: u32,
}

/// How a check ended, as far as scheduling is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// The check ran; carries the watch's failure streak (0 after a success).
    Done {
        /// Failed checks in a row, including this one.
        consecutive_errors: u32,
    },
    /// The check could not even be recorded; counts as one more failure.
    Failed,
    /// The watch no longer exists; stop scheduling it.
    Gone,
}

/// Where the scheduler gets its watches from and how it runs a check.
pub trait Backend: Send + Sync + 'static {
    /// The watches that should currently be scheduled (paused ones left out).
    fn load(&self) -> impl Future<Output = Result<Vec<Job>, String>> + Send;
    /// Run one check of a watch.
    fn run(&self, id: WatchId) -> impl Future<Output = RunOutcome> + Send;
}

/// Scheduler tuning.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Checks running at once.
    pub concurrency: usize,
    /// Checks running at once against one host.
    pub per_host: usize,
    /// How often the watch list is reloaded to notice additions, removals and pauses.
    pub resync: Duration,
    /// How long shutdown waits for running checks before abandoning them.
    pub shutdown_grace: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            concurrency: 8,
            per_host: 2,
            resync: Duration::from_secs(30),
            shutdown_grace: Duration::from_secs(30),
        }
    }
}

/// Runs checks on their schedules until cancelled.
pub struct Scheduler<B: Backend> {
    backend: Arc<B>,
    limits: Arc<Limits>,
    settings: Settings,
}

impl<B: Backend> Scheduler<B> {
    /// Build a scheduler around `backend`.
    pub fn new(backend: B, settings: Settings) -> Self {
        let limits = Limits::new(settings.concurrency, settings.per_host);
        Self {
            backend: Arc::new(backend),
            limits: Arc::new(limits),
            settings,
        }
    }

    /// Schedule and run checks until `cancel` fires, then wait for running checks to finish
    /// (up to [`Settings::shutdown_grace`]) and return.
    pub async fn run(&self, cancel: CancellationToken) {
        let mut state = State::default();
        let mut tasks: JoinSet<(WatchId, RunOutcome)> = JoinSet::new();
        let mut task_watch: HashMap<Id, WatchId> = HashMap::new();
        let mut resync = tokio::time::interval(self.settings.resync);
        resync.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            let next_due = state.next_due();
            tokio::select! {
                () = cancel.cancelled() => break,
                _ = resync.tick() => self.reload(&mut state).await,
                () = sleep_until_opt(next_due) => {
                    for (id, host) in state.take_due(Instant::now()) {
                        let backend = Arc::clone(&self.backend);
                        let limits = Arc::clone(&self.limits);
                        let handle = tasks.spawn(async move {
                            let _permit = limits.acquire(&host).await;
                            (id, backend.run(id).await)
                        });
                        task_watch.insert(handle.id(), id);
                    }
                }
                Some(joined) = tasks.join_next_with_id(), if !tasks.is_empty() => {
                    match joined {
                        Ok((task, (id, outcome))) => {
                            task_watch.remove(&task);
                            state.finish(id, outcome, Instant::now());
                        }
                        Err(error) => {
                            if let Some(id) = task_watch.remove(&error.id()) {
                                tracing::error!(watch = %id, %error, "check task failed");
                                state.finish(id, RunOutcome::Failed, Instant::now());
                            }
                        }
                    }
                }
            }
        }

        self.drain(tasks).await;
    }

    async fn reload(&self, state: &mut State) {
        match self.backend.load().await {
            Ok(jobs) => state.sync(jobs, Instant::now(), now_ms()),
            Err(error) => tracing::warn!(%error, "reloading watches failed"),
        }
    }

    async fn drain(&self, mut tasks: JoinSet<(WatchId, RunOutcome)>) {
        if tasks.is_empty() {
            return;
        }
        tracing::info!(in_flight = tasks.len(), "waiting for running checks");
        let wait = async { while tasks.join_next().await.is_some() {} };
        let timed_out = tokio::time::timeout(self.settings.shutdown_grace, wait)
            .await
            .is_err();
        if timed_out {
            tracing::warn!(
                abandoned = tasks.len(),
                "shutdown grace period over; aborting checks"
            );
            tasks.abort_all();
        }
    }
}

async fn sleep_until_opt(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

struct Entry {
    job: Job,
    due: Instant,
    running: bool,
}

/// Schedule bookkeeping. Heap items whose entry has since moved on are skipped when popped.
#[derive(Default)]
struct State {
    entries: HashMap<WatchId, Entry>,
    heap: BinaryHeap<Reverse<(Instant, WatchId)>>,
}

impl State {
    fn next_due(&self) -> Option<Instant> {
        self.heap.peek().map(|Reverse((due, _))| *due)
    }

    /// Bring the schedule in line with the current watch list.
    fn sync(&mut self, jobs: Vec<Job>, now: Instant, now_ms: i64) {
        let present: HashSet<WatchId> = jobs.iter().map(|job| job.id).collect();
        self.entries.retain(|id, _| present.contains(id));
        for job in jobs {
            match self.entries.get_mut(&job.id) {
                Some(entry) => {
                    // A running check will report its own streak when it finishes.
                    let errors = if entry.running {
                        entry.job.consecutive_errors
                    } else {
                        job.consecutive_errors
                    };
                    entry.job = Job {
                        consecutive_errors: errors,
                        ..job
                    };
                }
                None => {
                    let due = initial_due(&job, now, now_ms);
                    self.heap.push(Reverse((due, job.id)));
                    self.entries.insert(
                        job.id,
                        Entry {
                            job,
                            due,
                            running: false,
                        },
                    );
                }
            }
        }
    }

    /// Mark every watch that is due as running and return it with its host key.
    fn take_due(&mut self, now: Instant) -> Vec<(WatchId, String)> {
        let mut due = Vec::new();
        while let Some(Reverse((at, id))) = self.heap.peek().copied() {
            if at > now {
                break;
            }
            self.heap.pop();
            if let Some(entry) = self.entries.get_mut(&id)
                && !entry.running
                && entry.due == at
            {
                entry.running = true;
                due.push((id, entry.job.host.clone()));
            }
        }
        due
    }

    /// Reschedule a watch whose check just ended.
    fn finish(&mut self, id: WatchId, outcome: RunOutcome, now: Instant) {
        let Some(entry) = self.entries.get_mut(&id) else {
            return;
        };
        entry.running = false;
        match outcome {
            RunOutcome::Gone => {
                self.entries.remove(&id);
                return;
            }
            RunOutcome::Done { consecutive_errors } => {
                entry.job.consecutive_errors = consecutive_errors
            }
            RunOutcome::Failed => {
                entry.job.consecutive_errors = entry.job.consecutive_errors.saturating_add(1);
            }
        }
        let delay = next_delay(entry.job.interval, entry.job.consecutive_errors).max(MIN_DELAY)
            + random_jitter(entry.job.jitter);
        entry.due = now + delay;
        self.heap.push(Reverse((entry.due, id)));
    }
}

/// When a watch seen for the first time is due: when its last check plus interval says, or
/// now if that has passed or it was never checked, spread out by jitter.
fn initial_due(job: &Job, now: Instant, now_ms: i64) -> Instant {
    let wait = job.last_checked_at.map_or(Duration::ZERO, |last| {
        let delay = next_delay(job.interval, job.consecutive_errors);
        let delay_ms = i64::try_from(delay.as_millis()).unwrap_or(i64::MAX);
        let remaining = last.saturating_add(delay_ms).saturating_sub(now_ms);
        Duration::from_millis(u64::try_from(remaining).unwrap_or(0))
    });
    now + wait + random_jitter(job.jitter)
}
