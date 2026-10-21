//! Who may call the API.
//!
//! With sign-in on, a call needs a token in the `x-api-key` header; a cookie never counts, so
//! a page on another site cannot make a browser call the API. With sign-in off there is no
//! token to ask for, so the only protection is where the request comes from: the address it
//! was sent to must be one an attacker's name cannot be made to look like, and a browser
//! must not be calling from another site.

use axum::extract::{Request, State};
use axum::http::header::{HOST, ORIGIN};
use axum::http::{HeaderMap, HeaderName};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use vigil_store::now_ms;

use super::error::ApiError;
use super::token::{hash_token, looks_like_token};
use crate::AppState;
use crate::auth::{host_is_trusted, origin_matches};
use crate::config::{Auth, WebConfig};

/// The header a token is sent in.
const API_KEY: HeaderName = HeaderName::from_static("x-api-key");

/// With sign-in off, refuse requests that a person on this machine or network did not make:
/// an address that could be a DNS-rebinding name, or a page from another site.
pub(crate) async fn open_door(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(error) = refusal(&state.web, request.headers()) {
        return error.into_response();
    }
    next.run(request).await
}

/// Why a request to an API with no token must be turned away, if it must.
fn refusal(web: &WebConfig, headers: &HeaderMap) -> Option<ApiError> {
    if web.auth != Auth::Disabled {
        return None;
    }
    let host = headers.get(HOST).and_then(|v| v.to_str().ok());
    if !host.is_some_and(|host| host_is_trusted(web, host)) {
        return Some(ApiError::forbidden(
            "while sign-in is off, the server answers only requests addressed to an IP address, localhost or its configured base_url",
        ));
    }
    // A client such as curl sends neither header, which is fine. A browser always says where
    // the request comes from, and `same-site` is another port or subdomain: not this site.
    let foreign_origin = headers
        .get(ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|origin| !origin_matches(web, origin, host));
    let foreign_site = headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|site| matches!(site, "cross-site" | "same-site"));
    (foreign_origin || foreign_site).then(|| {
        ApiError::forbidden("the API does not answer requests made by pages of other sites")
    })
}

/// Let a request through when sign-in is off or it carries a token that was made here.
pub(crate) async fn require_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if state.web.auth == Auth::Disabled {
        return next.run(request).await;
    }
    let Some(token) = request.headers().get(API_KEY).and_then(|v| v.to_str().ok()) else {
        return ApiError::unauthorized("send an API token in the x-api-key header").into_response();
    };
    if !looks_like_token(token) {
        return ApiError::unauthorized("the API token is not valid").into_response();
    }
    match state
        .store
        .use_api_token(&hash_token(token), now_ms())
        .await
    {
        Ok(Some(_)) => next.run(request).await,
        Ok(None) => ApiError::unauthorized("the API token is not valid").into_response(),
        Err(error) => ApiError::internal(&error).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderMap;

    use super::refusal;
    use crate::config::{Auth, WebConfig};

    fn headers(pairs: &[(&'static str, &str)]) -> Result<HeaderMap, Box<dyn std::error::Error>> {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, value.parse()?);
        }
        Ok(map)
    }

    #[test]
    fn with_sign_in_on_the_door_has_nothing_to_say() -> Result<(), Box<dyn std::error::Error>> {
        let web = WebConfig::new(Auth::Password, None);
        let evil = headers(&[
            ("host", "attacker.example"),
            ("origin", "http://evil.example"),
        ])?;
        assert!(refusal(&web, &evil).is_none());
        Ok(())
    }

    #[test]
    fn with_sign_in_off_only_local_addresses_and_this_site_get_in()
    -> Result<(), Box<dyn std::error::Error>> {
        let web = WebConfig::new(Auth::Disabled, None);
        let ok = |pairs: &[(&'static str, &str)]| -> Result<bool, Box<dyn std::error::Error>> {
            Ok(refusal(&web, &headers(pairs)?).is_none())
        };
        assert!(ok(&[("host", "localhost:5800")])?);
        assert!(ok(&[("host", "192.168.1.5:5800")])?);
        assert!(!ok(&[("host", "attacker.example")])?);
        assert!(!ok(&[])?);
        // Neither header: a command-line client.
        assert!(ok(&[("host", "127.0.0.1:5800")])?);
        // The page that is served from this address.
        assert!(ok(&[
            ("host", "127.0.0.1:5800"),
            ("origin", "http://127.0.0.1:5800")
        ])?);
        assert!(ok(&[
            ("host", "127.0.0.1:5800"),
            ("sec-fetch-site", "none")
        ])?);
        // Pages of other sites, including a text/plain POST that needs no CORS preflight.
        assert!(!ok(&[
            ("host", "127.0.0.1:5800"),
            ("origin", "http://evil.example")
        ])?);
        assert!(!ok(&[
            ("host", "127.0.0.1:5800"),
            ("sec-fetch-site", "cross-site")
        ])?);
        assert!(!ok(&[
            ("host", "127.0.0.1:5800"),
            ("sec-fetch-site", "same-site")
        ])?);
        Ok(())
    }
}
