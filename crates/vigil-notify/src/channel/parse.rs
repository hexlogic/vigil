//! Helpers shared by the URL parsers.

use std::borrow::Cow;
use std::net::IpAddr;

use percent_encoding::percent_decode_str;
use url::Url;

use crate::error::{ParseError, invalid};
use crate::secret::Secret;

/// Percent-decode `text`, which must decode to UTF-8.
pub(super) fn decode(scheme: &'static str, what: &str, text: &str) -> Result<String, ParseError> {
    percent_decode_str(text)
        .decode_utf8()
        .map(Cow::into_owned)
        .map_err(|_| invalid(scheme, format!("{what} is not valid UTF-8")))
}

/// Whether `host` is the local machine: `localhost` or a loopback address.
pub(super) fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Split `rest` (a URL without its scheme) into the part before `?` and the query.
pub(super) fn split_query(rest: &str) -> (&str, Option<&str>) {
    match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    }
}

/// `host` or `host:port`.
pub(super) fn authority(url: &Url) -> String {
    authority_of(url.host_str().unwrap_or_default(), url.port())
}

/// `host` or `host:port`.
pub(super) fn authority_of(host: &str, port: Option<u16>) -> String {
    match port {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

/// The server root `http(s)://host[:port]/segment/.../`, with every segment already
/// percent-encoded.
pub(super) fn http_base(
    tls: bool,
    host: &str,
    port: Option<u16>,
    segments: &[String],
) -> Result<Url, url::ParseError> {
    let mut text = format!("{}://{host}", if tls { "https" } else { "http" });
    if let Some(port) = port {
        text.push_str(&format!(":{port}"));
    }
    text.push('/');
    for segment in segments {
        text.push_str(segment);
        text.push('/');
    }
    Url::parse(&text)
}

/// A comma-separated list without blanks.
pub(super) fn split_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The `?key=value` options of a URL. Unknown and repeated options are errors.
pub(super) struct Query {
    scheme: &'static str,
    pairs: Vec<(String, String)>,
}

impl Query {
    /// Parse a raw query string.
    pub(super) fn parse(scheme: &'static str, raw: Option<&str>) -> Self {
        let pairs = raw
            .map(|query| {
                url::form_urlencoded::parse(query.as_bytes())
                    .map(|(key, value)| (key.to_ascii_lowercase(), value.into_owned()))
                    .collect()
            })
            .unwrap_or_default();
        Self { scheme, pairs }
    }

    /// Remove and return an option that may be given once.
    pub(super) fn take(&mut self, key: &str) -> Result<Option<String>, ParseError> {
        let mut found = self.take_all(key);
        match found.len() {
            0 => Ok(None),
            1 => Ok(found.pop()),
            _ => Err(invalid(
                self.scheme,
                format!("option {key}= is given more than once"),
            )),
        }
    }

    /// Remove and return every value of an option.
    pub(super) fn take_all(&mut self, key: &str) -> Vec<String> {
        let (taken, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pairs)
            .into_iter()
            .partition(|(name, _)| name == key);
        self.pairs = rest;
        taken.into_iter().map(|(_, value)| value).collect()
    }

    /// Fail if options are left over. The leftover names are not repeated: a mistyped URL can
    /// put a credential where an option name goes.
    pub(super) fn finish(self, supported: &str) -> Result<(), ParseError> {
        if self.pairs.is_empty() {
            return Ok(());
        }
        Err(invalid(
            self.scheme,
            format!("has an option it does not know; supported options: {supported}"),
        ))
    }
}

/// The parts of a URL that is parsed with the `url` crate.
pub(super) struct HttpUrl {
    /// Lowercase host; IPv6 addresses keep their brackets.
    pub host: String,
    /// The explicit port, if any.
    pub port: Option<u16>,
    /// The raw path, starting with `/` unless empty.
    pub path: String,
    /// The non-empty path segments, still percent-encoded.
    pub segments: Vec<String>,
    /// The raw query string.
    pub query: Option<String>,
    username: String,
    password: Option<String>,
}

impl HttpUrl {
    /// The `?key=value` options.
    pub(super) fn options(&self, scheme: &'static str) -> Query {
        Query::parse(scheme, self.query.as_deref())
    }

    /// `user:password@`, which must give both or neither.
    pub(super) fn credentials(
        &self,
        scheme: &'static str,
    ) -> Result<Option<(String, Secret)>, ParseError> {
        match (self.username.is_empty(), &self.password) {
            (true, None) => Ok(None),
            (false, Some(password)) if !password.is_empty() => {
                Ok(Some((self.username.clone(), Secret::new(password.clone()))))
            }
            _ => Err(invalid(
                scheme,
                "needs both a user name and a password: user:password@host",
            )),
        }
    }
}

/// Parse a URL of one of the `host[:port]/path` schemes. Errors never quote the input.
pub(super) fn split_url(scheme: &'static str, input: &str) -> Result<HttpUrl, ParseError> {
    let parsed = Url::parse(input)
        .map_err(|error| invalid(scheme, format!("is not a valid URL: {error}")))?;
    if parsed.fragment().is_some() {
        return Err(invalid(scheme, "does not take a #fragment"));
    }
    let host = match parsed.host_str() {
        Some(host) if !host.is_empty() => host.to_ascii_lowercase(),
        _ => return Err(invalid(scheme, "needs a host name")),
    };
    let segments = parsed
        .path_segments()
        .map(|parts| {
            parts
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Ok(HttpUrl {
        host,
        port: parsed.port(),
        path: parsed.path().to_owned(),
        segments,
        query: parsed.query().map(str::to_owned),
        username: decode(scheme, "the user name", parsed.username())?,
        password: parsed
            .password()
            .map(|password| decode(scheme, "the password", password))
            .transpose()?,
    })
}
