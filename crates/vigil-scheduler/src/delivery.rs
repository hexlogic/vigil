//! One delivery of a notification to one target.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use vigil_core::WatchId;
use vigil_notify::{Channel, DeliveryReport, Event, RetryPolicy, Sender, send_event};
use vigil_store::{DeliveryOutcome, DeliveryRecord, NotificationTarget, SnapshotId, Store, now_ms};

use crate::describe::describe;
use crate::notify::TargetReport;

/// A delivery's row in the outbox.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Outbox {
    pub(crate) id: i64,
    /// When the notification was first saved, in unix milliseconds.
    pub(crate) created_at: i64,
    /// How many tries have run out of attempts before this one.
    pub(crate) rounds: u32,
}

/// How a delivery ended.
pub(crate) struct Finished {
    pub(crate) report: DeliveryReport,
    /// Its tries ran out on something that may pass, and it will be tried again later.
    pub(crate) queued: bool,
}

/// How long a saved notification is still worth sending: a day.
pub(crate) const MAX_PENDING_AGE_MS: i64 = 24 * 60 * 60 * 1000;

/// Everything one delivery task needs, owned so that it can outlive the check.
pub(crate) struct Delivery {
    pub(crate) store: Store,
    pub(crate) sender: Sender,
    pub(crate) retry: RetryPolicy,
    /// How long to wait before each later try of a notification whose tries ran out: the first
    /// entry after the first round, and so on; the last one is repeated.
    pub(crate) later: Vec<Duration>,
    pub(crate) cancel: CancellationToken,
    pub(crate) reports: Option<Arc<Mutex<Vec<TargetReport>>>>,
    pub(crate) watch: Option<WatchId>,
    pub(crate) snapshot: Option<SnapshotId>,
    /// The watch's own template, which wins over the target's.
    pub(crate) template: Option<String>,
    pub(crate) event: Arc<Event>,
    /// The row in the outbox, if the notification was saved.
    pub(crate) outbox: Option<Outbox>,
}

impl Delivery {
    /// Deliver to `target`, record the result and say how it ended.
    pub(crate) async fn run(&self, target: &NotificationTarget) -> Finished {
        let template = self
            .template
            .as_deref()
            .or(target.default_template.as_deref());
        let report = match Channel::parse(&target.url) {
            Ok(channel) => {
                send_event(
                    &self.sender,
                    &channel,
                    &self.event,
                    template,
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
                interrupted: false,
                retryable: false,
            },
        };
        if report.interrupted {
            // Neither sent nor failed: it stays in the outbox for the next start.
            tracing::info!(target = %target.name, "notification interrupted by shutdown; it will be sent at the next start");
            return Finished {
                report,
                queued: false,
            };
        }

        if let (Some(outbox), Some(wait)) = (self.outbox, self.wait_before_next_try(&report)) {
            // Its tries ran out on something that may pass: keep it for a later try.
            let mut shown = report;
            if let Err(error) = &shown.result {
                shown.result = Err(format!("{error}; trying again in {}", describe_wait(wait)));
            }
            self.record(target, &shown, true).await;
            let next = now_ms().saturating_add(i64::try_from(wait.as_millis()).unwrap_or(i64::MAX));
            if let Err(error) = self
                .store
                .defer_notification(outbox.id, next, outbox.rounds + 1)
                .await
            {
                tracing::error!(error = %describe(&error), "could not schedule another try");
            }
            return Finished {
                report: shown,
                queued: true,
            };
        }

        self.record(target, &report, false).await;
        if let Some(outbox) = self.outbox
            && let Err(error) = self.store.complete_notification(outbox.id).await
        {
            tracing::error!(error = %describe(&error), "could not remove a saved notification");
        }
        Finished {
            report,
            queued: false,
        }
    }

    /// How long to wait before the next try, if this failure deserves one: it may pass, the
    /// notification is saved, and the try would still come within a day of the change.
    fn wait_before_next_try(&self, report: &DeliveryReport) -> Option<Duration> {
        let outbox = self.outbox?;
        if !report.retryable || report.result.is_ok() {
            return None;
        }
        let index = usize::try_from(outbox.rounds)
            .unwrap_or(usize::MAX)
            .min(self.later.len().checked_sub(1)?);
        let wait = *self.later.get(index)?;
        let due = now_ms().saturating_add(i64::try_from(wait.as_millis()).unwrap_or(i64::MAX));
        (due.saturating_sub(outbox.created_at) <= MAX_PENDING_AGE_MS).then_some(wait)
    }

    pub(crate) async fn record(
        &self,
        target: &NotificationTarget,
        report: &DeliveryReport,
        queued: bool,
    ) {
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
                queued,
            });
        }
    }
}

/// "30 s", "5 min", "2 h".
fn describe_wait(wait: Duration) -> String {
    let secs = wait.as_secs();
    if secs >= 3600 && secs.is_multiple_of(3600) {
        format!("{} h", secs / 3600)
    } else if secs >= 60 {
        format!("{} min", secs.div_ceil(60))
    } else {
        format!("{secs} s")
    }
}
