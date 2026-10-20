//! Gotify: `gotify://host[:port][/prefix]/APP_TOKEN` and `gotifys://` for HTTPS.

use url::Url;

use super::Secret;
use super::parse::{authority, decode, http_base, split_url};
use crate::error::{ParseError, invalid};

/// An application on a Gotify server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gotify {
    /// The server root, `http(s)://host[:port]/[prefix/]`.
    pub base: Url,
    /// The application token.
    pub token: Secret,
    /// Message priority 0 to 10.
    pub priority: Option<u8>,
}

pub(super) fn parse(tls: bool, input: &str) -> Result<Gotify, ParseError> {
    let scheme = if tls { "gotifys" } else { "gotify" };
    let url = split_url(scheme, input)?;
    if url.credentials(scheme)?.is_some() {
        return Err(invalid(
            scheme,
            "has no user:password@; the token goes last in the path",
        ));
    }
    let Some((token, prefix)) = url.segments.split_last() else {
        return Err(invalid(
            scheme,
            "needs the application token: gotify://host/TOKEN",
        ));
    };
    let token = decode(scheme, "the token", token)?;
    if token.is_empty()
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(invalid(
            scheme,
            "the token must be letters, digits, - _ or .",
        ));
    }

    let mut options = url.options(scheme);
    let priority = options
        .take("priority")?
        .map(|value| {
            value
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= 10)
                .ok_or_else(|| invalid(scheme, "priority= must be a number from 0 to 10"))
        })
        .transpose()?;
    options.finish("priority")?;

    let base = http_base(tls, &url.host, url.port, prefix)
        .map_err(|_| invalid(scheme, "has a host name that is not valid"))?;
    Ok(Gotify {
        base,
        token: Secret::new(token),
        priority,
    })
}

impl Gotify {
    /// The URL with the token replaced by `***`.
    pub(super) fn redacted(&self) -> String {
        let scheme = if self.base.scheme() == "https" {
            "gotifys"
        } else {
            "gotify"
        };
        let query = self
            .priority
            .map_or_else(String::new, |p| format!("?priority={p}"));
        format!(
            "{scheme}://{}{}***{query}",
            authority(&self.base),
            self.base.path()
        )
    }
}
