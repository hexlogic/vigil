//! Sending notifications after a change, without making the check wait for them.
//!
//! A check hands its change to the [`Notifier`] and moves on. Each target is delivered on a
//! background task, so retries (which can take a minute) never hold the scheduler's slots for
//! the watched host, never change the check's result and never count as a failed check.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use vigil_core::{LineDiff, Verdict, Watch, WatchId};
use vigil_notify::{DeliveryReport, Event, RetryPolicy, Sender};
use vigil_store::{
    NewPending, NotificationTarget, PendingNotification, SnapshotId, Store, StoreError, now_ms,
};

use crate::delivery::Delivery;
use crate::describe::describe;

/// How long a saved notification is still worth sending after a restart: a day.
const MAX_PENDING_AGE_MS: i64 = 24 * 60 * 60 * 1000;

/// A change that was just recorded, as the notifier needs to know it.
pub(crate) struct Change<'a> {
    /// The snapshot that was current before.
    pub previous: SnapshotId,
    /// The new snapshot.
    pub current: SnapshotId,
    /// When the check ran, in unix milliseconds.
    pub at: i64,
    /// The new text.
    pub text: &'a str,
    /// What changed.
    pub diff: &'a LineDiff,
    /// What the watch's conditions made of it.
    pub verdict: &'a Verdict,
}

/// How a delivery to one target ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetReport {
    /// The target's name.
    pub target: String,
    /// How it went.
    pub report: DeliveryReport,
}

/// Delivers the notifications a change calls for.
#[derive(Debug)]
pub struct Notifier {
    store: Store,
    sender: Sender,
    retry: RetryPolicy,
    base_url: Option<String>,
    tracker: TaskTracker,
    cancel: CancellationToken,
    reports: Option<Arc<Mutex<Vec<TargetReport>>>>,
}

impl Notifier {
    /// A notifier that records its deliveries in `store` and sends with `sender`.
    pub fn new(store: Store, sender: Sender) -> Self {
        Self {
            store,
            sender,
            retry: RetryPolicy::default(),
            base_url: None,
            tracker: TaskTracker::new(),
            cancel: CancellationToken::new(),
            reports: None,
        }
    }

    /// The address Vigil is served at, used for links to diff pages.
    #[must_use]
    pub fn with_base_url(mut self, base_url: Option<String>) -> Self {
        self.base_url = base_url
            .map(|url| url.trim().trim_end_matches('/').to_owned())
            .filter(|url| !url.is_empty());
        self
    }

    /// How failed deliveries are retried.
    #[must_use]
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Keep each delivery's report for [`close_and_wait`](Self::close_and_wait). The server
    /// leaves this off: it only needs the delivery log.
    #[must_use]
    pub fn collecting_reports(mut self) -> Self {
        self.reports = Some(Arc::default());
        self
    }

    /// Start delivering `change` to the watch's enabled targets and return at once.
    pub(crate) async fn dispatch(&self, watch: &Watch, change: Change<'_>) {
        let targets = match self.store.targets_for_watch(watch.id, true).await {
            Ok(targets) if targets.is_empty() => return,
            Ok(targets) => targets,
            Err(error) => {
                tracing::error!(
                    watch = %watch.id,
                    error = %describe(&error),
                    "could not load the notification targets"
                );
                return;
            }
        };
        let event = Arc::new(self.event(watch, &change));
        let payload = match event.to_json() {
            Ok(payload) => Some(payload),
            Err(error) => {
                tracing::error!(watch = %watch.id, %error, "could not save the notification");
                None
            }
        };
        for target in targets {
            // Saved before it is sent, so that a restart in the middle does not lose it.
            let outbox = match &payload {
                Some(payload) => {
                    self.save(watch.id, &target, change.current, payload.clone())
                        .await
                }
                None => None,
            };
            let mut delivery = self.delivery(
                Some(watch.id),
                Some(change.current),
                watch.notification_template.clone(),
                Arc::clone(&event),
            );
            delivery.outbox = outbox;
            self.tracker.spawn(async move {
                delivery.run(&target).await;
            });
        }
    }

