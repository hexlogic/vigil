//! Delivery with retries.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::channel::Channel;
use crate::event::Event;
use crate::message::Prepared;
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
                };
            }
        }
    }
}
