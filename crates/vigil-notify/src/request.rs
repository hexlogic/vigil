//! The HTTP request each channel sends. Building it is pure: nothing is sent here.

use serde_json::{Value, json};
use url::Url;

use crate::channel::{
    Channel, Discord, Gotify, HttpAuth, Ntfy, Slack, Telegram, Webhook, WebhookFormat,
};
use crate::error::SendError;
use crate::event::Event;
use crate::message::Prepared;
use crate::secret::{Secret, SecretUrl};

/// Most bytes of added or removed lines put in a webhook's JSON or form body.
const MAX_LINES_BYTES: usize = 256 * 1024;

/// Where the services with a fixed address are reached.
///
/// The defaults are the public services. Changing them points Telegram at a self-hosted Bot API
/// server, and lets tests use a local server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// Telegram Bot API, `https://api.telegram.org`.
    pub telegram: String,
    /// Discord, `https://discord.com`.
    pub discord: String,
    /// Slack incoming webhooks, `https://hooks.slack.com`.
    pub slack: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            telegram: "https://api.telegram.org".into(),
            discord: "https://discord.com".into(),
            slack: "https://hooks.slack.com".into(),
        }
    }
}

/// The body of a request.
///
/// `Debug` leaves the content out: it is the message, and for ntfy it holds the topic.
#[derive(Clone, PartialEq, Eq)]
pub enum HttpBody {
    /// A JSON document.
    Json(Value),
    /// `application/x-www-form-urlencoded` fields.
    Form(Vec<(String, String)>),
}

impl std::fmt::Debug for HttpBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(_) => f.write_str("Json(..)"),
            Self::Form(fields) => write!(f, "Form({} fields)", fields.len()),
        }
    }
}

/// A POST request, with its secrets kept apart so that `Debug` is safe to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// Where to POST. For most services the URL itself is a credential.
    pub url: SecretUrl,
    /// `Authorization` credentials.
    pub auth: Option<HttpAuth>,
    /// Other headers whose value is a credential.
    pub secret_headers: Vec<(&'static str, Secret)>,
    /// Credentials that are part of the body, such as an ntfy topic.
    pub body_secrets: Vec<Secret>,
    /// The body.
    pub body: HttpBody,
}

impl HttpRequest {
    /// Strings that must not appear in anything shown to the user: a service may echo the
    /// request path or a token back in an error page.
    pub(crate) fn secrets(&self) -> Vec<String> {
        let url = self.url.expose();
        let mut secrets = vec![url.path().to_owned()];
        secrets.extend(url.query().map(str::to_owned));
        match &self.auth {
            Some(HttpAuth::Basic { password, .. }) => secrets.push(password.expose().to_owned()),
            Some(HttpAuth::Bearer(token)) => secrets.push(token.expose().to_owned()),
            None => {}
        }
        secrets.extend(
            self.secret_headers
                .iter()
                .map(|(_, s)| s.expose().to_owned()),
        );
        secrets.extend(self.body_secrets.iter().map(|s| s.expose().to_owned()));
        // "/" and the like would blank out half of any message.
        secrets.retain(|s| s.len() > 3);
        secrets
    }
}

/// Build the request that delivers `prepared` to `channel`.
///
/// Email is not sent over HTTP: asking for its request is an error (see [`crate::Sender`],
/// which sends it with SMTP).
pub fn http_request(
    channel: &Channel,
    prepared: &Prepared,
    event: &Event,
    endpoints: &Endpoints,
) -> Result<HttpRequest, SendError> {
    Ok(match channel {
        Channel::Ntfy(c) => ntfy(c, prepared, event),
        Channel::Telegram(c) => telegram(c, prepared, endpoints)?,
        Channel::Discord(c) => discord(c, prepared, endpoints)?,
        Channel::Slack(c) => slack(c, prepared, endpoints)?,
        Channel::Webhook(c) => webhook(c, prepared, event),
        Channel::Gotify(c) => gotify(c, prepared)?,
        Channel::Email(_) => return Err(SendError::permanent("email is not sent over HTTP")),
    })
}

fn json_request(url: Url, auth: Option<HttpAuth>, body: Value) -> HttpRequest {
    HttpRequest {
        url: SecretUrl::new(url),
        auth,
        secret_headers: Vec::new(),
        body_secrets: Vec::new(),
        body: HttpBody::Json(body),
    }
}

/// `base` + `path`, where `base` is one of the configurable [`Endpoints`].
fn endpoint(service: &str, base: &str, path: &str) -> Result<Url, SendError> {
    Url::parse(&format!("{}/{path}", base.trim_end_matches('/')))
        .map_err(|_| SendError::permanent(format!("the {service} endpoint is not a valid URL")))
}

