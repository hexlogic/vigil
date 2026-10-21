//! The notification channels and the URLs that name them.

mod discord;
mod email;
mod gotify;
mod ntfy;
mod parse;
mod slack;
mod telegram;
mod webhook;

use std::str::FromStr;

pub use discord::Discord;
pub use email::{Email, SmtpTls};
pub use gotify::Gotify;
pub use ntfy::Ntfy;
pub use slack::Slack;
pub use telegram::Telegram;
pub use webhook::{Webhook, WebhookFormat};

use crate::error::ParseError;
use crate::escape::Markup;
pub use crate::secret::Secret;
use crate::truncate::Limit;

/// Every URL scheme that names a channel.
pub const SCHEMES: &[&str] = &[
    "ntfy", "ntfys", "tgram", "discord", "slack", "mailto", "mailtos", "json", "jsons", "form",
    "forms", "gotify", "gotifys",
];

/// Credentials sent in an HTTP header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpAuth {
    /// `Authorization: Basic`, from `user:password@` in the URL.
    Basic {
        /// The user name.
        user: String,
        /// The password.
        password: Secret,
    },
    /// `Authorization: Bearer`, from a `token=` option.
    Bearer(Secret),
}

/// Where a notification goes, parsed from a notification URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channel {
    /// An ntfy topic.
    Ntfy(Ntfy),
    /// A Telegram chat.
    Telegram(Telegram),
    /// A Discord webhook.
    Discord(Discord),
    /// A Slack incoming webhook.
    Slack(Slack),
    /// Email through an SMTP server.
    Email(Email),
    /// A generic JSON or form webhook.
    Webhook(Webhook),
    /// A Gotify application.
    Gotify(Gotify),
}

impl Channel {
    /// Parse a notification URL.
    ///
    /// Unknown schemes give [`ParseError::Unsupported`]; anything else wrong with the URL
    /// gives [`ParseError::Invalid`]. No error quotes the URL.
    pub fn parse(url: &str) -> Result<Self, ParseError> {
        let url = url.trim();
        let (scheme, rest) = url.split_once("://").ok_or(ParseError::NotAUrl)?;
        let scheme = scheme.to_ascii_lowercase();
        let plausible = !scheme.is_empty()
            && scheme.len() <= 16
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
        if !plausible {
            return Err(ParseError::NotAUrl);
        }
        use WebhookFormat::{Form, Json};
        Ok(match scheme.as_str() {
            "ntfy" => Self::Ntfy(ntfy::parse(false, url)?),
            "ntfys" => Self::Ntfy(ntfy::parse(true, url)?),
            "tgram" => Self::Telegram(telegram::parse(rest)?),
            "discord" => Self::Discord(discord::parse(rest)?),
            "slack" => Self::Slack(slack::parse(rest)?),
            "mailto" => Self::Email(email::parse(false, url)?),
            "mailtos" => Self::Email(email::parse(true, url)?),
            "json" => Self::Webhook(webhook::parse(false, Json, url)?),
            "jsons" => Self::Webhook(webhook::parse(true, Json, url)?),
            "form" => Self::Webhook(webhook::parse(false, Form, url)?),
            "forms" => Self::Webhook(webhook::parse(true, Form, url)?),
            "gotify" => Self::Gotify(gotify::parse(false, url)?),
            "gotifys" => Self::Gotify(gotify::parse(true, url)?),
            _ => {
                return Err(ParseError::Unsupported {
                    scheme,
                    supported: SCHEMES.join(", "),
                });
            }
        })
    }

    /// The URL with every secret replaced by `***`: the only form that may be shown or logged.
    pub fn redacted(&self) -> String {
        match self {
            Self::Ntfy(c) => c.redacted(),
            Self::Telegram(c) => c.redacted(),
            Self::Discord(c) => c.redacted(),
            Self::Slack(c) => c.redacted(),
            Self::Email(c) => c.redacted(),
            Self::Webhook(c) => c.redacted(),
            Self::Gotify(c) => c.redacted(),
        }
    }

    /// A short name for the kind of channel: `ntfy`, `telegram`, `discord`, `slack`, `email`,
    /// `webhook` or `gotify`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Ntfy(_) => "ntfy",
            Self::Telegram(_) => "telegram",
            Self::Discord(_) => "discord",
            Self::Slack(_) => "slack",
            Self::Email(_) => "email",
            Self::Webhook(_) => "webhook",
            Self::Gotify(_) => "gotify",
        }
    }

    /// The markup this channel's message body is written in.
    pub fn markup(&self) -> Markup {
        match self {
            Self::Telegram(_) => Markup::TelegramHtml,
            Self::Discord(_) => Markup::DiscordMarkdown,
            Self::Slack(_) => Markup::SlackMrkdwn,
            Self::Ntfy(_) | Self::Email(_) | Self::Webhook(_) | Self::Gotify(_) => Markup::Plain,
        }
    }

    /// How long a message body may be, from each service's documented limit (Slack's is a
    /// conservative choice: it accepts more but truncates long messages on display).
    pub fn limit(&self) -> Limit {
        match self {
            Self::Ntfy(_) => Limit::Bytes(4096),
            Self::Telegram(_) => Limit::Chars(4096),
            Self::Discord(_) => Limit::Chars(2000),
            Self::Slack(_) => Limit::Chars(4000),
            Self::Gotify(_) => Limit::Chars(32_000),
            Self::Email(_) | Self::Webhook(_) => Limit::Bytes(256 * 1024),
        }
    }

    /// Whether this URL sends a secret over plain HTTP to a host other than this machine.
    ///
    /// The secret may be a password or token, an ntfy topic (on a public server it is the only
    /// protection) or the path or query of a webhook (where its token usually is). Such a URL is
    /// accepted, since plain HTTP on a trusted network is common, but worth a warning.
    pub fn sends_secrets_in_cleartext(&self) -> bool {
        let remote_http = |url: &url::Url| {
            url.scheme() == "http" && !url.host_str().is_some_and(parse::is_loopback)
        };
        match self {
            Self::Ntfy(c) => remote_http(&c.base),
            Self::Gotify(c) => remote_http(&c.base),
            Self::Webhook(c) => {
                let url = c.url.expose();
                remote_http(url) && (c.auth.is_some() || url.path() != "/" || url.query().is_some())
            }
            Self::Telegram(_) | Self::Discord(_) | Self::Slack(_) | Self::Email(_) => false,
        }
    }
}

impl FromStr for Channel {
    type Err = ParseError;

    fn from_str(url: &str) -> Result<Self, Self::Err> {
        Self::parse(url)
    }
}
