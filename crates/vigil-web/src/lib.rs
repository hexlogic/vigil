//! axum application: REST API and web UI.

mod api;
mod assets;
mod auth;
mod config;
mod diff_html;
mod fmt;
mod form;
mod i18n;
mod import_cache;
mod page;
mod pages;
mod preview_cache;
mod security;
mod upload;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::response::Redirect;
use axum::routing::{get, post};
use tower_http::compression::CompressionLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use vigil_scheduler::{Checker, Notifier, SchedulerHandle};
use vigil_store::Store;

pub use api::{GeneratedToken, generate_token};
pub use auth::{AuthError, PasswordSource, change_password, ensure_admin_password};
pub use config::{Auth, WebConfig};
pub use i18n::{I18n, Lang, LocaleError, Translator};

/// Longest a request may take. Longer than the slowest page (a preview fetches for up to 30
/// seconds and then processes for up to 30 more), so only a stuck request is cut off.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Longest an import may take, from the first byte of the upload to the answer.
const IMPORT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Shared state handed to every request handler.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Handle to the database.
    pub store: Store,
    /// How the UI is set up.
    pub web: Arc<WebConfig>,
    /// The texts of the UI.
    pub i18n: Arc<I18n>,
    /// Checks pages and previews watches.
    pub checker: Checker,
    /// Sends notifications; also used for the test button.
    pub notifier: Arc<Notifier>,
    /// Asks the running scheduler to check a watch or to look at the watch list.
    pub scheduler: SchedulerHandle,
    /// Recent wrong passwords.
    limiter: Arc<auth::LoginLimiter>,
    /// Takes turns for the processing slot of each form.
    slots: Arc<form::SlotLocks>,
    /// The page each open form last fetched for its preview.
    previews: Arc<preview_cache::PreviewCache>,
    /// Import previews waiting for their button.
    import_plans: Arc<import_cache::ImportPlans>,
}

impl AppState {
    /// State for a server with the given settings and services. The `checker`, `notifier` and
    /// `scheduler` handle are the ones the running scheduler uses, so that a check asked for in
    /// the UI is one the scheduler runs.
    pub fn new(
        store: Store,
        web: WebConfig,
        checker: Checker,
        notifier: Arc<Notifier>,
        scheduler: SchedulerHandle,
    ) -> Result<Self, LocaleError> {
        Ok(Self {
            store,
            web: Arc::new(web),
            i18n: Arc::new(I18n::load()?),
            checker,
            notifier,
            scheduler,
            limiter: Arc::default(),
            slots: Arc::default(),
            previews: Arc::default(),
            import_plans: Arc::default(),
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
        .route("/", get(pages::watches))
        .route("/logout", post(pages::logout))
        .route("/watch/new", get(pages::new_form).post(pages::create))
        .route("/watch/steps", post(pages::steps))
        .route("/watch/preview", post(pages::preview))
        .route(
            "/watch/{id}/edit",
            get(pages::edit_form).post(pages::update),
        )
        .route("/settings", get(pages::settings))
        .route("/settings/language", post(pages::set_language))
        .route("/settings/password", post(pages::set_password))
        .route("/settings/tokens", post(pages::create_token))
        .route("/settings/tokens/{id}/row", get(pages::token_row))
        .route(
            "/settings/tokens/{id}/revoke",
            get(pages::confirm_revoke_token).post(pages::revoke_token),
        )
        .route(
            "/settings/sign-out-everywhere",
            post(pages::sign_out_everywhere),
        )
        .route("/tags", get(pages::tags))
        .route(
            "/tags/new",
            get(pages::new_tag_form).post(pages::create_tag),
        )
        .route(
            "/tags/{id}/edit",
            get(pages::edit_tag_form).post(pages::update_tag),
        )
        .route("/tags/{id}/row", get(pages::tag_row))
        .route(
            "/tags/{id}/delete",
            get(pages::confirm_delete_tag).post(pages::delete_tag),
        )
        .route("/notifications", get(pages::targets))
        .route(
            "/notifications/new",
            get(pages::new_target_form).post(pages::create_target),
        )
        .route(
            "/notifications/{id}/edit",
            get(pages::edit_target_form).post(pages::update_target),
        )
        .route("/notifications/{id}/row", get(pages::target_row))
        .route("/notifications/{id}/test", post(pages::test_target))
        .route("/notifications/{id}/enable", post(pages::enable_target))
        .route("/notifications/{id}/disable", post(pages::disable_target))
        .route(
            "/notifications/{id}/delete",
            get(pages::confirm_delete_target).post(pages::delete_target),
        )
        .route("/watch/{id}", get(pages::show))
        .route("/watch/{id}/history", get(pages::history))
        .route("/watch/{id}/history/{snapshot}", get(pages::snapshot))
        .route("/watch/{id}/diff", get(pages::diff))
        .route("/watch/{id}/row", get(pages::watch_row))
        .route("/watch/{id}/check", post(pages::check_watch))
        .route("/watch/{id}/pause", post(pages::pause_watch))
        .route("/watch/{id}/resume", post(pages::resume_watch))
        .route(
            "/watch/{id}/delete",
            get(pages::confirm_delete).post(pages::delete_watch),
        )
        .route_layer(from_fn_with_state(state.clone(), auth::require_session));
    // The import takes a file of up to hundreds of megabytes, so its pages get a longer
    // timeout than the others, which an outer layer could not give them.
    let imports = Router::new()
        .route(
            "/import",
            get(pages::import_form)
                .post(pages::import_preview)
                .layer(DefaultBodyLimit::max(upload_limit(&state))),
        )
        .route("/import/apply", post(pages::import_apply))
        .route_layer(from_fn_with_state(state.clone(), auth::require_session))
        .layer(timeout(IMPORT_TIMEOUT));
    let ui = public
        .merge(protected)
        .layer(timeout(REQUEST_TIMEOUT))
        .merge(imports)
        .fallback(pages::not_found)
        .layer(from_fn_with_state(state.clone(), auth::check_origin))
        .with_state(state.clone());
    // The API has its own checks (a token, not a cookie), so it is outside the UI's. Whatever
    // is not an API path is the UI's.
    let site =
        api::router(&state, timeout(REQUEST_TIMEOUT), timeout(IMPORT_TIMEOUT)).fallback_service(ui);
    let base_path = state.web.base_path();
    let app = Router::new().route("/healthz", get(healthz).layer(timeout(REQUEST_TIMEOUT)));
    let app = if base_path.is_empty() {
        app.merge(site)
    } else {
        // axum nests `/` at the prefix alone; people also type the address with a slash. That
        // is sent on to the real page, so that it passes the same checks as every other.
        let canonical = base_path.clone();
        app.route(
            &format!("{base_path}/"),
            get(move || async move { Redirect::permanent(&canonical) }),
        )
        .nest(&base_path, site)
    };
    security::add(app)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// The most bytes the import form accepts: the file, and a little for the other fields.
fn upload_limit(state: &AppState) -> usize {
    usize::try_from(state.web.max_upload_bytes)
        .unwrap_or(usize::MAX)
        .saturating_add(64 * 1024)
}

/// Cuts off a request that hangs. It is applied to the UI and to the API separately, because
/// an outer layer cannot be loosened for one route inside it, and the import takes longer.
fn timeout(limit: Duration) -> TimeoutLayer {
    TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, limit)
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
