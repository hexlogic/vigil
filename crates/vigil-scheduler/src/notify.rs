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
use vigil_notify::{Channel, DeliveryReport, Event, RetryPolicy, Sender, deliver, prepare};
use vigil_store::{DeliveryOutcome, DeliveryRecord, NotificationTarget, SnapshotId, Store, now_ms};

use crate::describe::describe;

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
        for target in targets {
            let delivery = self.delivery(
                Some(watch.id),
                Some(change.current),
                watch.notification_template.clone(),
                Arc::clone(&event),
            );
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

/// Everything one delivery task needs, owned so that it can outlive the check.
struct Delivery {
    store: Store,
    sender: Sender,
    retry: RetryPolicy,
    cancel: CancellationToken,
    reports: Option<Arc<Mutex<Vec<TargetReport>>>>,
    watch: Option<WatchId>,
    snapshot: Option<SnapshotId>,
    /// The watch's own template, which wins over the target's.
    template: Option<String>,
    event: Arc<Event>,
}

impl Delivery {
    /// Deliver to `target`, record the result and return it.
    async fn run(&self, target: &NotificationTarget) -> DeliveryReport {
        let template = self
            .template
            .as_deref()
            .or(target.default_template.as_deref());
        let report = match Channel::parse(&target.url) {
            Ok(channel) => {
                let prepared = prepare(channel.markup(), channel.limit(), &self.event, template);
                deliver(
                    &self.sender,
                    &channel,
                    &prepared,
                    &self.event,
                    &self.retry,
                    &self.cancel,
                )
                .await
            }
            // The message names what is wrong and never repeats the URL.
            Err(error) => DeliveryReport {
                attempts: 0,
                result: Err(format!("the stored notification URL is not valid: {error}")),
                note: None,
            },
        };
        self.record(target, &report).await;
        report
    }

    async fn record(&self, target: &NotificationTarget, report: &DeliveryReport) {
        let (outcome, error) = match (&report.result, &report.note) {
            (Ok(()), None) => (DeliveryOutcome::Sent, None),
            (Ok(()), Some(note)) => (DeliveryOutcome::Sent, Some(note.clone())),
            (Err(error), None) => (DeliveryOutcome::Failed, Some(error.clone())),
            (Err(error), Some(note)) => {
                (DeliveryOutcome::Failed, Some(format!("{error} ({note})")))
            }
        };
        let record = DeliveryRecord {
            watch: self.watch,
            target: target.id,
            snapshot: self.snapshot,
            at: now_ms(),
            attempts: report.attempts,
            outcome,
            error,
        };
        if let Err(error) = self.store.record_delivery(&record).await {
            tracing::error!(target = %target.name, error = %describe(&error), "could not record a delivery");
        }
        if report.result.is_ok() {
            tracing::info!(target = %target.name, attempts = report.attempts, "notification sent");
        }
        if let Some(reports) = &self.reports
            && let Ok(mut reports) = reports.lock()
        {
            reports.push(TargetReport {
                target: target.name.clone(),
                report: report.clone(),
            });
        }
    }
}
