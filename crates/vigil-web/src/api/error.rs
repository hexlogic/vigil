//! The JSON error every API endpoint answers with, and the body extractor that uses it.

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;
use vigil_store::StoreError;

/// What every error answer holds.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ErrorBody {
    error: ErrorDetail,
}

/// What went wrong.
#[derive(Debug, Serialize, ToSchema)]
struct ErrorDetail {
    /// A short name for the kind of problem, such as `not_found` or `invalid`.
    code: String,
    /// A sentence for a person.
    message: String,
    /// The field of the request the problem is about, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<String>,
}

/// An error answer: a status and the JSON above.
#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    detail: ErrorDetail,
}

impl ApiError {
    /// An error with this status; the code is named after the status.
    pub(crate) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            detail: ErrorDetail {
                code: code_of(status).to_owned(),
                message: message.into(),
                field: None,
            },
        }
    }

    /// Name the kind of problem more precisely than the status does.
    pub(crate) fn code(mut self, code: &str) -> Self {
        code.clone_into(&mut self.detail.code);
        self
    }

    /// `404`: no such thing.
    pub(crate) fn not_found(what: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::NOT_FOUND, format!("{what} was not found"))
    }

    /// `401`: no valid token.
    pub(crate) fn unauthorized(message: &str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    /// `403`: refused whoever asks.
    pub(crate) fn forbidden(message: &str) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    /// `500`: our fault. The details are logged and not sent, since they may name paths or
    /// hold credentials.
    pub(crate) fn internal(error: &dyn std::fmt::Display) -> Self {
        tracing::error!(%error, "an API request failed");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "something went wrong on the server",
        )
    }
}

fn code_of(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "bad_request",
        StatusCode::UNAUTHORIZED => "unauthorized",
        StatusCode::FORBIDDEN => "forbidden",
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::METHOD_NOT_ALLOWED => "method_not_allowed",
        StatusCode::REQUEST_TIMEOUT => "timeout",
        StatusCode::CONFLICT => "conflict",
        StatusCode::PAYLOAD_TOO_LARGE => "too_large",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported_media_type",
        StatusCode::UNPROCESSABLE_ENTITY => "invalid",
        StatusCode::SERVICE_UNAVAILABLE => "unavailable",
        status if status.is_server_error() => "internal",
        _ => "error",
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody { error: self.detail };
        (self.status, axum::Json(body)).into_response()
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match &error {
            StoreError::NotFound(what) => Self::not_found(what),
            StoreError::Conflict(message) => Self::new(StatusCode::CONFLICT, message.as_str()),
            StoreError::AmbiguousId(_)
            | StoreError::AmbiguousTarget(_)
            | StoreError::InvalidId(_) => Self::new(StatusCode::BAD_REQUEST, error.to_string()),
            _ => Self::internal(&error),
        }
    }
}

/// Turn an error answer that is not JSON (a timeout, a wrong method, a bad query string) into
/// the JSON one, so a client can always read the body the same way.
pub(crate) async fn as_json(response: Response) -> Response {
    let status = response.status();
    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if (status.is_client_error() || status.is_server_error()) && !is_json {
        let allow = response.headers().get(header::ALLOW).cloned();
        let message = status.canonical_reason().unwrap_or("error").to_lowercase();
        let mut replaced = ApiError::new(status, message).into_response();
        if let Some(allow) = allow {
            replaced.headers_mut().insert(header::ALLOW, allow);
        }
        return replaced;
    }
    response
}

/// The answer to any `/api/...` path that is not an endpoint.
pub(crate) async fn no_such_endpoint() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "there is no such endpoint").code("no_such_endpoint")
}
