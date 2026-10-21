//! `/api/v1/watch`: list, add, read, replace and delete watches, and check one now.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use vigil_core::WatchId;
use vigil_scheduler::RunOutcome;
use vigil_store::{NewWatch, WatchLinks};

use super::error::{ApiError, ApiJson, ErrorBody};
use super::watch_json::{Built, WatchInput, WatchView, build};
use crate::AppState;

/// How long `?wait=true` waits for a check to end.
const WAIT: Duration = Duration::from_secs(60);

/// The watches.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct WatchList {
    /// The watches, by name.
    watches: Vec<WatchView>,
}

/// Which watches to list.
#[derive(Debug, Deserialize, IntoParams)]
pub(super) struct ListQuery {
    /// Only the watches that have this tag.
    tag: Option<Uuid>,
}

/// Whether to wait for a check.
#[derive(Debug, Deserialize, IntoParams)]
pub(super) struct CheckQuery {
    /// Wait up to a minute for the check to end and answer with the watch.
    #[serde(default)]
    wait: bool,
}

/// How a requested check ended.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
enum CheckOutcome {
    /// The check ran. A page that could not be fetched is still a check that ran: see
    /// `state.last_error` of the watch.
    Done,
    /// The check could not even be recorded.
    Failed,
}

/// The answer to a request to check a watch.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct CheckResponse {
    /// The check was asked for and has not ended yet.
    queued: bool,
    /// How it ended, when it was waited for and did.
    outcome: Option<CheckOutcome>,
    /// The watch after the check, when it was waited for and ended.
    watch: Option<WatchView>,
}

fn parse_id(text: &str) -> Result<WatchId, ApiError> {
    text.parse()
        .map_err(|_| ApiError::not_found(format!("watch {text:?}")))
}

/// The view of a watch with its tags and targets.
async fn view_of(state: &AppState, id: WatchId) -> Result<WatchView, ApiError> {
    let watch = state.store.get_watch(id).await?;
    let tags = state.store.tags_for_watch(id).await?;
    let targets = state.store.targets_for_watch(id, false).await?;
    Ok(WatchView::new(
        &watch,
        tags.iter().map(|t| t.id).collect(),
        targets.iter().map(|t| t.id).collect(),
    ))
}

/// Tell the scheduler to look at the watch list now, which is a courtesy: it does so every
/// 30 seconds anyway.
async fn reload(state: &AppState) {
    if let Err(error) = state.scheduler.reload().await {
        tracing::warn!(%error, "the scheduler could not be told about a change");
    }
}

/// List the watches, by name.
#[utoipa::path(
    get,
    path = "/watch",
    tag = "watches",
    security(("api_key" = [])),
    params(ListQuery),
    responses(
        (status = 200, description = "The watches", body = WatchList),
        (status = 401, description = "No valid token", body = ErrorBody),
    )
)]
pub(super) async fn list_watches(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<WatchList>, ApiError> {
    let mut watches = state.store.list_watches().await?;
    let tags = state.store.watch_tags_map().await?;
    let targets = state.store.watch_target_ids().await?;
    if let Some(tag) = query.tag {
        watches.retain(|w| tags.get(&w.id).is_some_and(|t| t.contains(&tag)));
    }
    watches.sort_by_key(|w| (w.name.to_lowercase(), w.id));
    let watches = watches
        .iter()
        .map(|watch| {
            WatchView::new(
                watch,
                tags.get(&watch.id).cloned().unwrap_or_default(),
                targets.get(&watch.id).cloned().unwrap_or_default(),
            )
        })
        .collect();
    Ok(Json(WatchList { watches }))
}

/// Add a watch. Its first check starts at once.
#[utoipa::path(
    post,
    path = "/watch",
    tag = "watches",
    security(("api_key" = [])),
    request_body = WatchInput,
    responses(
        (status = 201, description = "The watch that was made", body = WatchView),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 422, description = "A field is not acceptable", body = ErrorBody),
    )
)]
pub(super) async fn create_watch(
    State(state): State<AppState>,
    ApiJson(input): ApiJson<WatchInput>,
) -> Result<(StatusCode, HeaderMap, Json<WatchView>), ApiError> {
    let Built {
        new,
        paused,
        tags,
        targets,
        template,
    } = build(&state, input).await?;
    let links = WatchLinks {
        tags: &tags,
        targets: &targets,
        template: template.as_deref(),
    };
    let watch = state
        .store
        .create_watch_with_links(&new, paused, &links)
        .await?;
    reload(&state).await;
    if !paused && let Err(error) = state.scheduler.check_now(watch.id).await {
        tracing::warn!(%error, "the first check could not be asked for");
    }
    let mut headers = HeaderMap::new();
    let location = format!("{}/api/v1/watch/{}", state.web.base_path(), watch.id);
    if let Ok(value) = HeaderValue::from_str(&location) {
        headers.insert(header::LOCATION, value);
    }
    let view = view_of(&state, watch.id).await?;
    Ok((StatusCode::CREATED, headers, Json(view)))
}

