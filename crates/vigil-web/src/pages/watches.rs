//! The watch list, and what can be done to a watch from a row of it.

use std::collections::HashMap;
use std::time::Duration;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use vigil_core::{Watch, WatchId};
use vigil_store::{StoreError, Tag, now_ms};

use crate::AppState;
use crate::fmt::{self, TimeView};
use crate::page::{Ctx, Nav, render};
use crate::pages::tag_class;

/// How long a request to check a watch waits for the check, before it answers that the check
/// is still running.
const CHECK_WAIT: Duration = Duration::from_secs(60);

/// One watch in the list.
pub(crate) struct Row {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) paused: bool,
    pub(crate) status_text: String,
    pub(crate) status_class: &'static str,
    pub(crate) error: Option<String>,
    pub(crate) every: String,
    pub(crate) checked: TimeView,
    pub(crate) changed: TimeView,
    pub(crate) tags: Vec<TagChip>,
    pub(crate) show_url: String,
    pub(crate) edit_url: String,
    pub(crate) row_url: String,
    pub(crate) check_url: String,
    pub(crate) pause_url: String,
    pub(crate) delete_url: String,
    /// The row is asked for again in a few seconds, because a check has not ended yet.
    pub(crate) still_checking: bool,
}

/// A tag, as shown next to a watch.
pub(crate) struct TagChip {
    pub(crate) name: String,
    pub(crate) class: &'static str,
}

impl Row {
    fn new(ctx: &Ctx, watch: &Watch, tags: &[&Tag], now: i64) -> Self {
        let t = ctx.translator();
        let (status_text, status_class) = if watch.paused {
            (t.t("watches.status.paused"), "")
        } else if watch.state.consecutive_errors > 0 {
            (
                t.t1("watches.status.error", "n", watch.state.consecutive_errors),
                "badge-error",
            )
        } else if watch.state.last_checked_at.is_none() {
            (t.t("watches.status.pending"), "badge-warn")
        } else {
            (t.t("watches.status.ok"), "badge-ok")
        };
        let id = watch.id.to_string();
        let at = |suffix: &str| ctx.url(&format!("/watch/{id}{suffix}"));
        let pause_path = if watch.paused { "/resume" } else { "/pause" };
        Row {
            name: watch.name.clone(),
            url: watch.url.clone(),
            paused: watch.paused,
            status_text,
            status_class,
            error: watch.state.last_error.clone().filter(|_| !watch.paused),
            every: fmt::every(watch.schedule.interval().as_secs()),
            checked: fmt::when(t, watch.state.last_checked_at, now),
            changed: fmt::when(t, watch.state.last_changed_at, now),
            tags: tags
                .iter()
                .map(|tag| TagChip {
                    name: tag.name.clone(),
                    class: tag_class(&tag.color),
                })
                .collect(),
            show_url: at(""),
            edit_url: at("/edit"),
            row_url: at("/row"),
            check_url: at("/check"),
            pause_url: at(pause_path),
            delete_url: at("/delete"),
            still_checking: false,
            id,
        }
    }
}

#[derive(Template)]
#[template(path = "watches.html")]
struct Watches {
    ctx: Ctx,
    rows: Vec<Row>,
    /// Every tag, to filter by.
    tags: Vec<FilterTag>,
    /// The tag the list is filtered by, if any.
    filtered: bool,
    /// All watches, whatever the filter.
    total: usize,
}

struct FilterTag {
    name: String,
    class: &'static str,
    url: String,
    current: bool,
}

#[derive(Template)]
#[template(path = "watch_row.html")]
struct RowFragment {
    ctx: Ctx,
    row: Row,
}

#[derive(Template)]
#[template(path = "watch_delete.html")]
struct DeleteFragment {
    ctx: Ctx,
    row: Row,
}

/// Query of the watch list.
#[derive(Debug, Deserialize)]
pub(crate) struct ListQuery {
    #[serde(default)]
    tag: String,
}

/// `GET /`.
pub(crate) async fn watches(
    State(state): State<AppState>,
    ctx: Ctx,
    Query(query): Query<ListQuery>,
) -> Response {
    match list(&state, ctx.clone(), &query.tag).await {
        Ok(page) => render(&page),
        Err(error) => server_error(ctx, &error),
    }
}

async fn list(state: &AppState, ctx: Ctx, tag: &str) -> Result<Watches, StoreError> {
    let (watches, tags, links) = (
        state.store.list_watches().await?,
        state.store.list_tags().await?,
        state.store.watch_tags_map().await?,
    );
    let now = now_ms();
    let wanted = tags.iter().find(|t| t.id.to_string() == tag).map(|t| t.id);
    let by_id: HashMap<_, _> = tags.iter().map(|tag| (tag.id, tag)).collect();

    let total = watches.len();
    let mut watches: Vec<Watch> = watches
        .into_iter()
        .filter(|watch| {
            wanted.is_none_or(|tag| links.get(&watch.id).is_some_and(|ids| ids.contains(&tag)))
        })
        .collect();
    watches.sort_by_cached_key(|watch| (watch.name.to_lowercase(), watch.created_at));

    let rows = watches
        .iter()
        .map(|watch| {
            let mine: Vec<&Tag> = links
                .get(&watch.id)
                .into_iter()
                .flatten()
                .filter_map(|id| by_id.get(id).copied())
                .collect();
            Row::new(&ctx, watch, &mine, now)
        })
        .collect();
    let tags = tags
        .iter()
        .map(|tag| FilterTag {
            name: tag.name.clone(),
            class: tag_class(&tag.color),
            url: ctx.url(&format!("/?tag={}", tag.id)),
            current: Some(tag.id) == wanted,
        })
        .collect();
    Ok(Watches {
        ctx: ctx.nav(Nav::Watches),
        rows,
        tags,
        filtered: wanted.is_some(),
        total,
    })
}

