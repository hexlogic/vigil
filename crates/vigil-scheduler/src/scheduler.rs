//! The interval scheduler: a min-heap of due times feeding spawned, rate-limited checks.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, mpsc};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use vigil_core::WatchId;
use vigil_store::now_ms;

use crate::handle::{Command, SchedulerHandle};
use crate::limits::Limits;
use crate::runs::Runs;
use crate::state::State;

/// Requests that can wait for the scheduler's loop to get to them.
const COMMAND_QUEUE: usize = 256;

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
    /// One watch, whether or not it should be scheduled; `None` if it does not exist. Used to
    /// check a paused watch on request.
    fn find(&self, id: WatchId) -> impl Future<Output = Result<Option<Job>, String>> + Send;
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
    /// How often the watch list is reloaded to notice additions, removals and pauses. A
    /// [`SchedulerHandle`] can ask for a reload sooner.
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
    commands: mpsc::Sender<Command>,
    inbox: Mutex<mpsc::Receiver<Command>>,
}

impl<B: Backend> Scheduler<B> {
    /// Build a scheduler around `backend`.
    pub fn new(backend: B, settings: Settings) -> Self {
        let limits = Limits::new(settings.concurrency, settings.per_host);
        let (commands, inbox) = mpsc::channel(COMMAND_QUEUE);
        Self {
            backend: Arc::new(backend),
            limits: Arc::new(limits),
            settings,
            commands,
            inbox: Mutex::new(inbox),
        }
    }

    /// A way to reach this scheduler while it runs, from another task.
    pub fn handle(&self) -> SchedulerHandle {
        SchedulerHandle {
            commands: self.commands.clone(),
        }
    }

    /// Schedule and run checks until `cancel` fires, then wait for running checks to finish
    /// (up to [`Settings::shutdown_grace`]) and return.
    ///
    /// A scheduler runs once: when it stops, requests that were waiting are dropped and later
    /// ones fail with [`Stopped`](crate::Stopped).
    pub async fn run(&self, cancel: CancellationToken) {
        let mut inbox = self.inbox.lock().await;
        let mut state = State::default();
        let mut runs = Runs::default();
        let mut resync = tokio::time::interval(self.settings.resync);
        resync.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            let next_due = state.next_due();
            tokio::select! {
                () = cancel.cancelled() => break,
                _ = resync.tick() => self.reload(&mut state).await,
                // This scheduler holds a sender itself, so the queue never reports "closed".
                Some(command) = inbox.recv() => {
                    self.command(command, &mut state, &mut runs, &cancel).await;
                }
                () = sleep_until_opt(next_due) => {
                    for (id, host) in state.take_due(Instant::now()) {
                        runs.spawn(&self.backend, &self.limits, &cancel, id, host);
                    }
                }
                Some((id, outcome)) = runs.next(), if !runs.is_empty() => {
                    // `None`: dropped at shutdown before it started, nothing to record.
                    if let Some(outcome) = outcome {
                        state.finish(id, outcome, Instant::now());
                    }
                }
            }
        }

        inbox.close();
        // Requests still queued are dropped, which tells their senders the scheduler stopped.
        while inbox.try_recv().is_ok() {}
        runs.drain(self.settings.shutdown_grace).await;
    }

    async fn reload(&self, state: &mut State) {
        match self.backend.load().await {
            Ok(jobs) => state.sync(jobs, Instant::now(), now_ms()),
            Err(error) => tracing::warn!(%error, "reloading watches failed"),
        }
    }

    async fn command(
        &self,
        command: Command,
        state: &mut State,
        runs: &mut Runs,
        cancel: &CancellationToken,
    ) {
        match command {
            Command::Reload => self.reload(state).await,
            Command::CheckNow { id, reply } => {
                if !state.is_running(id) {
                    let host = match state.host_of(id) {
                        Some(host) => host,
                        None => match self.backend.find(id).await {
                            Ok(Some(job)) => job.host,
                            Ok(None) => {
                                // The caller may have given up waiting.
                                let _ = reply.send(RunOutcome::Gone);
                                return;
                            }
                            Err(error) => {
                                tracing::warn!(watch = %id, %error, "looking up the watch failed");
                                let _ = reply.send(RunOutcome::Failed);
                                return;
                            }
                        },
                    };
                    state.start_now(id);
                    runs.spawn(&self.backend, &self.limits, cancel, id, host);
                }
                runs.wait_for(id, reply);
            }
        }
    }
}

async fn sleep_until_opt(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}
