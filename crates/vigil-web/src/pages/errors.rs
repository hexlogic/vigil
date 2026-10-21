//! Pages for requests that cannot be answered.

use askama::Template;
use axum::http::StatusCode;
use axum::response::Response;

use crate::page::{Ctx, render_with};

/// A page that says what went wrong.
#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    ctx: Ctx,
    title_key: &'static str,
    body_key: &'static str,
}

/// The answer to an address nothing is served at.
pub(crate) async fn not_found(ctx: Ctx) -> Response {
    render_with(
        StatusCode::NOT_FOUND,
        &ErrorPage {
            ctx,
            title_key: "error.not_found_title",
            body_key: "error.not_found_body",
        },
    )
}

/// Why a request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// It did not come from a page of this site.
    Origin,
    /// It was sent to an address that cannot be served without a password.
    Host,
}

/// The answer to a request that was refused.
pub(crate) fn forbidden(ctx: Ctx, why: Refusal) -> Response {
    let (title_key, body_key) = match why {
        Refusal::Origin => ("error.forbidden_title", "error.forbidden_body"),
        Refusal::Host => ("error.host_title", "error.host_body"),
    };
    render_with(
        StatusCode::FORBIDDEN,
        &ErrorPage {
            ctx,
            title_key,
            body_key,
        },
    )
}
