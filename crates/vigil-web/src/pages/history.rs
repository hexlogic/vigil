//! The versions a watch has stored, and one of them.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::Response;
use uuid::Uuid;
use vigil_store::{SnapshotMeta, StoreError, now_ms};

use super::{not_found, parse_id, server_error};
use crate::AppState;
use crate::fmt::{self, TimeView};
use crate::page::{Ctx, Nav, render};

/// Versions listed. A watch keeps 100 by default.
const LISTED: u32 = 500;

/// Characters of a version shown on its page; the rest is counted.
const SHOWN_CHARS: usize = 100_000;

struct Version {
    when: TimeView,
    size: String,
    status: String,
    content_type: String,
    view_url: String,
    /// The difference from the version before it; the oldest has none.
    diff_url: Option<String>,
    newest: bool,
}

#[derive(Template)]
#[template(path = "history.html")]
struct History {
    ctx: Ctx,
    name: String,
    watch_url: String,
    kept: String,
    versions: Vec<Version>,
}

#[derive(Template)]
#[template(path = "snapshot.html")]
struct Snapshot {
    ctx: Ctx,
    name: String,
    when: TimeView,
    facts: String,
    text: String,
    shown: Option<String>,
    history_url: String,
    diff_url: Option<String>,
}

fn version_facts(meta: &SnapshotMeta) -> (String, String, String) {
    (
        fmt::bytes(meta.raw_size),
        meta.http_status
            .map(|s| format!("HTTP {s}"))
            .unwrap_or_default(),
        meta.content_type.clone().unwrap_or_default(),
    )
}

/// `GET /watch/{id}/history`.
pub(crate) async fn history(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> Response {
    let Some(id) = parse_id(&id) else {
        return not_found(ctx).await;
    };
    let loaded = async {
        let watch = state.store.get_watch(id).await?;
        let snapshots = state.store.list_snapshots(id, LISTED).await?;
        Ok::<_, StoreError>((watch, snapshots))
    }
    .await;
    let (watch, snapshots) = match loaded {
        Ok(loaded) => loaded,
        Err(StoreError::NotFound(_)) => return not_found(ctx).await,
        Err(error) => return server_error(ctx, &error),
    };
    let t = ctx.translator();
    let now = now_ms();
    let versions = snapshots
        .iter()
        .enumerate()
        .map(|(at, meta)| {
            let (size, status, content_type) = version_facts(meta);
            Version {
                when: fmt::when(t, Some(meta.taken_at), now),
                size,
                status,
                content_type,
                view_url: ctx.url(&format!("/watch/{id}/history/{}", meta.id)),
                diff_url: snapshots.get(at + 1).map(|older| {
                    ctx.url(&format!(
                        "/watch/{id}/diff?from={}&to={}",
                        older.id, meta.id
                    ))
                }),
                newest: at == 0,
            }
        })
        .collect();
    render(&History {
        name: watch.name,
        watch_url: ctx.url(&format!("/watch/{id}")),
        kept: t.t2(
            "history.kept",
            "count",
            t.plural("detail.versions", snapshots.len()),
            "limit",
            watch.snapshot_retention,
        ),
        versions,
        ctx: ctx.nav(Nav::Watches),
    })
}

/// `GET /watch/{id}/history/{snapshot}`.
pub(crate) async fn snapshot(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, snapshot)): Path<(String, String)>,
) -> Response {
    let (Some(id), Ok(snapshot)) = (parse_id(&id), snapshot.parse::<Uuid>()) else {
        return not_found(ctx).await;
    };
    let loaded = async {
        let watch = state.store.get_watch(id).await?;
        let meta = state.store.get_snapshot(snapshot).await?;
        // A version of another watch is not found here, whoever asks.
        if meta.watch_id != id {
            return Err(StoreError::NotFound(format!("snapshot {snapshot}")));
        }
        let text = state.store.snapshot_content(snapshot).await?;
        let older = state
            .store
            .list_snapshots(id, LISTED)
            .await?
            .into_iter()
            .skip_while(|other| other.id != snapshot)
            .nth(1);
        Ok((watch, meta, text, older))
    }
    .await;
    let (watch, meta, text, older) = match loaded {
        Ok(loaded) => loaded,
        Err(StoreError::NotFound(_)) => return not_found(ctx).await,
        Err(error) => return server_error(ctx, &error),
    };
    let t = ctx.translator();
    let (size, status, content_type) = version_facts(&meta);
    let total = text.chars().count();
    render(&Snapshot {
        name: watch.name,
        when: fmt::when(t, Some(meta.taken_at), now_ms()),
        facts: [status, content_type, size]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · "),
        shown: (total > SHOWN_CHARS)
            .then(|| t.t2("preview.shown", "shown", SHOWN_CHARS, "total", total)),
        text: text.chars().take(SHOWN_CHARS).collect(),
        history_url: ctx.url(&format!("/watch/{id}/history")),
        diff_url: older
            .map(|older| ctx.url(&format!("/watch/{id}/diff?from={}&to={snapshot}", older.id))),
        ctx: ctx.nav(Nav::Watches),
    })
}
