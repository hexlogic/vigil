//! Discord: `discord://WEBHOOK_ID/WEBHOOK_TOKEN`.

use super::Secret;
use super::parse::{Query, split_query};
use crate::error::{ParseError, invalid};

const SCHEME: &str = "discord";

/// A Discord channel reached through a webhook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discord {
    /// The numeric webhook id.
    pub id: String,
    /// The webhook token.
    pub token: Secret,
    /// Name shown as the sender instead of the webhook's own.
    pub username: Option<String>,
}

pub(super) fn parse(rest: &str) -> Result<Discord, ParseError> {
    let (path, query) = split_query(rest);
    let mut parts = path.trim_end_matches('/').split('/');
    let (Some(id), Some(token), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(invalid(
            SCHEME,
            "expected discord://WEBHOOK_ID/WEBHOOK_TOKEN",
        ));
    };
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid(SCHEME, "the webhook id must be a number"));
    }
    if token.is_empty()
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(invalid(
            SCHEME,
            "the webhook token must be letters, digits, - or _",
        ));
    }

    let mut options = Query::parse(SCHEME, query);
    let username = options.take("username")?;
    options.finish("username")?;
    if username
        .as_ref()
        .is_some_and(|name| name.is_empty() || name.chars().count() > 80)
    {
        return Err(invalid(SCHEME, "username= must be 1-80 characters"));
    }
    Ok(Discord {
        id: id.to_owned(),
        token: Secret::new(token),
        username,
    })
}

impl Discord {
    /// The URL with the webhook token replaced by `***`.
    pub(super) fn redacted(&self) -> String {
        format!("discord://{}/***", self.id)
    }
}
