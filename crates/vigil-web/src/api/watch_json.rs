//! The JSON of a watch: what the API sends, what it accepts, and the checks in between.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use vigil_core::{
    Conditions, FetchSettings, FetchSettingsError, Processor, Schedule, Step, Watch, WatchId,
};
use vigil_fetch::check_url;
use vigil_notify::{describe_error, validate_template};
use vigil_scheduler::backoff::{MAX_INTERVAL, MIN_INTERVAL};
use vigil_store::{NewWatch, Tag};

use super::error::ApiError;
use crate::AppState;
use crate::fmt::rfc3339;
use crate::form::build::fetch::{TIMEOUT_SECS, header_is_valid};
use crate::form::preview_watch;

/// Interval when the request names none and no tag has a default.
const DEFAULT_EVERY_SECS: u64 = 3600;

/// Snapshots kept when the request does not say.
const DEFAULT_RETENTION: u32 = 100;

/// What a check has found so far. Read-only.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct StateView {
    /// When the watch was last checked, successfully or not (RFC 3339, UTC).
    last_checked_at: Option<String>,
    /// When the text last changed (RFC 3339, UTC).
    last_changed_at: Option<String>,
    /// The message of the last failed check; empty after a success.
    last_error: Option<String>,
    /// Failed checks in a row.
    consecutive_errors: u32,
}

/// A watch, as the API sends it.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct WatchView {
    /// The watch's id.
    id: WatchId,
    /// A name for people; the address when none was given.
    name: String,
    /// The address that is watched.
    url: String,
    /// A paused watch is not checked on a schedule, but can be checked on request.
    paused: bool,
    /// When it is checked.
    schedule: Schedule,
    /// Most of a random delay added to every scheduled check, in seconds; 0 uses a tenth of
    /// the interval, at most a minute.
    jitter_secs: u32,
    /// How the page is requested.
    fetch: FetchSettings,
    /// How the response becomes text.
    processor: Processor,
    /// Steps applied after the processor, in order.
    pipeline: Vec<Step>,
    /// Which changes notify.
    conditions: Conditions,
    /// How many snapshots are kept.
    snapshot_retention: u32,
    /// The tags of the watch.
    tags: Vec<Uuid>,
    /// The notification targets the watch notifies, besides those of its tags.
    targets: Vec<Uuid>,
    /// A message template for this watch; empty uses the target's or the built-in one.
    notification_template: Option<String>,
    /// When the watch was added (RFC 3339, UTC).
    created_at: String,
    /// When its settings last changed (RFC 3339, UTC).
    updated_at: String,
    /// What the checks found.
    state: StateView,
}

impl WatchView {
    /// The view of `watch` with these tags and targets.
    pub(super) fn new(watch: &Watch, tags: Vec<Uuid>, targets: Vec<Uuid>) -> Self {
        let at = |ms: Option<i64>| ms.map(rfc3339);
        Self {
            id: watch.id,
            name: watch.name.clone(),
            url: watch.url.clone(),
            paused: watch.paused,
            schedule: watch.schedule,
            jitter_secs: watch.jitter_secs,
            fetch: watch.fetch.clone(),
            processor: watch.processor,
            pipeline: watch.pipeline.clone(),
            conditions: watch.conditions.clone(),
            snapshot_retention: watch.snapshot_retention,
            tags,
            targets,
            notification_template: watch.notification_template.clone(),
            created_at: rfc3339(watch.created_at),
            updated_at: rfc3339(watch.updated_at),
            state: StateView {
                last_checked_at: at(watch.state.last_checked_at),
                last_changed_at: at(watch.state.last_changed_at),
                last_error: watch.state.last_error.clone(),
                consecutive_errors: watch.state.consecutive_errors,
            },
        }
    }
}

/// A watch, as the API accepts it. `PUT` replaces the whole watch, so a field that is left
/// out gets its default, not the value it had.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WatchInput {
    /// The address to watch (`http` or `https`).
    url: String,
    /// A name for people; defaults to the address.
    #[serde(default)]
    name: Option<String>,
    /// Do not check on a schedule.
    #[serde(default)]
    paused: bool,
    /// When to check, as `{"interval": 300}` (seconds, from 1 to 365 days). Defaults to the
    /// shortest default of the watch's tags, else an hour.
    #[serde(default)]
    schedule: Option<Schedule>,
    /// Most of a random delay added to every scheduled check, in seconds.
    #[serde(default)]
    jitter_secs: u32,
    /// How the page is requested.
    #[serde(default)]
    fetch: FetchSettings,
    /// How the response becomes text: `text` (the default), `json` or `feed`.
    #[serde(default)]
    processor: Processor,
    /// Steps applied after the processor, in order.
    #[serde(default)]
    pipeline: Vec<Step>,
    /// Which changes notify.
    #[serde(default)]
    conditions: Conditions,
    /// How many snapshots to keep, at least 1. Defaults to 100.
    #[serde(default)]
    snapshot_retention: Option<u32>,
    /// Ids of the tags of the watch.
    #[serde(default)]
    tags: Vec<Uuid>,
    /// Ids of the notification targets of the watch.
    #[serde(default)]
    targets: Vec<Uuid>,
    /// A message template for this watch.
    #[serde(default)]
    notification_template: Option<String>,
}

