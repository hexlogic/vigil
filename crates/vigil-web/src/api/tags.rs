//! `/api/v1/tags`: groups of watches, with a default interval and default targets.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use vigil_scheduler::backoff::{MAX_INTERVAL, MIN_INTERVAL};
use vigil_store::{NewTag, Tag};

use super::error::{ApiError, ApiJson, ErrorBody};
use crate::AppState;
use crate::fmt::rfc3339;
use crate::pages::{TAG_COLORS, TAG_NAME_LEN};

/// A tag, as the API sends it.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct TagView {
    /// The tag's id.
    id: Uuid,
    /// Its unique name.
    name: String,
    /// One of `gray`, `red`, `orange`, `yellow`, `green`, `teal`, `blue`, `purple`, `pink`.
    color: String,
    /// The interval, in seconds, of a watch added with this tag and no interval of its own.
    default_interval_secs: Option<u64>,
    /// The targets that are notified about a change of any watch with this tag.
    targets: Vec<Uuid>,
    /// How many watches have this tag.
    watches: u32,
    /// When it was made (RFC 3339, UTC).
    created_at: String,
}

/// The tags.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct TagList {
    /// The tags, by name without regard to case.
    tags: Vec<TagView>,
}

/// A tag, as the API accepts it. `PUT` replaces all of it, so a field that is left out gets
/// its default.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct TagInput {
    /// A unique name of at most 50 characters.
    name: String,
    /// One of the colours above; `gray` when left out.
    #[serde(default)]
    color: Option<String>,
    /// The interval, in seconds, for a watch with this tag and no interval of its own.
    #[serde(default)]
    default_interval_secs: Option<u64>,
    /// The targets notified about a change of any watch with this tag.
    #[serde(default)]
    targets: Vec<Uuid>,
}

fn parse_id(text: &str) -> Result<Uuid, ApiError> {
    text.parse()
        .map_err(|_| ApiError::not_found(format!("tag {text:?}")))
}

async fn view_of(state: &AppState, tag: &Tag) -> Result<TagView, ApiError> {
    let counts = state.store.tag_watch_counts().await?;
    let targets = state.store.tag_targets(tag.id).await?;
    Ok(TagView {
        id: tag.id,
        name: tag.name.clone(),
        color: tag.color.clone(),
        default_interval_secs: tag.default_interval_secs,
        targets,
        watches: counts.get(&tag.id).copied().unwrap_or(0),
        created_at: rfc3339(tag.created_at),
    })
}

/// Check a request. Unlike the web form, which shows an unknown colour as gray, the API
/// refuses it: a program should learn that its colour was not used.
async fn check(state: &AppState, input: TagInput) -> Result<(NewTag, Vec<Uuid>), ApiError> {
    let name = input.name.trim().to_owned();
    if name.is_empty() {
        return Err(ApiError::invalid("the name is empty").field("name"));
    }
    if name.chars().count() > TAG_NAME_LEN {
        return Err(ApiError::invalid(format!(
            "the name can have at most {TAG_NAME_LEN} characters"
        ))
        .field("name"));
    }
    let color = input.color.unwrap_or_else(|| "gray".to_owned());
    if !TAG_COLORS.contains(&color.as_str()) {
        return Err(ApiError::invalid(format!(
            "the colour must be one of {}",
            TAG_COLORS.join(", ")
        ))
        .field("color"));
    }
    if let Some(secs) = input.default_interval_secs {
        let every = std::time::Duration::from_secs(secs);
        if every < MIN_INTERVAL || every > MAX_INTERVAL {
            return Err(
                ApiError::invalid("the interval must be between 1 second and 365 days")
                    .field("default_interval_secs"),
            );
        }
    }
    let known: Vec<Uuid> = state
        .store
        .list_targets()
        .await?
        .iter()
        .map(|t| t.id)
        .collect();
    let mut targets = Vec::new();
    for id in input.targets {
        if !known.contains(&id) {
            return Err(ApiError::invalid(format!("targets: {id} does not exist")).field("targets"));
        }
        if !targets.contains(&id) {
            targets.push(id);
        }
    }
    Ok((
        NewTag {
            name,
            color,
            default_interval_secs: input.default_interval_secs,
        },
        targets,
    ))
}

