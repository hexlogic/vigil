//! The checks in flight, and the callers waiting for them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::task::{Id, JoinSet};
use tokio_util::sync::CancellationToken;
use vigil_core::WatchId;

use crate::limits::Limits;
use crate::scheduler::{Backend, RunOutcome};

/// Running checks. Each is a task that first waits for its host and global slots.
#[derive(Default)]
pub(crate) struct Runs {
    tasks: JoinSet<(WatchId, Option<RunOutcome>)>,
    task_watch: HashMap<Id, WatchId>,
    waiters: HashMap<WatchId, Vec<oneshot::Sender<RunOutcome>>>,
}

impl Runs {
    /// Whether no check is running.
    pub(crate) fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Start a check of `id` once a slot is free for `host`.
    pub(crate) fn spawn<B: Backend>(
        &mut self,
        backend: &Arc<B>,
        limits: &Arc<Limits>,
        cancel: &CancellationToken,
        id: WatchId,
        host: String,
    ) {
        let backend = Arc::clone(backend);
        let limits = Arc::clone(limits);
        let cancel = cancel.clone();
        let handle = self.tasks.spawn(async move {
            // A check still waiting for a slot when shutdown starts never runs.
            let permit = tokio::select! {
                () = cancel.cancelled() => None,
                permit = limits.acquire(&host) => Some(permit),
            };
            let Some(_permit) = permit else {
                return (id, None);
            };
            // Both branches can be ready at once; honour the cancellation.
            if cancel.is_cancelled() {
                return (id, None);
            }
            (id, Some(backend.run(id).await))
        });
        self.task_watch.insert(handle.id(), id);
    }

    /// Tell `reply` how the running check of `id` ends.
    pub(crate) fn wait_for(&mut self, id: WatchId, reply: oneshot::Sender<RunOutcome>) {
        self.waiters.entry(id).or_default().push(reply);
    }

    /// Wait for one check to end and report it, after telling those waiting for it. The
    /// outcome is `None` for a check that was dropped at shutdown before it started. Returns
    /// `None` when there is nothing to report; cancel safe.
    pub(crate) async fn next(&mut self) -> Option<(WatchId, Option<RunOutcome>)> {
        let (id, outcome) = match self.tasks.join_next_with_id().await? {
            Ok((task, (id, outcome))) => {
                self.task_watch.remove(&task);
                (id, outcome)
            }
            Err(error) => {
                let id = self.task_watch.remove(&error.id())?;
                tracing::error!(watch = %id, %error, "check task failed");
                (id, Some(RunOutcome::Failed))
            }
        };
        let waiting = self.waiters.remove(&id).unwrap_or_default();
        if let Some(outcome) = outcome {
            for reply in waiting {
                // A caller that stopped waiting has dropped its end.
                let _ = reply.send(outcome);
            }
        }
        Some((id, outcome))
    }

    /// Wait for running checks to end, for at most `grace`, then abort what is left. Callers
    /// still waiting find out that the scheduler stopped.
    pub(crate) async fn drain(mut self, grace: Duration) {
        self.waiters.clear();
        if self.tasks.is_empty() {
            return;
        }
        tracing::info!(in_flight = self.tasks.len(), "waiting for running checks");
        let wait = async { while self.tasks.join_next().await.is_some() {} };
        let timed_out = tokio::time::timeout(grace, wait).await.is_err();
        if timed_out {
            tracing::warn!(
                abandoned = self.tasks.len(),
                "shutdown grace period over; aborting checks"
            );
            self.tasks.abort_all();
        }
    }
}
