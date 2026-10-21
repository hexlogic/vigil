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
use vigil_store::{NotificationTarget, SnapshotId, Store, now_ms};

use crate::delivery::{Delivery, Outbox};
use crate::describe::describe;

mod outbox;

/// How long to wait before each later try of a notification whose quick retries ran out; the
/// last wait is repeated until the notification is a day old.
const LATER_TRIES: [Duration; 6] = [
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
    Duration::from_secs(30 * 60),
    Duration::from_secs(60 * 60),
    Duration::from_secs(2 * 60 * 60),
    Duration::from_secs(4 * 60 * 60),
];

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
    /// The tries ran out on something that may pass, and the notification is kept to be tried
    /// again later by a running `vigil serve`.
    pub queued: bool,
}

/// Delivers the notifications a change calls for.
#[derive(Debug)]
pub struct Notifier {
    store: Store,
    sender: Sender,
    retry: RetryPolicy,
    later: Vec<Duration>,
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
            later: LATER_TRIES.to_vec(),
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

    /// How long to wait before each later try of a notification whose quick retries ran out
    /// (see [`retry_due`](Self::retry_due)). The last wait is repeated. Empty means no later
    /// tries.
    #[must_use]
    pub fn with_later_tries(mut self, later: Vec<Duration>) -> Self {
        self.later = later;
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
            delivery.outbox = outbox.map(|id| Outbox {
                id,
                created_at: now_ms(),
                rounds: 0,
            });
            self.tracker.spawn(async move {
                delivery.run(&target).await;
            });
        }
    }

    /// Send a test notification to `target` and wait for the result, which is also recorded in
    /// the delivery log.
    pub async fn test(&self, target: &NotificationTarget) -> TargetReport {
        let event = Arc::new(Event::test(self.base_url.as_deref()));
        let delivery = self.delivery(None, None, None, event);
        let finished = delivery.run(target).await;
        TargetReport {
            target: target.name.clone(),
            report: finished.report,
            queued: finished.queued,
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
    /// to finish. A delivery that is still waiting to retry stays in the outbox, to be sent by
    /// [`resume`](Self::resume) at the next start.
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
            later: self.later.clone(),
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