/// A request that passed every check.
pub(super) struct Built {
    pub(super) new: NewWatch,
    pub(super) paused: bool,
    pub(super) tags: Vec<Uuid>,
    pub(super) targets: Vec<Uuid>,
    pub(super) template: Option<String>,
}

/// Check what a request says and turn it into a watch. A problem is a `422` that names the
/// field. The pipeline and the conditions are built as a check would build them, so a request
/// that passes is a watch that works.
pub(super) async fn build(state: &AppState, input: WatchInput) -> Result<Built, ApiError> {
    let url = input.url.trim().to_owned();
    if url.is_empty() {
        return Err(ApiError::invalid("the address is empty").field("url"));
    }
    if let Err(error) = check_url(&url) {
        return Err(ApiError::invalid(describe_error(&error)).field("url"));
    }

    let all_tags = state.store.list_tags().await?;
    let tags = existing(&input.tags, all_tags.iter().map(|t| t.id), "tags")?;
    let targets = existing(
        &input.targets,
        state.store.list_targets().await?.iter().map(|t| t.id),
        "targets",
    )?;

    let every = match input.schedule {
        Some(Schedule::Interval(secs)) => {
            let every = std::time::Duration::from_secs(secs);
            if every < MIN_INTERVAL || every > MAX_INTERVAL {
                return Err(ApiError::invalid(
                    "the interval must be between 1 second and 365 days",
                )
                .field("schedule"));
            }
            secs
        }
        None => default_interval(&all_tags, &tags),
    };
    let retention = input.snapshot_retention.unwrap_or(DEFAULT_RETENTION);
    if retention == 0 {
        return Err(ApiError::invalid("keep at least one snapshot").field("snapshot_retention"));
    }
    check_fetch(&input.fetch)?;
    if let Some(template) = &input.notification_template
        && !template.is_empty()
        && let Err(error) = validate_template(template)
    {
        return Err(ApiError::invalid(error.to_string()).field("notification_template"));
    }

    let mut new = NewWatch::new(&url, Schedule::Interval(every));
    new.name = input
        .name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or(url);
    new.jitter_secs = input.jitter_secs;
    new.fetch = input.fetch;
    new.processor = input.processor;
    new.pipeline = input.pipeline;
    new.conditions = input.conditions;
    new.snapshot_retention = retention;

    // A slot of its own: the pipeline is built on a thread of which one per key is alive.
    let slot = WatchId::from_uuid(Uuid::now_v7());
    if let Err(error) = state
        .checker
        .validate_watch(&preview_watch(&new, slot), slot)
        .await
    {
        let field = if error.message.starts_with("conditions") {
            "conditions"
        } else {
            "pipeline"
        };
        return Err(ApiError::invalid(error.message).field(field));
    }

    Ok(Built {
        new,
        paused: input.paused,
        tags,
        targets,
        template: input.notification_template.filter(|t| !t.is_empty()),
    })
}

/// The shortest default interval of the chosen tags, else an hour.
fn default_interval(all: &[Tag], chosen: &[Uuid]) -> u64 {
    all.iter()
        .filter(|tag| chosen.contains(&tag.id))
        .filter_map(|tag| tag.default_interval_secs)
        .min()
        .unwrap_or(DEFAULT_EVERY_SECS)
}

/// `chosen` without repeats, each of which has to be among `known`.
fn existing(
    chosen: &[Uuid],
    known: impl Iterator<Item = Uuid>,
    field: &str,
) -> Result<Vec<Uuid>, ApiError> {
    let known: HashSet<Uuid> = known.collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in chosen {
        if !known.contains(id) {
            return Err(ApiError::invalid(format!("{field}: {id} does not exist")).field(field));
        }
        if seen.insert(*id) {
            out.push(*id);
        }
    }
    Ok(out)
}

fn check_fetch(fetch: &FetchSettings) -> Result<(), ApiError> {
    if let Some(timeout) = fetch.timeout_secs
        && !TIMEOUT_SECS.contains(&timeout)
    {
        return Err(
            ApiError::invalid("the timeout must be from 1 to 3600 seconds")
                .field("fetch.timeout_secs"),
        );
    }
    // The message names the header and never its value, which often holds a token.
    if let Some((name, _)) = fetch.headers.iter().find(|(n, v)| !header_is_valid(n, v)) {
        return Err(
            ApiError::invalid(format!("the header {name:?} cannot be sent")).field("fetch.headers"),
        );
    }
    fetch.validate().map_err(|error| {
        let field = match error {
            FetchSettingsError::BodyWithGet | FetchSettingsError::BodyTooLarge => "fetch.body",
        };
        ApiError::invalid(error.to_string()).field(field)
    })
}
