//! Headers that tell the browser how little this site is allowed to do.

use axum::Router;
use axum::http::HeaderValue;
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, HeaderName, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
    X_FRAME_OPTIONS,
};
use tower_http::set_header::SetResponseHeaderLayer;

/// Everything is loaded from this site: no inline script or style, no plugins, no embedding
/// in another page, forms post only here. htmx is configured (see the layout) not to inject
/// styles or evaluate code, which this policy would refuse.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
                   object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// Add the security headers to every response that does not set its own.
pub(crate) fn add<S: Clone + Send + Sync + 'static>(router: Router<S>) -> Router<S> {
    let header = |name: HeaderName, value: &'static str| {
        SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
    };
    router
        .layer(header(CONTENT_SECURITY_POLICY, CSP))
        .layer(header(X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(header(X_FRAME_OPTIONS, "DENY"))
        .layer(header(REFERRER_POLICY, "same-origin"))
        .layer(header(
            HeaderName::from_static("cross-origin-opener-policy"),
            "same-origin",
        ))
        .layer(header(
            HeaderName::from_static("permissions-policy"),
            "camera=(), microphone=(), geolocation=()",
        ))
        // Pages show private data and change after every action; assets set their own.
        .layer(header(CACHE_CONTROL, "no-store"))
}
