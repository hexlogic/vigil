//! `/api/v1/notifications`: where notifications are sent. A target's address usually holds
//! credentials, so it is written but never read back: it is shown with its secrets hidden.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use vigil_notify::{Channel, validate_template};
use vigil_store::{NewTarget, NotificationTarget};

use super::error::{ApiError, ApiJson, ErrorBody};
use crate::AppState;
use crate::fmt::rfc3339;
use crate::pages::TARGET_NAME_LEN;

/// A notification target, as the API sends it.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct TargetView {
    /// The target's id.
    id: Uuid,
    /// Its unique name.
    name: String,
    /// The kind of channel, such as `ntfy` or `telegram`; `invalid` if the stored address no
    /// longer parses.
    kind: String,
    /// The address with every secret replaced by `***`.
    url: String,
    /// Whether the address would send a secret over plain HTTP to another machine.
    sends_secrets_in_cleartext: bool,
    /// A disabled target stays linked to its watches but is skipped when notifying.
    enabled: bool,
    /// The message template used when the watch has none of its own.
    default_template: Option<String>,
    /// How many watches are linked to the target.
    watches: u32,
    /// When it was made (RFC 3339, UTC).
    created_at: String,
}

/// The targets.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct TargetList {
    /// The targets, by name without regard to case.
    notifications: Vec<TargetView>,
}

/// A target, as the API accepts it.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetInput {
    /// A unique name of at most 100 characters.
    name: String,
    /// The notification address, such as `ntfys://ntfy.sh/my-topic`. Required to make a
    /// target. In a `PUT` it may be left out to keep the stored one, since it cannot be read
    /// back.
    #[serde(default)]
    url: Option<String>,
    /// A message template for this target; empty or left out removes it.
    #[serde(default)]
    default_template: Option<String>,
    /// Whether the target is used. Defaults to `true` on creation; left out in a `PUT`, it
    /// stays as it is.
    #[serde(default)]
    enabled: Option<bool>,
}

/// How a test notification went.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct TestReport {
    /// The channel accepted the message.
    ok: bool,
    /// Why not, when it did not; the text holds no credentials.
    error: Option<String>,
    /// Set when the target's own template failed and the built-in message was sent instead.
    note: Option<String>,
}

fn parse_id(text: &str) -> Result<Uuid, ApiError> {
    text.parse()
        .map_err(|_| ApiError::not_found(format!("notification target {text:?}")))
}

fn view_of(target: &NotificationTarget, watches: u32) -> TargetView {
    let (kind, url, cleartext) = match Channel::parse(&target.url) {
        Ok(channel) => (
            channel.kind().to_owned(),
            channel.redacted(),
            channel.sends_secrets_in_cleartext(),
        ),
        Err(_) => (
            "invalid".to_owned(),
            "(not a valid notification URL)".to_owned(),
            false,
        ),
    };
    TargetView {
        id: target.id,
        name: target.name.clone(),
        kind,
        url,
        sends_secrets_in_cleartext: cleartext,
        enabled: target.enabled,
        default_template: target.default_template.clone(),
        watches,
        created_at: rfc3339(target.created_at),
    }
}

async fn watches_of(state: &AppState, id: Uuid) -> Result<u32, ApiError> {
    Ok(state
        .store
        .linked_watch_counts()
        .await?
        .get(&id)
        .copied()
        .unwrap_or(0))
}

/// Check a request. `keep` is the target being replaced, whose address a missing `url` keeps.
fn check(input: &TargetInput, keep: Option<&NotificationTarget>) -> Result<NewTarget, ApiError> {
    let name = input.name.trim().to_owned();
    if name.is_empty() {
        return Err(ApiError::invalid("the name is empty").field("name"));
    }
    if name.chars().count() > TARGET_NAME_LEN {
        return Err(ApiError::invalid(format!(
            "the name can have at most {TARGET_NAME_LEN} characters"
        ))
        .field("name"));
    }
    let url = match (input.url.as_deref().map(str::trim), keep) {
        (None | Some(""), Some(target)) => target.url.clone(),
        (None | Some(""), None) => {
            return Err(ApiError::invalid("the address is required").field("url"));
        }
        (Some(url), _) => {
            // The message says what is wrong and never repeats the address.
            if let Err(error) = Channel::parse(url) {
                return Err(ApiError::invalid(error.to_string()).field("url"));
            }
            url.to_owned()
        }
    };
    let template = input
        .default_template
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    if let Some(template) = template
        && let Err(error) = validate_template(template)
    {
        return Err(ApiError::invalid(error.to_string()).field("default_template"));
    }
    Ok(NewTarget {
        name,
        url,
        default_template: template.map(str::to_owned),
    })
}

