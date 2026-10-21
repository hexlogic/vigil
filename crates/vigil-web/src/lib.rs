//! axum application: REST API and web UI.

mod assets;
mod auth;
mod config;
mod i18n;
mod page;
mod pages;
mod security;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::response::Redirect;
use axum::routing::{get, post};
use tower_http::compression::CompressionLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use vigil_store::Store;

pub use auth::{AuthError, PasswordSource, change_password, ensure_admin_password};
pub use config::{Auth, WebConfig};
pub use i18n::{I18n, Lang, LocaleError, Translator};

/// Longest a request may take. Longer than the slowest page (a preview fetches for up to 30
/// seconds and then processes for up to 30 more), so only a stuck request is cut off.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Shared state handed to every request handler.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Handle to the database.
    pub store: Store,
    /// How the UI is set up.
    pub web: Arc<WebConfig>,
    /// The texts of the UI.
    pub i18n: Arc<I18n>,
    /// Recent wrong passwords.
    limiter: Arc<auth::LoginLimiter>,
}

impl AppState {
    /// State for a server with the given settings.
    pub fn new(store: Store, web: WebConfig) -> Result<Self, LocaleError> {
        Ok(Self {
            store,
            web: Arc::new(web),
            i18n: Arc::new(I18n::load()?),
            limiter: Arc::default(),
        })
    }
}

/// Build the application router.
///
/// `/healthz` is always at the root. Everything else is under the path of the configured
/// `base_url`, if it has one, because a reverse proxy that serves Vigil under a prefix passes
/// the prefix on.
pub fn router(state: AppState) -> Router {
    // What a visitor who is not signed in may reach.
    let public = Router::new()
        .route("/login", get(pages::login_form).post(pages::login_submit))
        .route("/assets/{name}", get(assets::serve));
    // Everything else needs a session, unless sign-in is off.
    let protected = Router::new()
        .route("/", get(pages::home))
        .route("/logout", post(pages::logout))
        .route_layer(from_fn_with_state(state.clone(), auth::require_session));
    let ui = public
        .merge(protected)
        .fallback(pages::not_found)
        .layer(from_fn_with_state(state.clone(), auth::check_origin));
    let base_path = state.web.base_path();
    let app = Router::new().route("/healthz", get(healthz));
    let app = if base_path.is_empty() {
        app.merge(ui)
    } else {
        // axum nests `/` at the prefix alone; people also type the address with a slash. That
        // is sent on to the real page, so that it passes the same checks as every other.
        let canonical = base_path.clone();
        app.route(
            &format!("{base_path}/"),
            get(move || async move { Redirect::permanent(&canonical) }),
        )
        .nest(&base_path, ui)
    };
    security::add(app)
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Liveness/readiness probe: `200 ok` when the database answers, `503` otherwise.
async fn healthz(State(state): State<AppState>) -> (StatusCode, &'static str) {
    match state.store.ping().await {
        Ok(()) => (StatusCode::OK, "ok"),
        Err(err) => {
            tracing::error!(%err, "health check failed");
            (StatusCode::SERVICE_UNAVAILABLE, "database unavailable")
        }
    }
}
