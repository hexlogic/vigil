//! Delivery with retries.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::channel::Channel;
use crate::event::Event;
use crate::message::{Prepared, prepare};
use crate::send::Sender;

/// How a failed delivery is retried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    /// How long to wait before each retry; its length is the number of retries.
    pub delays: Vec<Duration>,
    /// The longest a server's `Retry-After` is honoured.
    pub max_retry_after: Duration,
}

impl Default for RetryPolicy {
    /// Three retries, after 2 s, 10 s and 30 s.
    fn default() -> Self {
        Self {
            delays: vec![
                Duration::from_secs(2),
                Duration::from_secs(10),
                Duration::from_secs(30),
            ],
            max_retry_after: Duration::from_secs(60),
        }
    }
}

/// How a delivery ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryReport {
    /// How many attempts were made.
    pub attempts: u32,
    /// `Ok` once the channel accepted the message; otherwise why the last attempt failed. The
    /// text holds no credentials.
    pub result: Result<(), String>,
    /// Set when a custom template failed and the built-in one was sent instead.
    pub note: Option<String>,
    /// Set when the delivery was stopped by its cancellation token before it was over. It is
    /// neither sent nor failed for good, and can be started again.
    pub interrupted: bool,
    /// Set when the delivery failed on something that may pass (a network problem, a busy or
    /// failing server) and ran out of attempts, so a later try may work. A refusal is not.
    pub retryable: bool,
}

/// Deliver `prepared` to `channel`, retrying transient failures as `policy` says.
///
/// A refused request (a bad token, a missing topic) is not retried. Cancelling `cancel` stops
/// the waiting between attempts, so shutdown does not have to sit out a long backoff.
pub async fn deliver(
    sender: &Sender,
    channel: &Channel,
    prepared: &Prepared,
    event: &Event,
    policy: &RetryPolicy,
    cancel: &CancellationToken,
) -> DeliveryReport {
    let mut attempts = 0;
    loop {
        attempts += 1;
        let error = match sender.send(channel, prepared, event).await {
            Ok(()) => {
                return DeliveryReport {
                    attempts,
                    result: Ok(()),
                    note: prepared.template_note.clone(),
                    interrupted: false,
                    retryable: false,
                };
            }
            Err(error) => error,
        };
        let retry = usize::try_from(attempts - 1)
            .ok()
            .and_then(|i| policy.delays.get(i));
        let Some(delay) = retry.filter(|_| error.transient) else {
            tracing::warn!(channel = channel.kind(), attempts, error = %error, "notification failed");
            return DeliveryReport {
                attempts,
                result: Err(error.message),
                note: prepared.template_note.clone(),
                interrupted: false,
                retryable: error.transient,
            };
        };
        let delay = error
            .retry_after
            .map_or(*delay, |asked| asked.min(policy.max_retry_after));
        tracing::warn!(
            channel = channel.kind(),
            attempts,
            error = %error,
            retry_in_ms = delay.as_millis(),
            "notification failed, will retry"
        );
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = cancel.cancelled() => {
                return DeliveryReport {
                    attempts,
                    result: Err(format!("{}; gave up retrying: shutting down", error.message)),
                    note: prepared.template_note.clone(),
                    interrupted: true,
                    retryable: false,
                };
            }
        }
    }
}

/// Build the message for `event` (with `template`, if any, falling back to the built-in one)
/// and deliver it to `channel`, retrying as `policy` says.
pub async fn send_event(
    sender: &Sender,
    channel: &Channel,
    event: &Event,
    template: Option<&str>,
    policy: &RetryPolicy,
    cancel: &CancellationToken,
) -> DeliveryReport {
    let prepared = prepare(channel.markup(), channel.limit(), event, template);
    deliver(sender, channel, &prepared, event, policy, cancel).await
}

/// Send a test notification to `channel`, with retries, to check that the target works.
///
/// `template` is the custom template to try, if the target has one, so that a broken template
/// shows up here instead of on the first real change; `base_url` is Vigil's own address, if
/// configured.
pub async fn send_test(
    sender: &Sender,
    channel: &Channel,
    template: Option<&str>,
    base_url: Option<&str>,
    policy: &RetryPolicy,
) -> DeliveryReport {
    let event = Event::test(base_url);
    send_event(
        sender,
        channel,
        &event,
        template,
        policy,
        &CancellationToken::new(),
    )
    .await
}
