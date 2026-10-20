//! Slack: `slack://T000/B000/XXXX`, the three parts of an incoming-webhook path.

use super::Secret;
use crate::error::{ParseError, invalid};

const SCHEME: &str = "slack";

/// A Slack channel reached through an incoming webhook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slack {
    /// The workspace part, `T…`.
    pub team: String,
    /// The app part, `B…`.
    pub bot: String,
    /// The secret part.
    pub token: Secret,
}

pub(super) fn parse(rest: &str) -> Result<Slack, ParseError> {
    if rest.contains('?') {
        return Err(invalid(SCHEME, "takes no options"));
    }
    let mut parts = rest.trim_end_matches('/').split('/');
    let (Some(team), Some(bot), Some(token), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid(SCHEME, "expected slack://T000/B000/XXXX"));
    };
    for part in [team, bot, token] {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(invalid(SCHEME, "each part must be letters and digits"));
        }
    }
    Ok(Slack {
        team: team.to_owned(),
        bot: bot.to_owned(),
        token: Secret::new(token),
    })
}

impl Slack {
    /// The URL with the secret part replaced by `***`.
    pub(super) fn redacted(&self) -> String {
        format!("slack://{}/{}/***", self.team, self.bot)
    }
}
