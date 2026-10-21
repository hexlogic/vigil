//! `/api/v1/watch/{id}/history` and `/diff`: the versions of a watch's text and how they differ.

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use vigil_core::{LineDiff, WatchId};
use vigil_store::{SnapshotMeta, StoreError};

use super::error::{ApiError, ErrorBody};
use crate::AppState;
use crate::fmt::rfc3339;

/// Versions listed when the request does not say.
const DEFAULT_LIMIT: u32 = 100;

/// Most versions one request lists.
const MAX_LIMIT: u32 = 500;

/// Versions looked through to find the two to compare. Retention keeps 100 by default.
const COMPARE_WINDOW: u32 = 500;

/// Most lines of a difference one answer holds.
const MAX_LINES: usize = 5000;

/// Most characters of a unified diff one answer holds.
const MAX_UNIFIED_CHARS: usize = 1_000_000;

/// Unchanged lines shown around a change in the unified diff.
const CONTEXT: usize = 3;

/// One version of a watch's text, without the text.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct SnapshotView {
    /// The version's id.
    id: Uuid,
    /// When it was taken (RFC 3339, UTC).
    taken_at: String,
    /// The BLAKE3 hash of the text, in hex.
    hash: String,
    /// Size of the fetched body in bytes, before processing.
    raw_size: u64,
    /// The HTTP status of the response.
    http_status: Option<u16>,
    /// The `Content-Type` of the response.
    content_type: Option<String>,
}

impl From<&SnapshotMeta> for SnapshotView {
    fn from(meta: &SnapshotMeta) -> Self {
        Self {
            id: meta.id,
            taken_at: rfc3339(meta.taken_at),
            hash: meta.hash.to_hex(),
            raw_size: meta.raw_size,
            http_status: meta.http_status,
            content_type: meta.content_type.clone(),
        }
    }
}

/// The versions of a watch, newest first.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct History {
    /// The versions that are kept, newest first.
    snapshots: Vec<SnapshotView>,
}

/// One version with its text.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct SnapshotWithText {
    #[serde(flatten)]
    snapshot: SnapshotView,
    /// The processed text.
    content: String,
}

/// How many versions to list.
#[derive(Debug, Deserialize, IntoParams)]
pub(super) struct HistoryQuery {
    /// At most this many, newest first (default 100, at most 500).
    limit: Option<u32>,
}

/// Which two versions to compare.
#[derive(Debug, Deserialize, IntoParams)]
pub(super) struct DiffQuery {
    /// The older version. Defaults to the one before `to`.
    from: Option<Uuid>,
    /// The newer version. Defaults to the newest.
    to: Option<Uuid>,
}

/// How two versions differ, by line.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct Diff {
    /// The older version.
    from: SnapshotView,
    /// The newer version.
    to: SnapshotView,
    /// How many lines were added.
    added_count: usize,
    /// How many lines were removed.
    removed_count: usize,
    /// The added lines, in order.
    added: Vec<String>,
    /// The removed lines, in order.
    removed: Vec<String>,
    /// The difference as unified-diff hunks with three lines of context; empty when the two
    /// versions have the same lines.
    unified: String,
    /// Whether `added`, `removed` or `unified` were cut short. The counts are always whole.
    truncated: bool,
}

fn watch_id(text: &str) -> Result<WatchId, ApiError> {
    text.parse()
        .map_err(|_| ApiError::not_found(format!("watch {text:?}")))
}

/// A version of this watch, or the same "not found" as for a made-up one: an id of another
/// watch's version must not be a way to read it.
async fn snapshot_of(state: &AppState, watch: WatchId, id: Uuid) -> Result<SnapshotMeta, ApiError> {
    match state.store.get_snapshot(id).await {
        Ok(meta) if meta.watch_id == watch => Ok(meta),
        Ok(_) | Err(StoreError::NotFound(_)) => Err(ApiError::not_found(format!("version {id}"))),
        Err(error) => Err(error.into()),
    }
}

