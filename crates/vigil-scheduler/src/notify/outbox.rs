//! Notifications kept in the outbox: saving them, and sending them again after a restart or
//! after their quick retries ran out.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use vigil_core::WatchId;
use vigil_notify::{DeliveryReport, Event};
use vigil_store::{
    NewPending, NotificationTarget, PendingNotification, SnapshotId, StoreError, now_ms,
};

use super::Notifier;
use crate::delivery::{MAX_PENDING_AGE_MS, Outbox};
use crate::describe::describe;

impl Notifier {
    /// Save a notification before it is delivered. `None` if that failed.
    pub(super) async fn save(
        &self,
        watch: WatchId,
        target: &NotificationTarget,
        snapshot: SnapshotId,
        payload: String,
    ) -> Option<i64> {
        let pending = NewPending {
            watch,
            target: target.id,
            snapshot,
            payload,
        };
        match self.store.enqueue_notification(&pending).await {
            Ok(id) => Some(id),
            // Deliver anyway: a notification that cannot survive a restart beats none.
            Err(error) => {
                tracing::error!(%watch, error = %describe(&error), "could not save the notification");
                None
            }
        }
    }

    /// Deliver the notifications that were saved but not finished when the process last stopped:
    /// the ones a shutdown cut short, or that a crash left behind. Call it once at start-up,
    /// from the process that serves; returns how many were started. Those waiting for a later
    /// try are left to [`retry_due`](Self::retry_due).
    pub async fn resume(&self) -> usize {
        let pending = match self.store.pending_notifications().await {
            Ok(pending) => pending,
            Err(error) => {
                tracing::error!(error = %describe(&error), "could not read the saved notifications");
                return 0;
            }
        };
        let mut started = 0;
        for item in pending.iter().filter(|p| p.next_attempt_at.is_none()) {
            if self.start_saved(item).await {
                started += 1;
            }
        }
        started
    }

    /// Try again the saved notifications whose wait is over: those whose quick retries ran out
    /// on a failure that may pass. Each gets a single attempt per round, and is given up on
    /// when it is a day old. Returns how many were started.
    ///
    /// Several callers may run this at once; each notification is taken by one of them.
    pub async fn retry_due(&self) -> usize {
        let now = now_ms();
        let due = match self.store.due_notifications(now).await {
            Ok(due) => due,
            Err(error) => {
                tracing::error!(error = %describe(&error), "could not read the saved notifications");
                return 0;
            }
        };
        let mut started = 0;
        for item in &due {
            match self.store.claim_notification(item.id, now).await {
                Ok(true) => {
                    if self.start_saved(item).await {
                        started += 1;
                    }
                }
                Ok(false) => {}
                Err(error) => {
                    tracing::error!(error = %describe(&error), "could not take a saved notification");
                }
            }
        }
        started
    }

    /// Call [`retry_due`](Self::retry_due) every `every` until `stop` is cancelled.
    pub async fn run_retries(&self, every: Duration, stop: CancellationToken) {
        let mut ticker = tokio::time::interval(every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = stop.cancelled() => return,
                _ = ticker.tick() => {
                    let started = self.retry_due().await;
                    if started > 0 {
                        tracing::info!(started, "trying saved notifications again");
                    }
                }
            }
        }
    }

    /// Start delivering a saved notification. False if it was dropped instead: its target or
    /// watch is gone or disabled, it is a day old (recorded as failed), or it cannot be read.
    async fn start_saved(&self, item: &PendingNotification) -> bool {
        let target = match self.store.get_target(item.target).await {
            Ok(target) if target.enabled => target,
            Ok(_) | Err(StoreError::NotFound(_)) => {
                self.forget(item.id).await;
                return false;
            }
            Err(error) => {
                tracing::error!(error = %describe(&error), "could not load a notification target");
                return false;
            }
        };
        if now_ms().saturating_sub(item.created_at) > MAX_PENDING_AGE_MS {
            let report = DeliveryReport {
                attempts: 0,
                result: Err("not sent: the notification was too old to be worth sending".into()),
                note: None,
                interrupted: false,
                retryable: false,
            };
            self.delivery(Some(item.watch), Some(item.snapshot), None, Arc::default())
                .record(&target, &report, false)
                .await;
            self.forget(item.id).await;
            return false;
        }
        let mut event = match Event::from_json(&item.payload) {
            Ok(event) => event,
            Err(error) => {
                tracing::warn!(id = item.id, %error, "dropping an unreadable saved notification");
                self.forget(item.id).await;
                return false;
            }
        };
        event.snapshot = self
            .store
            .snapshot_content(item.snapshot)
            .await
            .unwrap_or_default();
        let template = match self.store.get_watch(item.watch).await {
            Ok(watch) => watch.notification_template,
            Err(StoreError::NotFound(_)) => {
                self.forget(item.id).await;
                return false;
            }
            Err(error) => {
                tracing::error!(error = %describe(&error), "could not load a watch");
                return false;
            }
        };
        let mut delivery = self.delivery(
            Some(item.watch),
            Some(item.snapshot),
            template,
            Arc::new(event),
        );
        delivery.outbox = Some(Outbox {
            id: item.id,
            created_at: item.created_at,
            rounds: item.rounds,
        });
        if item.rounds > 0 {
            // The wait between rounds is the retry; one attempt per round.
            delivery.retry.delays.clear();
        }
        self.tracker.spawn(async move {
            delivery.run(&target).await;
        });
        true
    }

    async fn forget(&self, id: i64) {
        if let Err(error) = self.store.complete_notification(id).await {
            tracing::error!(error = %describe(&error), "could not remove a saved notification");
        }
    }
}
