//! The OpenAPI description of the API, and the two addresses that serve it.

use std::sync::Arc;

use axum::Json;
use axum::response::IntoResponse;
use serde_json::json;
use utoipa::openapi::OpenApi as Spec;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
use utoipa::{Modify, OpenApi};

/// Adds the way a token is sent to the description.
struct TokenScheme;

impl Modify for TokenScheme {
    fn modify(&self, spec: &mut Spec) {
        let components = spec.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "api_key",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::with_description(
                "x-api-key",
                "An API token made on the settings page or with `vigil token add`. Not needed \
                 while sign-in is off.",
            ))),
        );
    }
}

/// The parts of the description that are not about one endpoint.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Vigil API",
        description = "Manage watches, tags and notification targets, read history and \
                       differences, and import watches from other tools.",
        license(name = "MIT OR Apache-2.0"),
    ),
    modifiers(&TokenScheme),
    tags(
        (name = "meta", description = "About the server."),
        (name = "watches", description = "The pages that are watched."),
    )
)]
pub(super) struct ApiDoc;

/// `GET /api/docs`: where the description is.
pub(super) fn index(base_path: &str) -> impl IntoResponse + use<> {
    Json(json!({"openapi": format!("{base_path}/api/docs/openapi.json")}))
}

/// `GET /api/docs/openapi.json`: the description.
pub(super) fn spec(spec: &Arc<Spec>) -> impl IntoResponse + use<> {
    Json(Arc::clone(spec))
}
