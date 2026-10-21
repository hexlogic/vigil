//! The REST API under `/api/v1`, described by an OpenAPI document at `/api/docs/openapi.json`.
//!
//! Every endpoint is registered together with its description (`routes!` with a
//! `#[utoipa::path]` on the handler), so an endpoint cannot exist without being documented.

mod docs;
mod error;
mod guard;
mod history;
mod tags;
mod targets;
mod token;
mod watch_json;
mod watches;

use std::sync::Arc;

use axum::Json;
use axum::extract::DefaultBodyLimit;
use axum::middleware::{from_fn_with_state, map_response};
use axum::routing::{any, get};
use serde::Serialize;
use tower_http::timeout::TimeoutLayer;
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use self::error::{ErrorBody, as_json, no_such_endpoint};
use crate::{AppState, config::Auth};

pub use token::{GeneratedToken, generate_token};

/// Largest JSON request body.
const JSON_LIMIT: usize = 1024 * 1024;

/// Whether the API wants a token.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum Access {
    /// Send an API token in the `x-api-key` header.
    Token,
    /// Sign-in is off, so the API takes any request from this machine or network.
    Open,
}

/// What the server says about itself.
#[derive(Debug, Serialize, ToSchema)]
struct Version {
    /// The version of Vigil.
    version: &'static str,
    /// How the API is protected. A client can call this endpoint to check its token.
    access: Access,
}

/// Which version of Vigil is running, and whether the API asks for a token.
#[utoipa::path(
    get,
    path = "/version",
    tag = "meta",
    security(("api_key" = [])),
    responses(
        (status = 200, description = "The server's version", body = Version),
        (status = 401, description = "No valid token", body = ErrorBody),
    )
)]
async fn version(axum::extract::State(state): axum::extract::State<AppState>) -> Json<Version> {
    Json(Version {
        version: env!("CARGO_PKG_VERSION"),
        access: match state.web.auth {
            Auth::Password => Access::Token,
            Auth::Disabled => Access::Open,
        },
    })
}

/// The API's routes, with the checks every request goes through. Anything else under `/api`
/// is answered with a JSON error, so a client never gets the HTML 404 page.
pub(crate) fn router(state: &AppState, timeout: TimeoutLayer) -> axum::Router<AppState> {
    let (routes, mut spec) = OpenApiRouter::with_openapi(docs::ApiDoc::openapi())
        .nest(
            "/api/v1",
            OpenApiRouter::new()
                .routes(routes!(version))
                .routes(routes!(watches::list_watches, watches::create_watch))
                .routes(routes!(
                    watches::get_watch,
                    watches::replace_watch,
                    watches::delete_watch
                ))
                .routes(routes!(watches::check_watch))
                .routes(routes!(history::list_history))
                .routes(routes!(history::get_snapshot))
                .routes(routes!(history::get_diff))
                .routes(routes!(tags::list_tags, tags::create_tag))
                .routes(routes!(tags::get_tag, tags::replace_tag, tags::delete_tag))
                .routes(routes!(targets::list_targets, targets::create_target))
                .routes(routes!(
                    targets::get_target,
                    targets::replace_target,
                    targets::delete_target
                ))
                .routes(routes!(targets::test_target)),
        )
        .split_for_parts();
    let base_path = state.web.base_path();
    if !base_path.is_empty() {
        spec.servers = Some(vec![utoipa::openapi::Server::new(base_path.clone())]);
    }
    let spec = Arc::new(spec);
    let pointer = base_path.clone();

    routes
        // Only the endpoints need a token; the description and the 404s are open.
        .route_layer(from_fn_with_state(state.clone(), guard::require_token))
        .route(
            "/api/docs",
            get(move || async move { docs::index(&pointer) }),
        )
        .route(
            "/api/docs/openapi.json",
            get(move || async move { docs::spec(&spec) }),
        )
        .route("/api/{*rest}", any(no_such_endpoint))
        .layer(DefaultBodyLimit::max(JSON_LIMIT))
        .layer(from_fn_with_state(state.clone(), guard::open_door))
        .layer(timeout)
        .layer(map_response(as_json))
}