/// List the tags, by name without regard to case.
#[utoipa::path(
    get,
    path = "/tags",
    tag = "tags",
    security(("api_key" = [])),
    responses(
        (status = 200, description = "The tags", body = TagList),
        (status = 401, description = "No valid token", body = ErrorBody),
    )
)]
pub(super) async fn list_tags(State(state): State<AppState>) -> Result<Json<TagList>, ApiError> {
    let mut found = state.store.list_tags().await?;
    found.sort_by_key(|tag| (tag.name.to_lowercase(), tag.id));
    let mut tags = Vec::new();
    for tag in &found {
        tags.push(view_of(&state, tag).await?);
    }
    Ok(Json(TagList { tags }))
}

/// Make a tag.
#[utoipa::path(
    post,
    path = "/tags",
    tag = "tags",
    security(("api_key" = [])),
    request_body = TagInput,
    responses(
        (status = 201, description = "The tag that was made", body = TagView),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 409, description = "The name is taken", body = ErrorBody),
        (status = 422, description = "A field is not acceptable", body = ErrorBody),
    )
)]
pub(super) async fn create_tag(
    State(state): State<AppState>,
    ApiJson(input): ApiJson<TagInput>,
) -> Result<(StatusCode, [(header::HeaderName, String); 1], Json<TagView>), ApiError> {
    let (new, targets) = check(&state, input).await?;
    let tag = state.store.create_tag(&new).await?;
    if let Err(error) = state.store.set_tag_targets(tag.id, &targets).await {
        // Not half a tag: take back what was made.
        if let Err(undo) = state.store.delete_tag(tag.id).await {
            tracing::warn!(error = %undo, "a tag that could not be completed was not removed");
        }
        return Err(error.into());
    }
    let location = format!("{}/api/v1/tags/{}", state.web.base_path(), tag.id);
    let view = view_of(&state, &tag).await?;
    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(view),
    ))
}

/// Read one tag.
#[utoipa::path(
    get,
    path = "/tags/{id}",
    tag = "tags",
    security(("api_key" = [])),
    params(("id" = Uuid, Path, description = "The tag's id")),
    responses(
        (status = 200, description = "The tag", body = TagView),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 404, description = "No such tag", body = ErrorBody),
    )
)]
pub(super) async fn get_tag(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TagView>, ApiError> {
    let tag = state.store.get_tag(parse_id(&id)?).await?;
    Ok(Json(view_of(&state, &tag).await?))
}

/// Replace a tag. Its watches keep it; a new default interval applies to watches added later.
#[utoipa::path(
    put,
    path = "/tags/{id}",
    tag = "tags",
    security(("api_key" = [])),
    params(("id" = Uuid, Path, description = "The tag's id")),
    request_body = TagInput,
    responses(
        (status = 200, description = "The tag as it is now", body = TagView),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 404, description = "No such tag", body = ErrorBody),
        (status = 409, description = "The name is taken", body = ErrorBody),
        (status = 422, description = "A field is not acceptable", body = ErrorBody),
    )
)]
pub(super) async fn replace_tag(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<TagInput>,
) -> Result<Json<TagView>, ApiError> {
    let id = parse_id(&id)?;
    state.store.get_tag(id).await?;
    let (new, targets) = check(&state, input).await?;
    let tag = state.store.update_tag(id, &new).await?;
    state.store.set_tag_targets(id, &targets).await?;
    Ok(Json(view_of(&state, &tag).await?))
}

/// Delete a tag. Its watches stay, without it.
#[utoipa::path(
    delete,
    path = "/tags/{id}",
    tag = "tags",
    security(("api_key" = [])),
    params(("id" = Uuid, Path, description = "The tag's id")),
    responses(
        (status = 204, description = "The tag is gone"),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 404, description = "No such tag", body = ErrorBody),
    )
)]
pub(super) async fn delete_tag(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_tag(parse_id(&id)?).await?;
    Ok(StatusCode::NO_CONTENT)
}
