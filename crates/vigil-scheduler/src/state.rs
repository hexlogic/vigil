//! Schedule bookkeeping: which watch is due when, and which are running.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::time::Duration;

use tokio::time::Instant;
use vigil_core::WatchId;

use crate::backoff::{MAX_INTERVAL, next_delay, random_jitter};
use crate::scheduler::{Job, RunOutcome};

/// Shortest wait between two checks of one watch, whatever its interval says.
const MIN_DELAY: Duration = Duration::from_secs(1);

struct Entry {
    job: Job,
    due: Instant,
    running: bool,
    /// What the store said about the schedule the last time the watch list was loaded.
    /// Comparing against this, not against `job`, keeps a failed recording from looking like
    /// an edit.
    loaded: Loaded,
}

/// The parts of a watch that decide when it runs next.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Loaded {
    interval: Duration,
    jitter: Duration,
    consecutive_errors: u32,
}

impl Loaded {
    fn of(job: &Job) -> Self {
        Self {
            interval: job.interval,
            jitter: job.jitter,
            consecutive_errors: job.consecutive_errors,
        }
    }
}

/// Schedule bookkeeping. Heap items whose entry has since moved on are skipped when popped.
#[derive(Default)]
pub(crate) struct State {
    entries: HashMap<WatchId, Entry>,
    heap: BinaryHeap<Reverse<(Instant, WatchId)>>,
    /// Watches running because someone asked for a check, though they are not on the schedule
    /// (paused, or not loaded yet).
    on_request: HashSet<WatchId>,
}

impl State {
    pub(crate) fn next_due(&self) -> Option<Instant> {
        self.heap.peek().map(|Reverse((due, _))| *due)
    }

    /// Bring the schedule in line with the current watch list.
    pub(crate) fn sync(&mut self, jobs: Vec<Job>, now: Instant, now_ms: i64) {
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
                    // An edit that changes the interval or clears the failure streak moves the
                    // next check; otherwise a watch changed from daily to every five minutes
                    // would wait out the day, and a fixed watch the whole backoff.
                    let loaded = Loaded::of(&job);
                    let moved = loaded != entry.loaded;
                    entry.loaded = loaded;
                    entry.job = Job {
                        consecutive_errors: errors,
                        ..job
                    };
                    if moved && !entry.running {
                        entry.due = initial_due(&entry.job, now, now_ms);
                        self.heap.push(Reverse((entry.due, entry.job.id)));
                    }
                }
                None => {
                    let due = initial_due(&job, now, now_ms);
                    self.heap.push(Reverse((due, job.id)));
                    // A watch resumed while a requested check of it is still running must not
                    // be started a second time; it is scheduled when that check ends.
                    let running = self.on_request.contains(&job.id);
                    self.entries.insert(
                        job.id,
                        Entry {
                            loaded: Loaded::of(&job),
                            job,
                            due,
                            running,
                        },
                    );
                }
            }
        }
    }

    /// Mark every watch that is due as running and return it with its host key.
    pub(crate) fn take_due(&mut self, now: Instant) -> Vec<(WatchId, String)> {
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

    /// Whether a check of the watch is running, scheduled or requested.
    pub(crate) fn is_running(&self, id: WatchId) -> bool {
        self.on_request.contains(&id) || self.entries.get(&id).is_some_and(|e| e.running)
    }

    /// The host key of a scheduled watch.
    pub(crate) fn host_of(&self, id: WatchId) -> Option<String> {
        self.entries.get(&id).map(|entry| entry.job.host.clone())
    }

    /// Mark a watch as running because a check was asked for now. A scheduled watch keeps its
    /// place and is rescheduled from the end of this check; any other runs once and is
    /// forgotten.
    pub(crate) fn start_now(&mut self, id: WatchId) {
        match self.entries.get_mut(&id) {
            Some(entry) => entry.running = true,
            None => {
                self.on_request.insert(id);
            }
        }
    }

    /// Reschedule a watch whose check just ended.
    pub(crate) fn finish(&mut self, id: WatchId, outcome: RunOutcome, now: Instant) {
        self.on_request.remove(&id);
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
        let delay = next_delay(entry.job.interval, entry.job.consecutive_errors)
            .max(MIN_DELAY)
            .saturating_add(random_jitter(entry.job.jitter));
        entry.due = later(now, delay);
        self.heap.push(Reverse((entry.due, id)));
    }
}

/// When a watch seen for the first time is due: when its last check plus interval says, or
/// now if that has passed or it was never checked, spread out by jitter.
fn initial_due(job: &Job, now: Instant, now_ms: i64) -> Instant {
    let wait = job.last_checked_at.map_or(Duration::ZERO, |last| {
        let delay = next_delay(job.interval, job.consecutive_errors);
        let delay_ms = i64::try_from(delay.as_millis()).unwrap_or(i64::MAX);
        // Never wait longer than one delay: a last check that lies in the future (the clock was
        // set back since) must not park the watch until then.
        let remaining = last
            .saturating_add(delay_ms)
            .saturating_sub(now_ms)
            .min(delay_ms);
        Duration::from_millis(u64::try_from(remaining).unwrap_or(0))
    });
    later(now, wait.saturating_add(random_jitter(job.jitter)))
}

/// `now + delay`, with the delay bounded by [`MAX_INTERVAL`] so it cannot overflow an `Instant`.
fn later(now: Instant, delay: Duration) -> Instant {
    now + delay.min(MAX_INTERVAL)
}