/// Read one watch.
#[utoipa::path(
    get,
    path = "/watch/{id}",
    tag = "watches",
    security(("api_key" = [])),
    params(("id" = WatchId, Path, description = "The watch's id")),
    responses(
        (status = 200, description = "The watch", body = WatchView),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 404, description = "No such watch", body = ErrorBody),
    )
)]
pub(super) async fn get_watch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<WatchView>, ApiError> {
    Ok(Json(view_of(&state, parse_id(&id)?).await?))
}

/// Replace a watch. Everything but its history is replaced, so a field that is left out gets
/// its default. Changing how the page is fetched or processed forgets the remembered `ETag`
/// and the failure streak, as editing in the web UI does.
#[utoipa::path(
    put,
    path = "/watch/{id}",
    tag = "watches",
    security(("api_key" = [])),
    params(("id" = WatchId, Path, description = "The watch's id")),
    request_body = WatchInput,
    responses(
        (status = 200, description = "The watch as it is now", body = WatchView),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 404, description = "No such watch", body = ErrorBody),
        (status = 422, description = "A field is not acceptable", body = ErrorBody),
    )
)]
pub(super) async fn replace_watch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<WatchInput>,
) -> Result<Json<WatchView>, ApiError> {
    let id = parse_id(&id)?;
    let current = state.store.get_watch(id).await?;
    let built = build(&state, input).await?;
    // Writing the settings clears the remembered validators and the failure streak and moves
    // the schedule, so it is only done when something changed. All of it is saved or none.
    let settings = (NewWatch::from(&current) != built.new).then_some(&built.new);
    let paused = (current.paused != built.paused).then_some(built.paused);
    let links = WatchLinks {
        tags: &built.tags,
        targets: &built.targets,
        template: built.template.as_deref(),
    };
    state
        .store
        .replace_watch(id, settings, paused, &links)
        .await?;
    reload(&state).await;
    Ok(Json(view_of(&state, id).await?))
}

/// Delete a watch with its history.
#[utoipa::path(
    delete,
    path = "/watch/{id}",
    tag = "watches",
    security(("api_key" = [])),
    params(("id" = WatchId, Path, description = "The watch's id")),
    responses(
        (status = 204, description = "The watch is gone"),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 404, description = "No such watch", body = ErrorBody),
    )
)]
pub(super) async fn delete_watch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_watch(parse_id(&id)?).await?;
    reload(&state).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Check a watch now, even if it is paused. It goes through the same limits as a scheduled
/// check and joins one that is already running.
#[utoipa::path(
    post,
    path = "/watch/{id}/check",
    tag = "watches",
    security(("api_key" = [])),
    params(("id" = WatchId, Path, description = "The watch's id"), CheckQuery),
    responses(
        (status = 200, description = "The check ended (with `wait=true`)", body = CheckResponse),
        (status = 202, description = "The check was asked for", body = CheckResponse),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 404, description = "No such watch", body = ErrorBody),
        (status = 503, description = "The server is shutting down", body = ErrorBody),
    )
)]
pub(super) async fn check_watch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<CheckQuery>,
) -> Result<(StatusCode, Json<CheckResponse>), ApiError> {
    let id = parse_id(&id)?;
    state.store.get_watch(id).await?;
    let ticket = state
        .scheduler
        .check_now(id)
        .await
        .map_err(|_| ApiError::unavailable())?;
    let queued = (
        StatusCode::ACCEPTED,
        Json(CheckResponse {
            queued: true,
            outcome: None,
            watch: None,
        }),
    );
    if !query.wait {
        return Ok(queued);
    }
    let outcome = match tokio::time::timeout(WAIT, ticket.wait()).await {
        Err(_) => return Ok(queued),
        Ok(Err(_)) => return Err(ApiError::unavailable()),
        Ok(Ok(outcome)) => outcome,
    };
    let outcome = match outcome {
        RunOutcome::Done { .. } => CheckOutcome::Done,
        RunOutcome::Failed => CheckOutcome::Failed,
        RunOutcome::Gone => return Err(ApiError::not_found(format!("watch {id}"))),
    };
    Ok((
        StatusCode::OK,
        Json(CheckResponse {
            queued: false,
            outcome: Some(outcome),
            watch: Some(view_of(&state, id).await?),
        }),
    ))
}