/// List the targets, by name without regard to case, with their addresses hidden.
#[utoipa::path(
    get,
    path = "/notifications",
    tag = "notifications",
    security(("api_key" = [])),
    responses(
        (status = 200, description = "The targets", body = TargetList),
        (status = 401, description = "No valid token", body = ErrorBody),
    )
)]
pub(super) async fn list_targets(
    State(state): State<AppState>,
) -> Result<Json<TargetList>, ApiError> {
    let counts = state.store.linked_watch_counts().await?;
    let mut found = state.store.list_targets().await?;
    found.sort_by_key(|target| (target.name.to_lowercase(), target.id));
    let notifications = found
        .iter()
        .map(|target| view_of(target, counts.get(&target.id).copied().unwrap_or(0)))
        .collect();
    Ok(Json(TargetList { notifications }))
}

/// Make a target.
#[utoipa::path(
    post,
    path = "/notifications",
    tag = "notifications",
    security(("api_key" = [])),
    request_body = TargetInput,
    responses(
        (status = 201, description = "The target that was made", body = TargetView),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 409, description = "The name is taken", body = ErrorBody),
        (status = 422, description = "A field is not acceptable", body = ErrorBody),
    )
)]
pub(super) async fn create_target(
    State(state): State<AppState>,
    ApiJson(input): ApiJson<TargetInput>,
) -> Result<
    (
        StatusCode,
        [(header::HeaderName, String); 1],
        Json<TargetView>,
    ),
    ApiError,
> {
    let new = check(&input, None)?;
    let mut target = state.store.create_target(&new).await?;
    if input.enabled == Some(false) {
        state.store.set_target_enabled(target.id, false).await?;
        target.enabled = false;
    }
    let location = format!(
        "{}/api/v1/notifications/{}",
        state.web.base_path(),
        target.id
    );
    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(view_of(&target, 0)),
    ))
}

/// Read one target, with its address hidden.
#[utoipa::path(
    get,
    path = "/notifications/{id}",
    tag = "notifications",
    security(("api_key" = [])),
    params(("id" = Uuid, Path, description = "The target's id")),
    responses(
        (status = 200, description = "The target", body = TargetView),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 404, description = "No such target", body = ErrorBody),
    )
)]
pub(super) async fn get_target(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TargetView>, ApiError> {
    let id = parse_id(&id)?;
    let target = state.store.get_target(id).await?;
    Ok(Json(view_of(&target, watches_of(&state, id).await?)))
}

/// Replace a target's name, address and template. Which watches use it stays as it is.
#[utoipa::path(
    put,
    path = "/notifications/{id}",
    tag = "notifications",
    security(("api_key" = [])),
    params(("id" = Uuid, Path, description = "The target's id")),
    request_body = TargetInput,
    responses(
        (status = 200, description = "The target as it is now", body = TargetView),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 404, description = "No such target", body = ErrorBody),
        (status = 409, description = "The name is taken", body = ErrorBody),
        (status = 422, description = "A field is not acceptable", body = ErrorBody),
    )
)]
pub(super) async fn replace_target(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(input): ApiJson<TargetInput>,
) -> Result<Json<TargetView>, ApiError> {
    let id = parse_id(&id)?;
    let current = state.store.get_target(id).await?;
    let new = check(&input, Some(&current))?;
    let mut target = state.store.update_target(id, &new).await?;
    if let Some(enabled) = input.enabled
        && enabled != target.enabled
    {
        state.store.set_target_enabled(id, enabled).await?;
        target.enabled = enabled;
    }
    Ok(Json(view_of(&target, watches_of(&state, id).await?)))
}

/// Delete a target. Watches that used it stop notifying it.
#[utoipa::path(
    delete,
    path = "/notifications/{id}",
    tag = "notifications",
    security(("api_key" = [])),
    params(("id" = Uuid, Path, description = "The target's id")),
    responses(
        (status = 204, description = "The target is gone"),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 404, description = "No such target", body = ErrorBody),
    )
)]
pub(super) async fn delete_target(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.store.delete_target(parse_id(&id)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Send a test notification to a target, once, and say how it went. Disabled targets can be
/// tested. The result is written to the delivery log.
#[utoipa::path(
    post,
    path = "/notifications/{id}/test",
    tag = "notifications",
    security(("api_key" = [])),
    params(("id" = Uuid, Path, description = "The target's id")),
    responses(
        (status = 200, description = "The test ran; `ok` says whether it arrived", body = TestReport),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 404, description = "No such target", body = ErrorBody),
    )
)]
pub(super) async fn test_target(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TestReport>, ApiError> {
    let target = state.store.get_target(parse_id(&id)?).await?;
    // One attempt: a program is waiting for the answer.
    let report = state.notifier.test_once(&target).await.report;
    Ok(Json(TestReport {
        ok: report.result.is_ok(),
        error: report.result.err(),
        note: report.note,
    }))
}
