//! HTTP fetcher with a size cap, conditional requests and a redirect limit.

use std::time::Duration;

use reqwest::header::{
    CONTENT_TYPE, ETAG, HeaderMap, HeaderName, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH,
    LAST_MODIFIED,
};
use reqwest::redirect::Policy;
use reqwest::{StatusCode, Url};

/// Redirects followed before giving up.
const MAX_REDIRECTS: usize = 10;

/// Why a fetch failed.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// The URL could not be parsed.
    #[error("invalid url {0:?}")]
    InvalidUrl(String),
    /// Only `http` and `https` are fetched, never `file://` or anything else.
    #[error("unsupported url scheme {0:?}: only http and https are fetched")]
    UnsupportedScheme(String),
    /// A configured request header is not valid HTTP.
    #[error("invalid request header {0:?}")]
    InvalidHeader(String),
    /// The request exceeded its timeout.
    #[error("request timed out")]
    Timeout,
    /// The body grew past the size limit.
    #[error("response body exceeds the limit of {limit} bytes")]
    TooLarge {
        /// The configured limit in bytes.
        limit: u64,
    },
    /// The server answered with a status other than 2xx or a requested 304.
    #[error("server answered http {0}")]
    Status(u16),
    /// Connecting, TLS, redirects or reading the body failed. The cause is in the source chain.
    #[error("request failed")]
    Http(#[source] reqwest::Error),
}

/// Fetcher-wide defaults.
#[derive(Debug, Clone)]
pub struct FetchConfig {
    /// Timeout used when a request does not set its own.
    pub timeout: Duration,
    /// Largest accepted body, measured after decompression.
    pub max_body_bytes: u64,
    /// `User-Agent` used when a request does not set its own.
    pub user_agent: String,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_body_bytes: 10 * 1024 * 1024,
            user_agent: concat!("vigil/", env!("CARGO_PKG_VERSION")).to_owned(),
        }
    }
}

/// One fetch.
#[derive(Debug, Clone, Default)]
pub struct FetchRequest {
    /// The URL to GET.
    pub url: String,
    /// Overrides [`FetchConfig::timeout`].
    pub timeout: Option<Duration>,
    /// Overrides [`FetchConfig::user_agent`].
    pub user_agent: Option<String>,
    /// Extra headers, applied last.
    pub headers: Vec<(String, String)>,
    /// Sent as `If-None-Match`.
    pub etag: Option<String>,
    /// Sent as `If-Modified-Since`.
    pub last_modified: Option<String>,
}

impl FetchRequest {
    /// A plain GET of `url`.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Self::default()
        }
    }
}

/// A successful response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedBody {
    /// HTTP status (2xx).
    pub status: u16,
    /// The URL after redirects.
    pub final_url: String,
    /// `Content-Type` header.
    pub content_type: Option<String>,
    /// `ETag` header.
    pub etag: Option<String>,
    /// `Last-Modified` header.
    pub last_modified: Option<String>,
    /// The decompressed body.
    pub body: Vec<u8>,
}

/// What a fetch returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOutcome {
    /// The server confirmed the validators we sent; no body was downloaded.
    NotModified {
        /// `ETag` header of the 304, if any.
        etag: Option<String>,
        /// `Last-Modified` header of the 304, if any.
        last_modified: Option<String>,
    },
    /// A fresh body.
    Body(FetchedBody),
}

/// HTTP client shared by all checks.
#[derive(Debug, Clone)]
pub struct Fetcher {
    client: reqwest::Client,
    config: FetchConfig,
}

impl Fetcher {
    /// Build a fetcher. Follows at most 10 redirects; cookies are not kept between requests.
    pub fn new(config: FetchConfig) -> Result<Self, FetchError> {
        let client = reqwest::Client::builder()
            .redirect(Policy::limited(MAX_REDIRECTS))
            .build()
            .map_err(FetchError::Http)?;
        Ok(Self { client, config })
    }

