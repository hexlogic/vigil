//! Sending a prepared message.

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{CONTENT_TYPE, HeaderValue, RETRY_AFTER};

use crate::channel::{Channel, HttpAuth};
use crate::error::SendError;
use crate::event::Event;
use crate::message::Prepared;
use crate::request::{Endpoints, HttpBody, HttpRequest, http_request};
use crate::smtp::send_email;

/// Longest piece of a service's error reply put in an error message, in characters.
const MAX_REPLY_CHARS: usize = 200;

/// How a [`Sender`] behaves.
#[derive(Debug, Clone)]
pub struct SenderConfig {
    /// `User-Agent` header.
    pub user_agent: String,
    /// How long one attempt may take, connecting and answering included.
    pub timeout: Duration,
    /// Where the services with a fixed address are reached.
    pub endpoints: Endpoints,
}

impl Default for SenderConfig {
    fn default() -> Self {
        Self {
            user_agent: concat!("vigil/", env!("CARGO_PKG_VERSION")).into(),
            timeout: Duration::from_secs(15),
            endpoints: Endpoints::default(),
        }
    }
}

/// Delivers messages. Cheap to clone; share one.
#[derive(Debug, Clone)]
pub struct Sender {
    client: reqwest::Client,
    endpoints: Endpoints,
    timeout: Duration,
}

impl Sender {
    /// Build a sender.
    ///
    /// It follows no redirects (a redirect could carry a token to another host) and ignores
    /// proxy environment variables, like the page fetcher.
    pub fn new(config: SenderConfig) -> Result<Self, SendError> {
        let client = reqwest::Client::builder()
            .user_agent(&config.user_agent)
            .timeout(config.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|e| SendError::permanent(describe(&e.without_url())))?;
        Ok(Self {
            client,
            endpoints: config.endpoints,
            timeout: config.timeout,
        })
    }

    /// Make one attempt to deliver `prepared` to `channel`.
    pub async fn send(
        &self,
        channel: &Channel,
        prepared: &Prepared,
        event: &Event,
    ) -> Result<(), SendError> {
        if let Channel::Email(email) = channel {
            return send_email(email, prepared, self.timeout).await;
        }
        match http_request(channel, prepared, event, &self.endpoints)? {
            Some(request) => self.post(&request).await,
            None => Err(SendError::permanent("this channel has nothing to send")),
        }
    }

    async fn post(&self, request: &HttpRequest) -> Result<(), SendError> {
        let secrets = request.secrets();
        let scrub = |text: String| scrub(text, &secrets);

        let mut builder = self.client.post(request.url.expose().clone());
        match &request.auth {
            Some(HttpAuth::Basic { user, password }) => {
                builder = builder.basic_auth(user, Some(password.expose()));
            }
            Some(HttpAuth::Bearer(token)) => builder = builder.bearer_auth(token.expose()),
            None => {}
        }
        for (name, secret) in &request.secret_headers {
            let mut value = HeaderValue::from_str(secret.expose())
                .map_err(|_| SendError::permanent(format!("the {name} value is not valid")))?;
            value.set_sensitive(true);
            builder = builder.header(*name, value);
        }
        builder = match &request.body {
            HttpBody::Json(value) => builder.header(CONTENT_TYPE, "application/json").body(
                serde_json::to_vec(value)
                    .map_err(|_| SendError::permanent("the message could not be encoded"))?,
            ),
            HttpBody::Form(fields) => builder
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(
                    url::form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(fields)
                        .finish(),
                ),
        };

        let mut response = builder
            .send()
            .await
            .map_err(|e| from_reqwest(e).map_message(&scrub))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        // One chunk is enough to see what the service complained about.
        let reply = match response.chunk().await {
            Ok(Some(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
            _ => String::new(),
        };
        let reply: String = scrub(reply)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(MAX_REPLY_CHARS)
            .collect();
        let message = if reply.is_empty() {
            format!("HTTP {status}")
        } else {
            format!("HTTP {status}: {reply}")
        };
        Err(SendError {
            message,
            transient: is_transient(status),
            retry_after,
        })
    }
}

impl SendError {
    fn map_message(mut self, f: &impl Fn(String) -> String) -> Self {
        self.message = f(self.message);
        self
    }
}

/// Whether a status is worth another attempt later.
fn is_transient(status: StatusCode) -> bool {
    status.is_server_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
}

fn from_reqwest(error: reqwest::Error) -> SendError {
    // The URL of a request holds the credentials of most services: never keep it.
    let error = error.without_url();
    let message = if error.is_timeout() {
        "timed out".to_owned()
    } else {
        describe(&error)
    };
    SendError {
        message,
        transient: !error.is_builder(),
        retry_after: None,
    }
}

/// An error and its causes on one line.
fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

/// Replace every occurrence of a secret in `text` with `***`.
fn scrub(mut text: String, secrets: &[String]) -> String {
    for secret in secrets {
        text = text.replace(secret.as_str(), "***");
    }
    text
}