/// One row, fresh from the database.
async fn row_of(
    state: &AppState,
    ctx: &Ctx,
    id: WatchId,
    still_checking: bool,
) -> Result<Row, StoreError> {
    let watch = state.store.get_watch(id).await?;
    let tags = state.store.tags_for_watch(id).await?;
    let tags: Vec<&Tag> = tags.iter().collect();
    let mut row = Row::new(ctx, &watch, &tags, now_ms());
    row.still_checking = still_checking;
    Ok(row)
}

/// `GET /watch/{id}/row`: the row of the list, to put back after a confirmation is cancelled
/// or a check that was still running has ended.
pub(crate) async fn watch_row(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> Response {
    row_response(&state, ctx, &id, false).await
}

/// `POST /watch/{id}/check`: check the watch now and answer with its updated row.
pub(crate) async fn check_watch(
    State(state): State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Some(id) = parse_id(&id) else {
        return not_found(ctx).await;
    };
    let ticket = match state.scheduler.check_now(id).await {
        Ok(ticket) => ticket,
        Err(error) => {
            tracing::error!(%error, "asking for a check failed");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    let still_checking = tokio::time::timeout(CHECK_WAIT, ticket.wait())
        .await
        .is_err();
    if !is_htmx(&headers) {
        return Redirect::to(&ctx.url("/")).into_response();
    }
    row_response(&state, ctx, &id.to_string(), still_checking).await
}

/// `POST /watch/{id}/pause`.
pub(crate) async fn pause_watch(
    state: State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    id: Path<String>,
) -> Response {
    set_paused(state, ctx, headers, id, true).await
}

/// `POST /watch/{id}/resume`.
pub(crate) async fn resume_watch(
    state: State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    id: Path<String>,
) -> Response {
    set_paused(state, ctx, headers, id, false).await
}

async fn set_paused(
    State(state): State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    Path(id): Path<String>,
    paused: bool,
) -> Response {
    let Some(watch) = parse_id(&id) else {
        return not_found(ctx).await;
    };
    match state.store.set_paused(watch, paused).await {
        Ok(()) => {}
        Err(StoreError::NotFound(_)) => return not_found(ctx).await,
        Err(error) => return server_error(ctx, &error),
    }
    // The scheduler would notice at its next resync; the person who pressed the button wants
    // it now.
    if let Err(error) = state.scheduler.reload().await {
        tracing::warn!(%error, "the scheduler could not be told about the change");
    }
    if !is_htmx(&headers) {
        return Redirect::to(&ctx.url("/")).into_response();
    }
    row_response(&state, ctx, &id, false).await
}

/// `GET /watch/{id}/delete`: ask whether to delete, in place of the row.
pub(crate) async fn confirm_delete(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> Response {
    let Some(watch) = parse_id(&id) else {
        return not_found(ctx).await;
    };
    match row_of(&state, &ctx, watch, false).await {
        Ok(row) => render(&DeleteFragment { ctx, row }),
        Err(StoreError::NotFound(_)) => not_found(ctx).await,
        Err(error) => server_error(ctx, &error),
    }
}

/// `POST /watch/{id}/delete`.
pub(crate) async fn delete_watch(
    State(state): State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Some(watch) = parse_id(&id) else {
        return not_found(ctx).await;
    };
    match state.store.delete_watch(watch).await {
        // Already gone is what was asked for.
        Ok(()) | Err(StoreError::NotFound(_)) => {}
        Err(error) => return server_error(ctx, &error),
    }
    if let Err(error) = state.scheduler.reload().await {
        tracing::warn!(%error, "the scheduler could not be told about the change");
    }
    if is_htmx(&headers) {
        // The row is swapped for nothing.
        StatusCode::OK.into_response()
    } else {
        Redirect::to(&ctx.url("/")).into_response()
    }
}

async fn row_response(state: &AppState, ctx: Ctx, id: &str, still_checking: bool) -> Response {
    let Some(watch) = parse_id(id) else {
        return not_found(ctx).await;
    };
    match row_of(state, &ctx, watch, still_checking).await {
        Ok(row) => render(&RowFragment { ctx, row }),
        Err(StoreError::NotFound(_)) => not_found(ctx).await,
        Err(error) => server_error(ctx, &error),
    }
}

/// A watch id from an address; `None` if it is not one.
pub(crate) fn parse_id(text: &str) -> Option<WatchId> {
    text.parse().ok()
}

/// Whether htmx made the request, so that a fragment is wanted and not a page.
pub(crate) fn is_htmx(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request")
}

pub(crate) async fn not_found(ctx: Ctx) -> Response {
    crate::pages::not_found(ctx).await
}

pub(crate) fn server_error(ctx: Ctx, error: &StoreError) -> Response {
    crate::pages::server_error(ctx, error)
}
