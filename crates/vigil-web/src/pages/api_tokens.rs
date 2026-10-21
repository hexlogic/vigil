//! API tokens on the settings page: the list, making one, and revoking one.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use uuid::Uuid;
use vigil_store::{ApiToken, StoreError, now_ms};

use super::settings::{Settings, page_with};
use super::watches::is_htmx;
use super::{not_found, server_error};
use crate::AppState;
use crate::api::generate_token;
use crate::config::Auth;
use crate::fmt::{self, TimeView};
use crate::form::Fields;
use crate::page::{Ctx, render, render_with};

/// Longest name a token can have.
const NAME_LEN: usize = 50;

/// One token in the list.
pub(crate) struct TokenRow {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) prefix: String,
    pub(crate) created: TimeView,
    pub(crate) used: TimeView,
    pub(crate) row_url: String,
    pub(crate) revoke_url: String,
}

impl TokenRow {
    fn new(ctx: &Ctx, token: &ApiToken, now: i64) -> Self {
        let id = token.id.to_string();
        let at = |suffix: &str| ctx.url(&format!("/settings/tokens/{id}{suffix}"));
        Self {
            name: token.name.clone(),
            prefix: token.prefix.clone(),
            created: fmt::when(ctx.translator(), Some(token.created_at), now),
            used: fmt::when(ctx.translator(), token.last_used_at, now),
            row_url: at("/row"),
            revoke_url: at("/revoke"),
            id,
        }
    }
}

/// A token that was just made, which is shown once.
pub(crate) struct CreatedToken {
    pub(crate) name: String,
    pub(crate) token: String,
}

/// What the settings page shows of the API tokens.
#[derive(Default)]
pub(crate) struct TokensView {
    pub(crate) rows: Vec<TokenRow>,
    /// The token just made.
    pub(crate) created: Option<CreatedToken>,
    /// What is wrong with the name that was typed.
    pub(crate) error: Option<String>,
    /// The name that was typed, to show again after a mistake.
    pub(crate) typed: String,
    pub(crate) create_url: String,
    pub(crate) docs_url: String,
}

impl TokensView {
    /// The list as it is now.
    pub(crate) async fn load(state: &AppState, ctx: &Ctx) -> Result<Self, StoreError> {
        let now = now_ms();
        let rows = state
            .store
            .list_api_tokens()
            .await?
            .iter()
            .map(|token| TokenRow::new(ctx, token, now))
            .collect();
        Ok(Self {
            rows,
            create_url: ctx.url("/settings/tokens"),
            docs_url: ctx.url("/api/docs/openapi.json"),
            ..Self::default()
        })
    }
}

#[derive(Template)]
#[template(path = "token_row.html")]
struct RowFragment {
    ctx: Ctx,
    row: TokenRow,
}

#[derive(Template)]
#[template(path = "token_revoke.html")]
struct RevokeFragment {
    ctx: Ctx,
    row: TokenRow,
}

/// `POST /settings/tokens`: make a token and show it, once, on the page that answers.
pub(crate) async fn create_token(
    State(state): State<AppState>,
    ctx: Ctx,
    body: String,
) -> Response {
    // With sign-in off the API takes no token, so there is nothing to make.
    if state.web.auth != Auth::Password {
        return StatusCode::NOT_FOUND.into_response();
    }
    let t = ctx.translator().clone();
    let name = Fields::parse(&body).get("name").trim().to_owned();
    let problem = if name.is_empty() {
        Some(t.t("settings.err.token_name"))
    } else if name.chars().count() > NAME_LEN {
        Some(t.t1("settings.err.token_long", "n", NAME_LEN))
    } else {
        None
    };
    let mut view = match TokensView::load(&state, &ctx).await {
        Ok(view) => view,
        Err(error) => return server_error(ctx, &error),
    };
    if let Some(problem) = problem {
        view.error = Some(problem);
        view.typed = name;
        return show(&state, ctx, StatusCode::UNPROCESSABLE_ENTITY, view);
    }
    let made = match generate_token() {
        Ok(made) => made,
        Err(error) => return server_error(ctx, &error),
    };
    match state
        .store
        .create_api_token(&name, &made.hash, &made.prefix)
        .await
    {
        Ok(_) => {}
        Err(StoreError::Conflict(_)) => {
            view.error = Some(t.t("settings.err.token_taken"));
            view.typed = name;
            return show(&state, ctx, StatusCode::UNPROCESSABLE_ENTITY, view);
        }
        Err(error) => return server_error(ctx, &error),
    }
    // The list is read again so that it holds the new token; the token itself is not kept.
    let mut view = match TokensView::load(&state, &ctx).await {
        Ok(view) => view,
        Err(error) => return server_error(ctx, &error),
    };
    view.created = Some(CreatedToken {
        name,
        token: made.token,
    });
    show(&state, ctx, StatusCode::OK, view)
}

fn show(state: &AppState, ctx: Ctx, status: StatusCode, view: TokensView) -> Response {
    let page: Settings = page_with(state, ctx, None, None, view);
    render_with(status, &page)
}

async fn row_of(state: &AppState, ctx: &Ctx, id: Uuid) -> Result<TokenRow, StoreError> {
    let token = state.store.get_api_token(id).await?;
    Ok(TokenRow::new(ctx, &token, now_ms()))
}

/// `GET /settings/tokens/{id}/row`.
pub(crate) async fn token_row(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> Response {
    let Ok(uuid) = id.parse::<Uuid>() else {
        return not_found(ctx).await;
    };
    match row_of(&state, &ctx, uuid).await {
        Ok(row) => render(&RowFragment { ctx, row }),
        Err(StoreError::NotFound(_)) => not_found(ctx).await,
        Err(error) => server_error(ctx, &error),
    }
}

/// `GET /settings/tokens/{id}/revoke`: ask first, in place of the row.
pub(crate) async fn confirm_revoke_token(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> Response {
    let Ok(uuid) = id.parse::<Uuid>() else {
        return not_found(ctx).await;
    };
    match row_of(&state, &ctx, uuid).await {
        Ok(row) => render(&RevokeFragment { ctx, row }),
        Err(StoreError::NotFound(_)) => not_found(ctx).await,
        Err(error) => server_error(ctx, &error),
    }
}

/// `POST /settings/tokens/{id}/revoke`.
pub(crate) async fn revoke_token(
    State(state): State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Ok(uuid) = id.parse::<Uuid>() else {
        return not_found(ctx).await;
    };
    match state.store.delete_api_token(uuid).await {
        // Already gone is what was asked for.
        Ok(()) | Err(StoreError::NotFound(_)) => {}
        Err(error) => return server_error(ctx, &error),
    }
    if is_htmx(&headers) {
        StatusCode::OK.into_response()
    } else {
        Redirect::to(&ctx.url("/settings?done=token_revoked")).into_response()
    }
}
