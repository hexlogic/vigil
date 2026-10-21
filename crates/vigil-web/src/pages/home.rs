//! The watch list.

use askama::Template;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::AppState;
use crate::page::{Ctx, Nav, render};

#[derive(Template)]
#[template(path = "home.html")]
struct Home {
    ctx: Ctx,
    count: usize,
}

/// `GET /`.
pub(crate) async fn home(State(state): State<AppState>, ctx: Ctx) -> Response {
    let count = match state.store.list_watches().await {
        Ok(watches) => watches.len(),
        Err(error) => {
            tracing::error!(%error, "listing watches failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    render(&Home {
        ctx: ctx.nav(Nav::Watches),
        count,
    })
}
