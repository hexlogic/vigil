//! ntfy: `ntfy://[user:password@]host[:port][/prefix]/topic`.

use url::Url;

use super::parse::{authority, decode, http_base, split_list, split_url};
use super::{HttpAuth, Secret};
use crate::error::{ParseError, invalid};

/// A topic on an ntfy server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ntfy {
    /// The server root, `http(s)://host[:port]/[prefix/]`.
    pub base: Url,
    /// The topic. On a public server anyone who knows it can read the messages, so it is a
    /// secret.
    pub topic: Secret,
    /// Message priority 1 (min) to 5 (max).
    pub priority: Option<u8>,
    /// Tags (emoji short codes) to attach.
    pub tags: Vec<String>,
    /// Credentials for a protected server.
    pub auth: Option<HttpAuth>,
}

pub(super) fn parse(tls: bool, input: &str) -> Result<Ntfy, ParseError> {
    let scheme = if tls { "ntfys" } else { "ntfy" };
    let url = split_url(scheme, input)?;
    let Some((topic, prefix)) = url.segments.split_last() else {
        return Err(invalid(scheme, "needs a topic: ntfy://host/topic"));
    };
    let topic = decode(scheme, "the topic", topic)?;
    if topic.is_empty()
        || topic.len() > 64
        || !topic
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(invalid(
            scheme,
            "the topic must be 1-64 letters, digits, - or _",
        ));
    }

    let mut options = url.options(scheme);
    let priority = options
        .take("priority")?
        .map(|value| priority(scheme, &value))
        .transpose()?;
    let tags = options
        .take("tags")?
        .map_or_else(Vec::new, |v| split_list(&v));
    let token = options.take("token")?;
    options.finish("priority, tags, token")?;

    let auth = match (url.credentials(scheme)?, token) {
        (Some(_), Some(_)) => {
            return Err(invalid(
                scheme,
                "use either user:password@ or token=, not both",
            ));
        }
        (Some((user, password)), None) => Some(HttpAuth::Basic { user, password }),
        (None, Some(token)) if token.is_empty() => {
            return Err(invalid(scheme, "token= is empty"));
        }
        (None, Some(token)) => Some(HttpAuth::Bearer(Secret::new(token))),
        (None, None) => None,
    };
    let base = http_base(tls, &url.host, url.port, prefix)
        .map_err(|_| invalid(scheme, "has a host name that is not valid"))?;
    Ok(Ntfy {
        base,
        topic: Secret::new(topic),
        priority,
        tags,
        auth,
    })
}

fn priority(scheme: &'static str, value: &str) -> Result<u8, ParseError> {
    match value.to_ascii_lowercase().as_str() {
        "min" | "1" => Ok(1),
        "low" | "2" => Ok(2),
        "default" | "3" => Ok(3),
        "high" | "4" => Ok(4),
        "max" | "urgent" | "5" => Ok(5),
        _ => Err(invalid(
            scheme,
            "priority= must be 1-5 or min, low, default, high, max",
        )),
    }
}

impl Ntfy {
    /// The URL with every secret replaced by `***`.
    pub(super) fn redacted(&self) -> String {
        let scheme = if self.base.scheme() == "https" {
            "ntfys"
        } else {
            "ntfy"
        };
        let login = match &self.auth {
            Some(HttpAuth::Basic { user, .. }) => format!("{user}:***@"),
            _ => String::new(),
        };
        let mut options = Vec::new();
        if let Some(priority) = self.priority {
            options.push(format!("priority={priority}"));
        }
        if !self.tags.is_empty() {
            options.push(format!("tags={}", self.tags.join(",")));
        }
        if matches!(self.auth, Some(HttpAuth::Bearer(_))) {
            options.push("token=***".to_owned());
        }
        let query = if options.is_empty() {
            String::new()
        } else {
            format!("?{}", options.join("&"))
        };
        format!(
            "{scheme}://{login}{}{}***{query}",
            authority(&self.base),
            self.base.path()
        )
    }
}
