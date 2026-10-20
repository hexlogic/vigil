//! axum application: REST API and web UI.

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use tower_http::trace::TraceLayer;
use vigil_store::Store;

/// Shared state handed to every request handler.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Handle to the database.
    pub store: Store,
}

/// Build the application router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
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
