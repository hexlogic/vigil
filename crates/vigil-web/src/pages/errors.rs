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