    async fn save(
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
    /// from the process that serves; returns how many were started.
    ///
    /// One whose target has since been removed or disabled is dropped, and one older than a day
    /// is recorded as failed instead of being sent: a notice that late is noise.
    pub async fn resume(&self) -> usize {
        let pending = match self.store.pending_notifications().await {
            Ok(pending) => pending,
            Err(error) => {
                tracing::error!(error = %describe(&error), "could not read the saved notifications");
                return 0;
            }
        };
        let mut started = 0;
        for item in pending {
            if self.resume_one(&item).await {
                started += 1;
            }
        }
        started
    }

    async fn resume_one(&self, item: &PendingNotification) -> bool {
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
        let delivery = self.delivery(Some(item.watch), Some(item.snapshot), None, Arc::default());
        if now_ms().saturating_sub(item.created_at) > MAX_PENDING_AGE_MS {
            let report = DeliveryReport {
                attempts: 0,
                result: Err(
                    "not sent: the notification was too old by the time Vigil started again".into(),
                ),
                note: None,
                interrupted: false,
            };
            delivery.record(&target, &report).await;
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
        delivery.outbox = Some(item.id);
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

    /// Send a test notification to `target` and wait for the result, which is also recorded in
    /// the delivery log.
    pub async fn test(&self, target: &NotificationTarget) -> TargetReport {
        let event = Arc::new(Event::test(self.base_url.as_deref()));
        let delivery = self.delivery(None, None, None, event);
        let report = delivery.run(target).await;
        TargetReport {
            target: target.name.clone(),
            report,
        }
    }

    /// Stop taking new work and wait for every delivery in progress, however long the retries
    /// take. Returns the reports if [`collecting_reports`](Self::collecting_reports) was set.
    pub async fn close_and_wait(&self) -> Vec<TargetReport> {
        self.tracker.close();
        self.tracker.wait().await;
        self.reports
            .as_ref()
            .and_then(|reports| reports.lock().ok().map(|mut r| std::mem::take(&mut *r)))
            .unwrap_or_default()
    }

    /// Shut down: stop waiting between retries, and give attempts already on the wire `grace`
    /// to finish. A delivery that is still waiting to retry gives up and says so in the log.
    pub async fn shutdown(&self, grace: Duration) {
        self.tracker.close();
        self.cancel.cancel();
        if tokio::time::timeout(grace, self.tracker.wait())
            .await
            .is_err()
        {
            tracing::warn!("abandoned notification deliveries still in progress at shutdown");
        }
    }

    fn delivery(
        &self,
        watch: Option<WatchId>,
        snapshot: Option<SnapshotId>,
        template: Option<String>,
        event: Arc<Event>,
    ) -> Delivery {
        Delivery {
            store: self.store.clone(),
            sender: self.sender.clone(),
            retry: self.retry.clone(),
            cancel: self.cancel.clone(),
            reports: self.reports.clone(),
            watch,
            snapshot,
            template,
            event,
            outbox: None,
        }
    }

    fn event(&self, watch: &Watch, change: &Change<'_>) -> Event {
        // Phase 4 serves the diff page at this path.
        let diff_url = self.base_url.as_ref().map_or_else(String::new, |base| {
            format!(
                "{base}/watch/{}/diff?from={}&to={}",
                watch.id, change.previous, change.current
            )
        });
        Event {
            watch_id: watch.id.to_string(),
            watch_name: watch.name.clone(),
            watch_url: watch.url.clone(),
            added: change.diff.added().map(str::to_owned).collect(),
            removed: change.diff.removed().map(str::to_owned).collect(),
            diff: change.diff.unified(3),
            snapshot: change.text.to_owned(),
            triggered: change.verdict.triggered_text.clone(),
            checked_at_ms: change.at,
            base_url: self.base_url.clone().unwrap_or_default(),
            diff_url,
            is_test: false,
        }
    }
}
