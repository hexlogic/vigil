//! Tags: the list, and what can be done to one from its row.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use uuid::Uuid;
use vigil_store::{StoreError, Tag};

use super::watches::is_htmx;
use super::{not_found, server_error, tag_class};
use crate::AppState;
use crate::fmt;
use crate::page::{Ctx, Nav, render};

/// One tag in the list.
pub(crate) struct TagRow {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) class: &'static str,
    pub(crate) interval: Option<String>,
    pub(crate) watches: String,
    pub(crate) targets: Vec<String>,
    pub(crate) row_url: String,
    pub(crate) edit_url: String,
    pub(crate) delete_url: String,
    pub(crate) filter_url: String,
}

impl TagRow {
    fn new(ctx: &Ctx, tag: &Tag, watches: u32, targets: Vec<String>) -> Self {
        let id = tag.id.to_string();
        let at = |suffix: &str| ctx.url(&format!("/tags/{id}{suffix}"));
        Self {
            name: tag.name.clone(),
            class: tag_class(&tag.color),
            interval: tag.default_interval_secs.map(fmt::every),
            watches: ctx.plural("targets.watches", watches as usize),
            targets,
            row_url: at("/row"),
            edit_url: at("/edit"),
            delete_url: at("/delete"),
            filter_url: ctx.url(&format!("/?tag={id}")),
            id,
        }
    }
}

#[derive(Template)]
#[template(path = "tags.html")]
struct Tags {
    ctx: Ctx,
    rows: Vec<TagRow>,
    add_url: String,
}

#[derive(Template)]
#[template(path = "tag_row.html")]
struct RowFragment {
    ctx: Ctx,
    row: TagRow,
}

#[derive(Template)]
#[template(path = "tag_delete.html")]
struct DeleteFragment {
    ctx: Ctx,
    row: TagRow,
}

async fn row_of(state: &AppState, ctx: &Ctx, id: Uuid) -> Result<TagRow, StoreError> {
    let tag = state.store.get_tag(id).await?;
    let counts = state.store.tag_watch_counts().await?;
    let names: HashMap<_, _> = state
        .store
        .list_targets()
        .await?
        .into_iter()
        .map(|target| (target.id, target.name))
        .collect();
    let mut targets: Vec<String> = state
        .store
        .tag_targets(id)
        .await?
        .into_iter()
        .filter_map(|target| names.get(&target).cloned())
        .collect();
    targets.sort();
    Ok(TagRow::new(
        ctx,
        &tag,
        counts.get(&id).copied().unwrap_or(0),
        targets,
    ))
}

/// `GET /tags`.
pub(crate) async fn tags(State(state): State<AppState>, ctx: Ctx) -> Response {
    let tags = match state.store.list_tags().await {
        Ok(tags) => tags,
        Err(error) => return server_error(ctx, &error),
    };
    let mut rows = Vec::new();
    for tag in &tags {
        match row_of(&state, &ctx, tag.id).await {
            Ok(row) => rows.push(row),
            Err(error) => return server_error(ctx, &error),
        }
    }
    render(&Tags {
        add_url: ctx.url("/tags/new"),
        ctx: ctx.nav(Nav::Tags),
        rows,
    })
}

/// `GET /tags/{id}/row`.
pub(crate) async fn tag_row(
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

/// `GET /tags/{id}/delete`: ask first, in place of the row.
pub(crate) async fn confirm_delete_tag(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> Response {
    let Ok(uuid) = id.parse::<Uuid>() else {
        return not_found(ctx).await;
    };
    match row_of(&state, &ctx, uuid).await {
        Ok(row) => render(&DeleteFragment { ctx, row }),
        Err(StoreError::NotFound(_)) => not_found(ctx).await,
        Err(error) => server_error(ctx, &error),
    }
}

/// `POST /tags/{id}/delete`.
pub(crate) async fn delete_tag(
    State(state): State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Ok(uuid) = id.parse::<Uuid>() else {
        return not_found(ctx).await;
    };
    match state.store.delete_tag(uuid).await {
        // Already gone is what was asked for.
        Ok(()) | Err(StoreError::NotFound(_)) => {}
        Err(error) => return server_error(ctx, &error),
    }
    if is_htmx(&headers) {
        StatusCode::OK.into_response()
    } else {
        Redirect::to(&ctx.url("/tags")).into_response()
    }
}