fn ntfy(c: &Ntfy, prepared: &Prepared, event: &Event) -> HttpRequest {
    let mut body = json!({
        "topic": c.topic.expose(),
        "title": prepared.title,
        "message": prepared.body,
    });
    if let Some(priority) = c.priority {
        body["priority"] = json!(priority);
    }
    if !c.tags.is_empty() {
        body["tags"] = json!(c.tags);
    }
    if !event.diff_url.is_empty() {
        body["click"] = json!(event.diff_url);
    }
    let mut request = json_request(c.base.clone(), c.auth.clone(), body);
    request.body_secrets.push(c.topic.clone());
    request
}

fn telegram(
    c: &Telegram,
    prepared: &Prepared,
    endpoints: &Endpoints,
) -> Result<HttpRequest, SendError> {
    let url = endpoint(
        "telegram",
        &endpoints.telegram,
        &format!("bot{}/sendMessage", c.token.expose()),
    )?;
    // Numeric ids are sent as numbers, as the Bot API documents; `@name` stays a string.
    let chat_id = c
        .chat_id
        .parse::<i64>()
        .map_or_else(|_| json!(c.chat_id), |id| json!(id));
    Ok(json_request(
        url,
        None,
        json!({
            "chat_id": chat_id,
            "text": prepared.body,
            "parse_mode": "HTML",
            "link_preview_options": { "is_disabled": true },
        }),
    ))
}

fn discord(
    c: &Discord,
    prepared: &Prepared,
    endpoints: &Endpoints,
) -> Result<HttpRequest, SendError> {
    // `wait=true` makes Discord answer after it has accepted the message, so errors show up.
    let url = endpoint(
        "discord",
        &endpoints.discord,
        &format!("api/webhooks/{}/{}?wait=true", c.id, c.token.expose()),
    )?;
    let mut body = json!({
        "content": prepared.body,
        // Nothing in a page may ping anyone.
        "allowed_mentions": { "parse": [] },
    });
    if let Some(username) = &c.username {
        body["username"] = json!(username);
    }
    Ok(json_request(url, None, body))
}

fn slack(c: &Slack, prepared: &Prepared, endpoints: &Endpoints) -> Result<HttpRequest, SendError> {
    let url = endpoint(
        "slack",
        &endpoints.slack,
        &format!("services/{}/{}/{}", c.team, c.bot, c.token.expose()),
    )?;
    Ok(json_request(url, None, json!({ "text": prepared.body })))
}

fn gotify(c: &Gotify, prepared: &Prepared) -> Result<HttpRequest, SendError> {
    let url = c
        .base
        .join("message")
        .map_err(|_| SendError::permanent("the gotify address is not a valid URL"))?;
    let mut body = json!({
        "title": prepared.title,
        "message": prepared.body,
        "extras": { "client::display": { "contentType": "text/plain" } },
    });
    if let Some(priority) = c.priority {
        body["priority"] = json!(priority);
    }
    let mut request = json_request(url, None, body);
    request
        .secret_headers
        .push(("X-Gotify-Key", c.token.clone()));
    Ok(request)
}

/// Lines from `lines` up to `max` bytes in total; whether any were left out.
fn bounded(lines: &[String], max: usize) -> (Vec<&str>, bool) {
    let mut used = 0;
    let mut kept = Vec::new();
    for line in lines {
        used += line.len() + 1;
        if used > max {
            return (kept, true);
        }
        kept.push(line.as_str());
    }
    (kept, false)
}

fn webhook(c: &Webhook, prepared: &Prepared, event: &Event) -> HttpRequest {
    let (added, added_cut) = bounded(&event.added, MAX_LINES_BYTES);
    let (removed, removed_cut) = bounded(&event.removed, MAX_LINES_BYTES);
    let (triggered, triggered_cut) = bounded(&event.triggered, MAX_LINES_BYTES);
    let truncated = prepared.truncated || added_cut || removed_cut || triggered_cut;
    let body = match c.format {
        WebhookFormat::Json => HttpBody::Json(json!({
            "version": 1,
            "title": prepared.title,
            "message": prepared.body,
            "watch": { "id": event.watch_id, "name": event.watch_name, "url": event.watch_url },
            "checked_at": event.checked_at_rfc3339(),
            "diff_url": event.diff_url,
            "added": added,
            "removed": removed,
            "triggered_text": triggered,
            "truncated": truncated,
        })),
        WebhookFormat::Form => {
            let field = |name: &str, value: &str| (name.to_owned(), value.to_owned());
            HttpBody::Form(vec![
                field("title", &prepared.title),
                field("message", &prepared.body),
                field("watch_id", &event.watch_id),
                field("watch_name", &event.watch_name),
                field("watch_url", &event.watch_url),
                field("checked_at", &event.checked_at_rfc3339()),
                field("diff_url", &event.diff_url),
                field("added", &added.join("\n")),
                field("removed", &removed.join("\n")),
                field("triggered_text", &triggered.join("\n")),
                field("truncated", if truncated { "true" } else { "false" }),
            ])
        }
    };
    HttpRequest {
        url: c.url.clone(),
        auth: c.auth.clone(),
        secret_headers: Vec::new(),
        body_secrets: Vec::new(),
        body,
    }
}
