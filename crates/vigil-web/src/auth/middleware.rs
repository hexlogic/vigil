//! The checks every request goes through: is it from this site, and is it signed in.

use axum::extract::{Request, State};
use axum::http::header::{HOST, ORIGIN};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use std::net::IpAddr;
use vigil_store::{StoreError, now_ms};

use super::session::{self, LIFETIME_MS, TOUCH_AFTER_MS};
use crate::AppState;
use crate::config::{Auth, WebConfig};
use crate::page::Ctx;
use crate::pages::{Refusal, forbidden};

/// The hash of the session a request was made with, for handlers that end or change it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SessionKey(pub(crate) [u8; 32]);

/// Let a request through only if it is signed in (or sign-in is off); otherwise send the
/// browser to the sign-in page, and back afterwards.
pub(crate) async fn require_session(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    if state.web.auth == Auth::Disabled {
        return next.run(request).await;
    }
    match current_session(&state, request.headers()).await {
        Ok(Some(key)) => {
            request.extensions_mut().insert(SessionKey(key));
            next.run(request).await
        }
        Ok(None) => signed_out(&state.web, &request),
        Err(error) => {
            tracing::error!(%error, "checking the session failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// The session the request carries, if it is a live one. A session in use is renewed, at most
/// once an hour.
pub(crate) async fn current_session(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<[u8; 32]>, StoreError> {
    let Some(token) = session::token_of(headers) else {
        return Ok(None);
    };
    let key = session::hash_token(token);
    let now = now_ms();
    let Some(found) = state.store.session(&key, now).await? else {
        return Ok(None);
    };
    if now - found.last_seen_at > TOUCH_AFTER_MS {
        state
            .store
            .touch_session(&key, now, now + LIFETIME_MS)
            .await?;
    }
    Ok(Some(key))
}

/// What to answer a request that is not signed in. A page goes to the sign-in page, which
/// brings it back; a request htmx made is told to go there, since a redirect would be
/// followed by the request itself and swapped into the page.
fn signed_out(web: &WebConfig, request: &Request) -> Response {
    let htmx = request.headers().contains_key("hx-request");
    let next = if htmx {
        // The page the request was made from; the request itself may be a fragment of it.
        request
            .headers()
            .get("hx-current-url")
            .and_then(|value| value.to_str().ok())
            .and_then(path_of)
    } else if request.method() == Method::GET {
        request
            .uri()
            .path_and_query()
            .map(|p| p.as_str().to_owned())
    } else {
        None
    };
    let to = login_url(&web.base_path(), next.as_deref());
    if htmx {
        let mut response = StatusCode::UNAUTHORIZED.into_response();
        if let Ok(value) = to.parse() {
            response.headers_mut().insert("hx-redirect", value);
        }
        return response;
    }
    Redirect::to(&to).into_response()
}

/// The path and query of an absolute URL: `/a?b` of `https://example.com/a?b`.
fn path_of(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let at = rest.find('/')?;
    Some(rest[at..].split('#').next().unwrap_or_default().to_owned())
}

/// The address of the sign-in page, asking to come back to `next`.
pub(crate) fn login_url(base_path: &str, next: Option<&str>) -> String {
    // The home page is where signing in goes anyway.
    let is_home = |next: &str| next == "/" || next.trim_end_matches('/') == base_path;
    match next
        .filter(|next| !is_home(next))
        .and_then(|next| safe_next(base_path, next))
    {
        Some(next) => format!(
            "{base_path}/login?next={}",
            url::form_urlencoded::byte_serialize(next.as_bytes()).collect::<String>()
        ),
        None => format!("{base_path}/login"),
    }
}

/// `next` if it is a place on this site to go back to after signing in, and not an address on
/// another site (`//evil.example`, `https://evil.example`) or the sign-in page itself.
pub(crate) fn safe_next(base_path: &str, next: &str) -> Option<String> {
    let inside = if base_path.is_empty() {
        next.starts_with('/')
    } else {
        next == base_path
            || ["/", "?"]
                .iter()
                .any(|sep| next.starts_with(&format!("{base_path}{sep}")))
    };
    let path = next.split(['?', '#']).next().unwrap_or_default();
    let is_login = path.trim_end_matches('/') == format!("{base_path}/login");
    let suspicious =
        next.starts_with("//") || next.contains('\\') || next.chars().any(char::is_control);
    (inside && !suspicious && !is_login).then(|| next.to_owned())
}

/// Refuse requests that did not come from this site: a form on another page that posts here
/// must not act with the visitor's session, and a page that reaches this server through a
/// name of its own (DNS rebinding) must not read it when there is no password.
pub(crate) async fn check_origin(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    match refusal(&state.web, request.method(), request.headers()) {
        None => next.run(request).await,
        Some(why) => {
            let (parts, _) = request.into_parts();
            let ctx = Ctx::from_parts(&state, &parts.headers);
            forbidden(ctx, why)
        }
    }
}

/// Why a request must be refused, if it must.
pub(crate) fn refusal(web: &WebConfig, method: &Method, headers: &HeaderMap) -> Option<Refusal> {
    let host = headers.get(HOST).and_then(|v| v.to_str().ok());
    // Without a password the address in the request is all there is to go by: only an IP
    // address, `localhost`, or the configured address is something an attacker's name cannot
    // be made to look like.
    if web.auth == Auth::Disabled && !host.is_some_and(|host| host_is_trusted(web, host)) {
        return Some(Refusal::Host);
    }
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return None;
    }
    let same_site = match headers.get(ORIGIN).and_then(|v| v.to_str().ok()) {
        Some(origin) => origin_matches(web, origin, host),
        // Browsers send `Origin` with every form post; those that do not say so otherwise.
        None => headers
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|site| matches!(site, "same-origin" | "none")),
    };
    (!same_site).then_some(Refusal::Origin)
}

/// Whether `origin` is the configured address of this site, or the one the request was sent to.
fn origin_matches(web: &WebConfig, origin: &str, host: Option<&str>) -> bool {
    let origin = origin.trim_end_matches('/');
    if web
        .origin()
        .is_some_and(|configured| configured.eq_ignore_ascii_case(origin))
    {
        return true;
    }
    let authority = origin.split_once("://").map(|(_, rest)| rest);
    matches!((authority, host), (Some(a), Some(h)) if a.eq_ignore_ascii_case(h))
}

/// Whether a `Host` header names a place that is safe to serve without a password.
fn host_is_trusted(web: &WebConfig, host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => host.rsplit_once(':').map_or(host, |(name, port)| {
            if port.bytes().all(|b| b.is_ascii_digit()) {
                name
            } else {
                host
            }
        }),
    };
    if name.parse::<IpAddr>().is_ok()
        || name.eq_ignore_ascii_case("localhost")
        || name.to_ascii_lowercase().ends_with(".localhost")
    {
        return true;
    }
    web.origin()
        .and_then(|origin| origin.split_once("://").map(|(_, rest)| rest.to_owned()))
        .is_some_and(|authority| authority.eq_ignore_ascii_case(host))
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).expect("name"),
                HeaderValue::from_str(value).expect("value"),
            );
        }
        map
    }

    fn password(base_url: Option<&str>) -> WebConfig {
        WebConfig::new(Auth::Password, base_url)
    }

    #[test]
    fn next_must_stay_on_this_site() {
        for ok in ["/", "/watch/1", "/watch/1?x=1", "/a b"] {
            assert_eq!(safe_next("", ok).as_deref(), Some(ok), "{ok}");
        }
        for bad in [
            "//evil.example",
            "//evil.example/x",
            "https://evil.example",
            "http://evil.example/",
            "evil.example",
            "/\\evil.example",
            "/ok\r\nSet-Cookie: x=1",
            "",
            "/login",
            "/login?next=/",
            "/login/",
        ] {
            assert_eq!(safe_next("", bad), None, "{bad:?}");
        }
    }

    #[test]
    fn next_must_stay_under_the_base_path() {
        assert_eq!(safe_next("/vigil", "/vigil").as_deref(), Some("/vigil"));
        assert_eq!(
            safe_next("/vigil", "/vigil/watch/1").as_deref(),
            Some("/vigil/watch/1")
        );
        assert_eq!(
            safe_next("/vigil", "/vigil?a=1").as_deref(),
            Some("/vigil?a=1")
        );
        for bad in ["/", "/other", "/vigilant", "//evil.example", "/vigil/login"] {
            assert_eq!(safe_next("/vigil", bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_path_of_an_absolute_url_is_what_follows_the_host() {
        assert_eq!(
            path_of("http://localhost:5800/watch/1?a=b#x").as_deref(),
            Some("/watch/1?a=b")
        );
        assert_eq!(path_of("https://example.com/").as_deref(), Some("/"));
        assert_eq!(path_of("https://example.com"), None);
        assert_eq!(path_of("/relative"), None);
    }

    #[test]
    fn the_login_url_carries_the_page_to_come_back_to() {
        assert_eq!(login_url("", None), "/login");
        assert_eq!(login_url("", Some("/")), "/login");
        assert_eq!(login_url("/vigil", Some("/vigil")), "/vigil/login");
        assert_eq!(login_url("/vigil", Some("/vigil/")), "/vigil/login");
        assert_eq!(
            login_url("", Some("/watch/1?a=b")),
            "/login?next=%2Fwatch%2F1%3Fa%3Db"
        );
        assert_eq!(
            login_url("/vigil", Some("/vigil/tags")),
            "/vigil/login?next=%2Fvigil%2Ftags"
        );
        assert_eq!(login_url("", Some("//evil.example")), "/login");
    }

    #[test]
    fn a_post_needs_an_origin_that_is_this_site() {
        let web = password(None);
        let post = Method::POST;
        let from = |origin: &str| headers(&[("host", "vigil.lan:5800"), ("origin", origin)]);
        assert_eq!(refusal(&web, &post, &from("http://vigil.lan:5800")), None);
        assert_eq!(refusal(&web, &post, &from("http://VIGIL.lan:5800")), None);
        assert!(refusal(&web, &post, &from("http://evil.example")).is_some());
        assert!(refusal(&web, &post, &from("null")).is_some());
        assert!(refusal(&web, &post, &from("http://vigil.lan:5800.evil.example")).is_some());
        // Reading is not refused.
        assert_eq!(
            refusal(&web, &Method::GET, &from("http://evil.example")),
            None
        );
    }

    #[test]
    fn without_an_origin_the_browser_must_say_the_post_is_from_this_site() {
        let web = password(None);
        let post = Method::POST;
        let sent = |site: &str| headers(&[("host", "vigil.lan"), ("sec-fetch-site", site)]);
        assert_eq!(refusal(&web, &post, &sent("same-origin")), None);
        assert_eq!(refusal(&web, &post, &sent("none")), None);
        assert!(refusal(&web, &post, &sent("cross-site")).is_some());
        assert!(refusal(&web, &post, &sent("same-site")).is_some());
        assert!(refusal(&web, &post, &headers(&[("host", "vigil.lan")])).is_some());
    }

    #[test]
    fn the_configured_address_is_accepted_even_behind_a_proxy() {
        let web = password(Some("https://vigil.example.com/vigil"));
        // The proxy passes on a host of its own.
        let proxied = headers(&[
            ("host", "10.0.0.5:5800"),
            ("origin", "https://vigil.example.com"),
        ]);
        assert_eq!(refusal(&web, &Method::POST, &proxied), None);
        let other = headers(&[
            ("host", "10.0.0.5:5800"),
            ("origin", "https://evil.example"),
        ]);
        assert!(refusal(&web, &Method::POST, &other).is_some());
    }

    #[test]
    fn without_a_password_only_addresses_that_cannot_be_rebound_are_served() {
        let web = WebConfig::new(Auth::Disabled, None);
        for ok in [
            "127.0.0.1:5800",
            "192.168.1.5",
            "[::1]:5800",
            "localhost:5800",
            "vigil.localhost",
        ] {
            assert_eq!(
                refusal(&web, &Method::GET, &headers(&[("host", ok)])),
                None,
                "{ok}"
            );
        }
        for bad in ["evil.example", "evil.example:5800", "nas.lan"] {
            assert_eq!(
                refusal(&web, &Method::GET, &headers(&[("host", bad)])),
                Some(Refusal::Host),
                "{bad}"
            );
        }
        assert_eq!(
            refusal(&web, &Method::GET, &HeaderMap::new()),
            Some(Refusal::Host)
        );

        // A name is fine once the operator has said it is theirs.
        let named = WebConfig::new(Auth::Disabled, Some("http://nas.lan:5800"));
        assert_eq!(
            refusal(&named, &Method::GET, &headers(&[("host", "nas.lan:5800")])),
            None
        );
        assert!(refusal(&named, &Method::GET, &headers(&[("host", "evil.example")])).is_some());
    }

    #[test]
    fn a_cross_site_post_is_refused_without_a_password_too() {
        let web = WebConfig::new(Auth::Disabled, None);
        let cross = headers(&[
            ("host", "192.168.1.5:5800"),
            ("origin", "https://evil.example"),
        ]);
        assert_eq!(refusal(&web, &Method::POST, &cross), Some(Refusal::Origin));
        let same = headers(&[
            ("host", "192.168.1.5:5800"),
            ("origin", "http://192.168.1.5:5800"),
        ]);
        assert_eq!(refusal(&web, &Method::POST, &same), None);
    }
}
