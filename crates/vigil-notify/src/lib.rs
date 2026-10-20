//! Notification URL parsing, channels and message templating.
//!
//! A notification target is a URL such as `ntfy://ntfy.sh/my-topic` or
//! `tgram://123456:token/-100123`. [`Channel::parse`] turns it into a [`Channel`], checking it
//! strictly and explaining what is wrong without repeating the URL, which usually holds
//! credentials. Every credential is a [`Secret`] whose `Debug` and `Display` print `***`, and
//! [`Channel::redacted`] is the form that may be shown or logged.

mod channel;
mod error;
mod secret;

pub use channel::{
    Channel, Discord, Email, Gotify, HttpAuth, Ntfy, SCHEMES, Slack, SmtpTls, Telegram, Webhook,
    WebhookFormat,
};
pub use error::ParseError;
pub use secret::{Secret, SecretUrl};
