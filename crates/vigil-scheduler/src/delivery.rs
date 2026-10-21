//! One delivery of a notification to one target.

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;
use vigil_core::WatchId;
use vigil_notify::{Channel, DeliveryReport, Event, RetryPolicy, Sender, send_event};
use vigil_store::{DeliveryOutcome, DeliveryRecord, NotificationTarget, SnapshotId, Store, now_ms};

use crate::describe::describe;
use crate::notify::TargetReport;

/// Everything one delivery task needs, owned so that it can outlive the check.
pub(crate) struct Delivery {
    pub(crate) store: Store,
    pub(crate) sender: Sender,
    pub(crate) retry: RetryPolicy,
    pub(crate) cancel: CancellationToken,
    pub(crate) reports: Option<Arc<Mutex<Vec<TargetReport>>>>,
    pub(crate) watch: Option<WatchId>,
    pub(crate) snapshot: Option<SnapshotId>,
    /// The watch's own template, which wins over the target's.
    pub(crate) template: Option<String>,
    pub(crate) event: Arc<Event>,
    /// The row in the outbox to remove when the delivery is over.
    pub(crate) outbox: Option<i64>,
}

impl Delivery {
    /// Deliver to `target`, record the result and return it.
    pub(crate) async fn run(&self, target: &NotificationTarget) -> DeliveryReport {
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
            },
        };
        if report.interrupted {
            // Neither sent nor failed: it stays in the outbox for the next start.
            tracing::info!(target = %target.name, "notification interrupted by shutdown; it will be sent at the next start");
            return report;
        }
        self.record(target, &report).await;
        if let Some(id) = self.outbox
            && let Err(error) = self.store.complete_notification(id).await
        {
            tracing::error!(error = %describe(&error), "could not remove a saved notification");
        }
        report
    }

    pub(crate) async fn record(&self, target: &NotificationTarget, report: &DeliveryReport) {
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
