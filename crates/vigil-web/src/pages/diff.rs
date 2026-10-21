//! The difference between two versions of a watch. Notifications link here, with the ids of
//! the two versions in the address.

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use uuid::Uuid;
use vigil_core::{DiffView, WatchId};
use vigil_store::{SnapshotMeta, StoreError, now_ms};

use super::{not_found, parse_id, server_error};
use crate::AppState;
use crate::diff_html::{self, Inline, SideBySide};
use crate::fmt;
use crate::page::{Ctx, Nav, render, render_with};

/// Versions offered in the pickers.
const LISTED: u32 = 200;

/// Lines of context around a change.
const CONTEXT: usize = 3;

/// A version in a picker.
struct Pick {
    id: String,
    label: String,
    from: bool,
    to: bool,
}

#[derive(Template)]
#[template(path = "diff.html")]
struct Page {
    ctx: Ctx,
    name: String,
    action: String,
    history_url: String,
    watch_url: String,
    picks: Vec<Pick>,
    /// `inline` or `side`, as asked.
    side: bool,
    inline_url: String,
    side_url: String,
    summary: Option<String>,
    same: bool,
    inline: Option<Inline>,
    columns: Option<SideBySide>,
    /// Fewer than two versions exist, so there is nothing to compare yet.
    too_few: bool,
}

#[derive(Template)]
#[template(path = "snapshot_gone.html")]
struct Gone {
    ctx: Ctx,
    history_url: String,
    kept: String,
}

/// The query of the diff page.
#[derive(Debug, Deserialize)]
pub(crate) struct DiffQuery {
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: String,
    #[serde(default)]
    view: String,
}

/// Which version an address names.
enum Named {
    /// It names none: use the default.
    Unnamed,
    /// It names this one.
    Id(Uuid),
    /// It is not an id at all.
    Bad,
}

fn named(text: &str) -> Named {
    let text = text.trim();
    if text.is_empty() {
        Named::Unnamed
    } else {
        text.parse().map_or(Named::Bad, Named::Id)
    }
}

/// `GET /watch/{id}/diff?from=&to=&view=`.
pub(crate) async fn diff(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    Query(query): Query<DiffQuery>,
) -> Response {
    let Some(id) = parse_id(&id) else {
        return not_found(ctx).await;
    };
    match build(&state, &ctx, id, &query).await {
        Ok(Outcome::Page(page)) => render(&*page),
        Ok(Outcome::Gone(gone)) => render_with(StatusCode::NOT_FOUND, &*gone),
        Ok(Outcome::NotFound) | Err(StoreError::NotFound(_)) => not_found(ctx).await,
        Err(error) => server_error(ctx, &error),
    }
}

enum Outcome {
    Page(Box<Page>),
    Gone(Box<Gone>),
    NotFound,
}

async fn build(
    state: &AppState,
    ctx: &Ctx,
    id: WatchId,
    query: &DiffQuery,
) -> Result<Outcome, StoreError> {
    let t = ctx.translator();
    let watch = state.store.get_watch(id).await?;
    let listed = state.store.list_snapshots(id, LISTED).await?;
    let gone = || Gone {
        ctx: ctx.clone().nav(Nav::Watches),
        history_url: ctx.url(&format!("/watch/{id}/history")),
        kept: t.plural("detail.versions", listed.len()),
    };

    // Which two versions: those named, else the newest and the one before it.
    let position = |wanted: Uuid| listed.iter().position(|meta| meta.id == wanted);
    let (to_named, from_named) = (named(&query.to), named(&query.from));
    if matches!(to_named, Named::Bad) || matches!(from_named, Named::Bad) {
        return Ok(Outcome::NotFound);
    }
    let to_id = match to_named {
        Named::Id(wanted) => Some(wanted),
        _ => listed.first().map(|meta| meta.id),
    };
    let from_id = match from_named {
        Named::Id(wanted) => Some(wanted),
        _ => to_id
            .and_then(position)
            .and_then(|at| listed.get(at + 1))
            .map(|meta| meta.id),
    };

    let mut picks: Vec<Pick> = listed
        .iter()
        .map(|meta| pick(t, meta, from_id, to_id))
        .collect();
    let (Some(from_id), Some(to_id)) = (from_id, to_id) else {
        return Ok(Outcome::Page(Box::new(page(
            ctx,
            &watch.name,
            id,
            query,
            picks,
            None,
            true,
        ))));
    };

    // Each version must exist and be this watch's; one that retention removed gets its own page.
    let mut metas = Vec::new();
    for wanted in [from_id, to_id] {
        match state.store.get_snapshot(wanted).await {
            Ok(meta) if meta.watch_id == id => metas.push(meta),
            Ok(_) => return Ok(Outcome::NotFound),
            Err(StoreError::NotFound(_)) => return Ok(Outcome::Gone(Box::new(gone()))),
            Err(error) => return Err(error),
        }
    }
    // A version older than the pickers list is offered too, so that the page can show it.
    for meta in &metas {
        if !picks.iter().any(|p| p.id == meta.id.to_string()) {
            picks.push(pick(t, meta, Some(from_id), Some(to_id)));
        }
    }
    let old = state.store.snapshot_content(from_id).await?;
    let new = state.store.snapshot_content(to_id).await?;
    let view = DiffView::between(&old, &new, CONTEXT);
    Ok(Outcome::Page(Box::new(page(
        ctx,
        &watch.name,
        id,
        query,
        picks,
        Some(view),
        false,
    ))))
}

fn pick(
    t: &crate::i18n::Translator,
    meta: &SnapshotMeta,
    from: Option<Uuid>,
    to: Option<Uuid>,
) -> Pick {
    let when = fmt::when(t, Some(meta.taken_at), now_ms());
    Pick {
        id: meta.id.to_string(),
        label: format!("{} ({})", fmt::exact(meta.taken_at), when.text),
        from: Some(meta.id) == from,
        to: Some(meta.id) == to,
    }
}

fn page(
    ctx: &Ctx,
    name: &str,
    id: WatchId,
    query: &DiffQuery,
    picks: Vec<Pick>,
    view: Option<DiffView>,
    too_few: bool,
) -> Page {
    let t = ctx.translator();
    let side = query.view == "side";
    let selected = |pick_from: bool| picks.iter().find(|p| if pick_from { p.from } else { p.to });
    let link = |view: &str| {
        let mut url = format!("/watch/{id}/diff?view={view}");
        if let Some(p) = selected(true) {
            url.push_str(&format!("&from={}", p.id));
        }
        if let Some(p) = selected(false) {
            url.push_str(&format!("&to={}", p.id));
        }
        ctx.url(&url)
    };
    let (inline_url, side_url) = (link("inline"), link("side"));
    let (summary, same, inline, columns) = match &view {
        Some(view) => (
            Some(t.t2("diff.summary", "added", view.added, "removed", view.removed)),
            view.is_empty(),
            (!side).then(|| diff_html::inline(t, view)),
            side.then(|| diff_html::side_by_side(t, view)),
        ),
        None => (None, false, None, None),
    };
    Page {
        ctx: ctx.clone().nav(Nav::Watches),
        name: name.to_owned(),
        action: ctx.url(&format!("/watch/{id}/diff")),
        history_url: ctx.url(&format!("/watch/{id}/history")),
        watch_url: ctx.url(&format!("/watch/{id}")),
        picks,
        side,
        inline_url,
        side_url,
        summary,
        same,
        inline,
        columns,
        too_few,
    }
}
