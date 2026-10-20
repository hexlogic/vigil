//! Notification URL parsing, channels and message templating.
//!
//! A notification target is a URL such as `ntfy://ntfy.sh/my-topic` or
//! `tgram://123456:token/-100123`. [`Channel::parse`] turns it into a [`Channel`], checking it
//! strictly and explaining what is wrong without repeating the URL, which usually holds
//! credentials. Every credential is a [`Secret`] whose `Debug` and `Display` print `***`, and
//! [`Channel::redacted`] is the form that may be shown or logged.
//!
//! A change becomes a message in two steps: [`Event`] holds the facts, and [`prepare`] renders
//! them with a template, escaped for the channel's [`Markup`] and cut to its [`Limit`].

mod builtin;
mod channel;
mod error;
mod escape;
mod event;
mod message;
mod request;
mod retry;
mod secret;
mod send;
mod template;
mod truncate;

pub use channel::{
    Channel, Discord, Email, Gotify, HttpAuth, Ntfy, SCHEMES, Slack, SmtpTls, Telegram, Webhook,
    WebhookFormat,
};
pub use error::{ParseError, SendError, TemplateError};
pub use escape::{Markup, code_block, escape, link};
pub use event::Event;
pub use message::{Prepared, prepare};
pub use request::{Endpoints, HttpBody, HttpRequest, http_request};
pub use retry::{DeliveryReport, RetryPolicy, deliver};
pub use secret::{Secret, SecretUrl};
pub use send::{Sender, SenderConfig};
pub use template::validate_template;
pub use truncate::Limit;