    /// GET `request.url`.
    ///
    /// `If-None-Match` and `If-Modified-Since` are sent only when the request carries the
    /// validators. A 304 is accepted only in that case.
    pub async fn fetch(&self, request: &FetchRequest) -> Result<FetchOutcome, FetchError> {
        let url = parse_url(&request.url)?;
        let conditional = request.etag.is_some() || request.last_modified.is_some();

        let mut builder = self
            .client
            .get(url)
            .timeout(request.timeout.unwrap_or(self.config.timeout))
            .header(
                reqwest::header::USER_AGENT,
                request
                    .user_agent
                    .as_deref()
                    .unwrap_or(&self.config.user_agent),
            );
        if let Some(etag) = &request.etag {
            builder = builder.header(IF_NONE_MATCH, header_value(etag)?);
        }
        if let Some(modified) = &request.last_modified {
            builder = builder.header(IF_MODIFIED_SINCE, header_value(modified)?);
        }
        for (name, value) in &request.headers {
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| FetchError::InvalidHeader(name.clone()))?;
            builder = builder.header(header_name, header_value(value)?);
        }

        let mut response = builder.send().await.map_err(map_error)?;
        let status = response.status();

        if status == StatusCode::NOT_MODIFIED && conditional {
            let headers = response.headers();
            return Ok(FetchOutcome::NotModified {
                etag: header_text(headers, &ETAG),
                last_modified: header_text(headers, &LAST_MODIFIED),
            });
        }
        if !status.is_success() {
            return Err(FetchError::Status(status.as_u16()));
        }

        let limit = self.config.max_body_bytes;
        if response.content_length().is_some_and(|len| len > limit) {
            return Err(FetchError::TooLarge { limit });
        }

        let headers = response.headers().clone();
        let final_url = response.url().to_string();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(map_error)? {
            if body.len() as u64 + chunk.len() as u64 > limit {
                return Err(FetchError::TooLarge { limit });
            }
            body.extend_from_slice(&chunk);
        }

        Ok(FetchOutcome::Body(FetchedBody {
            status: status.as_u16(),
            final_url,
            content_type: header_text(&headers, &CONTENT_TYPE),
            etag: header_text(&headers, &ETAG),
            last_modified: header_text(&headers, &LAST_MODIFIED),
            body,
        }))
    }
}

/// The `host:port` a URL connects to, lowercased, used to rate-limit per host.
///
/// Falls back to the whole text when it is not a parseable http(s) URL.
pub fn host_key(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| {
            let host = u.host_str()?.to_ascii_lowercase();
            Some(match u.port_or_known_default() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            })
        })
        .unwrap_or_else(|| url.to_owned())
}

fn parse_url(text: &str) -> Result<Url, FetchError> {
    let url = Url::parse(text).map_err(|_| FetchError::InvalidUrl(text.to_owned()))?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(FetchError::UnsupportedScheme(other.to_owned())),
    }
}

fn header_value(text: &str) -> Result<HeaderValue, FetchError> {
    HeaderValue::from_str(text).map_err(|_| FetchError::InvalidHeader(text.to_owned()))
}

fn header_text(headers: &HeaderMap, name: &HeaderName) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_owned)
}

fn map_error(error: reqwest::Error) -> FetchError {
    if error.is_timeout() {
        FetchError::Timeout
    } else {
        FetchError::Http(error)
    }
}

#[cfg(test)]
mod tests {
    use super::host_key;

    #[test]
    fn host_key_includes_the_effective_port() {
        assert_eq!(host_key("https://Example.com/a?b=1"), "example.com:443");
        assert_eq!(host_key("http://example.com/"), "example.com:80");
        assert_eq!(host_key("http://example.com:8080/x"), "example.com:8080");
    }

    #[test]
    fn host_key_falls_back_to_the_input() {
        assert_eq!(host_key("not a url"), "not a url");
    }
}
