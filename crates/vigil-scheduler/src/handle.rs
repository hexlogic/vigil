//! Talking to a running scheduler: asking for a check now, or for a fresh look at the watches.

use tokio::sync::{mpsc, oneshot};
use vigil_core::WatchId;

use crate::scheduler::RunOutcome;

/// The scheduler is not running, or stopped before it could answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the scheduler is not running")]
pub struct Stopped;

/// What a [`SchedulerHandle`] can ask for.
pub(crate) enum Command {
    /// Load the watch list now instead of at the next resync.
    Reload,
    /// Check a watch now.
    CheckNow {
        /// The watch.
        id: WatchId,
        /// Where to report how the check ended.
        reply: oneshot::Sender<RunOutcome>,
    },
}

/// A way to reach a [`Scheduler`](crate::Scheduler) from elsewhere, such as a web handler.
/// Cheap to clone. Requests are answered while the scheduler runs; once it has stopped they
/// fail with [`Stopped`].
#[derive(Debug, Clone)]
pub struct SchedulerHandle {
    pub(crate) commands: mpsc::Sender<Command>,
}

impl SchedulerHandle {
    /// Have the scheduler look at the watch list now, so that a watch that was added, edited,
    /// paused or deleted takes effect at once instead of at the next resync.
    pub async fn reload(&self) -> Result<(), Stopped> {
        self.commands
            .send(Command::Reload)
            .await
            .map_err(|_| Stopped)
    }

    /// Check a watch now, whatever its schedule says, within the same concurrency limits as
    /// scheduled checks. Returns once the request is accepted; [`Ticket::wait`] tells when the
    /// check has ended.
    ///
    /// A watch that is being checked already is not checked again: the ticket reports that
    /// check. A scheduled watch is next due one interval after this check ends. A watch that
    /// is not scheduled (paused) is checked once and stays unscheduled. A watch that does not
    /// exist ends as [`RunOutcome::Gone`].
    pub async fn check_now(&self, id: WatchId) -> Result<Ticket, Stopped> {
        let (reply, outcome) = oneshot::channel();
        self.commands
            .send(Command::CheckNow { id, reply })
            .await
            .map_err(|_| Stopped)?;
        Ok(Ticket { outcome })
    }
}

/// A requested check, to wait for.
#[derive(Debug)]
pub struct Ticket {
    outcome: oneshot::Receiver<RunOutcome>,
}

impl Ticket {
    /// Wait until the check has ended and say how. The result of the check is in the store.
    /// Fails with [`Stopped`] if the scheduler stopped first.
    pub async fn wait(self) -> Result<RunOutcome, Stopped> {
        self.outcome.await.map_err(|_| Stopped)
    }
}
