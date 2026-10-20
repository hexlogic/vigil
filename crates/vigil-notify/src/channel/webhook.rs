//! Generic webhooks: `json://` and `form://` (plain HTTP), `jsons://` and `forms://` (HTTPS).

use url::Url;

use super::HttpAuth;
use super::parse::{authority_of, split_url};
use crate::error::{ParseError, invalid};
use crate::secret::SecretUrl;

/// How a webhook's body is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookFormat {
    /// A JSON object.
    Json,
    /// `application/x-www-form-urlencoded` fields.
    Form,
}

/// A URL that receives each notification as an HTTP POST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Webhook {
    /// Where to POST. Webhook URLs often carry a token in the path or query.
    pub url: SecretUrl,
    /// How the body is encoded.
    pub format: WebhookFormat,
    /// HTTP Basic credentials, if the URL had `user:password@`.
    pub auth: Option<HttpAuth>,
}

pub(super) fn parse(tls: bool, format: WebhookFormat, input: &str) -> Result<Webhook, ParseError> {
    let scheme = match (format, tls) {
        (WebhookFormat::Json, false) => "json",
        (WebhookFormat::Json, true) => "jsons",
        (WebhookFormat::Form, false) => "form",
        (WebhookFormat::Form, true) => "forms",
    };
    let url = split_url(scheme, input)?;
    let auth = url
        .credentials(scheme)?
        .map(|(user, password)| HttpAuth::Basic { user, password });

    // The query is passed through untouched: webhook URLs use it for tokens and routing.
    let mut target = format!(
        "{}://{}{}",
        if tls { "https" } else { "http" },
        authority_of(&url.host, url.port),
        if url.path.is_empty() { "/" } else { &url.path },
    );
    if let Some(query) = &url.query {
        target.push('?');
        target.push_str(query);
    }
    let target =
        Url::parse(&target).map_err(|_| invalid(scheme, "has a host name that is not valid"))?;
    Ok(Webhook {
        url: SecretUrl::new(target),
        format,
        auth,
    })
}

impl Webhook {
    /// The URL with the path, the query and the password hidden.
    pub(super) fn redacted(&self) -> String {
        let url = self.url.expose();
        let scheme = match (self.format, url.scheme() == "https") {
            (WebhookFormat::Json, false) => "json",
            (WebhookFormat::Json, true) => "jsons",
            (WebhookFormat::Form, false) => "form",
            (WebhookFormat::Form, true) => "forms",
        };
        let login = match &self.auth {
            Some(HttpAuth::Basic { user, .. }) => format!("{user}:***@"),
            _ => String::new(),
        };
        let hidden = if url.path() == "/" && url.query().is_none() {
            ""
        } else {
            "***"
        };
        format!(
            "{scheme}://{login}{}/{hidden}",
            authority_of(url.host_str().unwrap_or_default(), url.port())
        )
    }
}