/// List the versions of a watch's text that are kept, newest first.
#[utoipa::path(
    get,
    path = "/watch/{id}/history",
    tag = "history",
    security(("api_key" = [])),
    params(("id" = WatchId, Path, description = "The watch's id"), HistoryQuery),
    responses(
        (status = 200, description = "The versions", body = History),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 404, description = "No such watch", body = ErrorBody),
    )
)]
pub(super) async fn list_history(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<History>, ApiError> {
    let id = watch_id(&id)?;
    state.store.get_watch(id).await?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let snapshots = state.store.list_snapshots(id, limit).await?;
    Ok(Json(History {
        snapshots: snapshots.iter().map(SnapshotView::from).collect(),
    }))
}

/// Read one version of a watch's text.
#[utoipa::path(
    get,
    path = "/watch/{id}/history/{snapshot}",
    tag = "history",
    security(("api_key" = [])),
    params(
        ("id" = WatchId, Path, description = "The watch's id"),
        ("snapshot" = Uuid, Path, description = "The version's id"),
    ),
    responses(
        (status = 200, description = "The version and its text", body = SnapshotWithText),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 404, description = "No such watch or version", body = ErrorBody),
    )
)]
pub(super) async fn get_snapshot(
    State(state): State<AppState>,
    Path((id, snapshot)): Path<(String, String)>,
) -> Result<Json<SnapshotWithText>, ApiError> {
    let id = watch_id(&id)?;
    state.store.get_watch(id).await?;
    let snapshot: Uuid = snapshot
        .parse()
        .map_err(|_| ApiError::not_found(format!("version {snapshot:?}")))?;
    let meta = snapshot_of(&state, id, snapshot).await?;
    let content = state.store.snapshot_content(snapshot).await?;
    Ok(Json(SnapshotWithText {
        snapshot: SnapshotView::from(&meta),
        content,
    }))
}

/// Compare two versions of a watch's text by line. With no `from` and `to`, the newest two.
#[utoipa::path(
    get,
    path = "/watch/{id}/diff",
    tag = "history",
    security(("api_key" = [])),
    params(("id" = WatchId, Path, description = "The watch's id"), DiffQuery),
    responses(
        (status = 200, description = "The difference", body = Diff),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 404, description = "No such watch or version, or fewer than two versions", body = ErrorBody),
    )
)]
pub(super) async fn get_diff(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<Diff>, ApiError> {
    let id = watch_id(&id)?;
    state.store.get_watch(id).await?;
    let listed = state.store.list_snapshots(id, COMPARE_WINDOW).await?;
    let to_id = query.to.or_else(|| listed.first().map(|meta| meta.id));
    let from_id = query.from.or_else(|| {
        let at = listed.iter().position(|meta| Some(meta.id) == to_id)?;
        listed.get(at + 1).map(|meta| meta.id)
    });
    let (Some(from_id), Some(to_id)) = (from_id, to_id) else {
        return Err(
            ApiError::not_found("a second version to compare with").code("nothing_to_compare")
        );
    };
    let from = snapshot_of(&state, id, from_id).await?;
    let to = snapshot_of(&state, id, to_id).await?;
    let old = state.store.snapshot_content(from_id).await?;
    let new = state.store.snapshot_content(to_id).await?;

    let diff = LineDiff::between(&old, &new);
    let mut truncated = false;
    let mut lines = |iter: &mut dyn Iterator<Item = &str>| -> Vec<String> {
        let kept: Vec<String> = iter.take(MAX_LINES + 1).map(str::to_owned).collect();
        if kept.len() > MAX_LINES {
            truncated = true;
        }
        kept.into_iter().take(MAX_LINES).collect()
    };
    let added = lines(&mut diff.added());
    let removed = lines(&mut diff.removed());
    let mut unified = diff.unified(CONTEXT);
    if unified.chars().count() > MAX_UNIFIED_CHARS {
        let cut = unified
            .char_indices()
            .nth(MAX_UNIFIED_CHARS)
            .map_or(unified.len(), |(at, _)| at);
        unified.truncate(unified[..cut].rfind('\n').map_or(0, |at| at + 1));
        truncated = true;
    }
    Ok(Json(Diff {
        from: SnapshotView::from(&from),
        to: SnapshotView::from(&to),
        added_count: diff.added_count(),
        removed_count: diff.removed_count(),
        added,
        removed,
        unified,
        truncated,
    }))
}
