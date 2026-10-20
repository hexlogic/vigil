//! Telegram: `tgram://BOT_TOKEN/CHAT_ID`.

use super::Secret;
use super::parse::{decode, split_query};
use crate::error::{ParseError, invalid};

const SCHEME: &str = "tgram";

/// A Telegram chat reached through a bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Telegram {
    /// The bot token, `123456:ABC-DEF`.
    pub token: Secret,
    /// A numeric chat id (negative for groups and channels) or `@channelname`.
    pub chat_id: String,
}

// The URL is split by hand: a bot token is `digits:letters`, which the URL grammar would read
// as a host and a port.
pub(super) fn parse(rest: &str) -> Result<Telegram, ParseError> {
    let (path, query) = split_query(rest);
    if query.is_some() {
        return Err(invalid(SCHEME, "takes no options"));
    }
    let mut parts = path.trim_end_matches('/').split('/');
    let (Some(token), Some(chat_id), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(invalid(SCHEME, "expected tgram://BOT_TOKEN/CHAT_ID"));
    };

    let valid_token = token.split_once(':').is_some_and(|(id, key)| {
        !id.is_empty()
            && id.bytes().all(|b| b.is_ascii_digit())
            && !key.is_empty()
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    });
    if !valid_token {
        return Err(invalid(
            SCHEME,
            "the bot token should look like 123456:ABC-DEF",
        ));
    }

    let chat_id = decode(SCHEME, "the chat id", chat_id)?;
    let numeric = chat_id.strip_prefix('-').unwrap_or(&chat_id);
    let valid_chat = (!numeric.is_empty() && numeric.bytes().all(|b| b.is_ascii_digit()))
        || chat_id.strip_prefix('@').is_some_and(|name| {
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        });
    if !valid_chat {
        return Err(invalid(
            SCHEME,
            "the chat id must be a number (negative for groups) or @channelname",
        ));
    }
    Ok(Telegram {
        token: Secret::new(token),
        chat_id,
    })
}

impl Telegram {
    /// The URL with the bot token replaced by `***`.
    pub(super) fn redacted(&self) -> String {
        format!("tgram://***/{}", self.chat_id)
    }
}
