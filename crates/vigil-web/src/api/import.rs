//! `POST /api/v1/import`: read a file of watches and, unless it is a dry run, make them.

use std::time::Duration;

use axum::Json;
use axum::extract::{Query, Request, State};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use vigil_import::{Existing, Format, ImportError, Options, Report, plan_reader};
use vigil_scheduler::parse_interval;

use super::error::{ApiError, ErrorBody};
use crate::AppState;
use crate::upload::{Upload, UploadError};

/// How the file is read.
#[derive(Debug, Deserialize, IntoParams)]
pub(super) struct ImportQuery {
    /// What the file is: `url-list`, `opml`, `backup`, `urlwatch` or `distill`. Guessed from
    /// the content when left out.
    format: Option<String>,
    /// Only say what would be imported; change nothing.
    #[serde(default)]
    dry_run: bool,
    /// Make a watch even when one with its address exists.
    #[serde(default)]
    allow_duplicates: bool,
    /// The interval of sources that have none of their own, such as `30m` (default `1h`).
    every: Option<String>,
}

/// What an import made.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct Made {
    /// The ids of the watches that were made, in the order of the file; watches that were
    /// skipped are not listed.
    watches: Vec<Uuid>,
    /// How many tags were made.
    tags: usize,
    /// How many notification targets were made.
    targets: usize,
}

/// The answer to an import.
#[derive(Debug, Serialize, ToSchema)]
pub(super) struct ImportResponse {
    /// Whether this was only a preview.
    dry_run: bool,
    /// What was read, what will be made (or was), and what could not be carried over.
    report: Report,
    /// What was made; `null` for a dry run.
    made: Option<Made>,
}

fn import_error(error: ImportError) -> ApiError {
    match error {
        ImportError::TooLarge { limit } => ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            format!("the file is larger than the {limit} byte limit"),
        ),
        ImportError::UnknownFormat => ApiError::invalid(error.to_string()).code("unknown_format"),
        ImportError::Invalid { .. } | ImportError::TooMany { .. } => {
            ApiError::invalid(error.to_string()).code("unreadable_file")
        }
    }
}

fn upload_error(error: UploadError) -> ApiError {
    match error {
        UploadError::TooLarge { limit } => ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            format!("the upload is larger than the {limit} byte limit"),
        ),
        UploadError::Io(error) => ApiError::internal(&error),
    }
}

/// Read a file of watches: a list of addresses, an OPML outline, a backup of another
/// self-hosted change monitor (a ZIP or its older JSON file), a urlwatch `urls.yaml` or a
/// Distill JSON export. Send the file itself as the request body. The answer is a report of
/// what was read and what could not be carried over; with `dry_run=true` nothing is made.
/// History is not imported: the first check of each watch is its baseline.
#[utoipa::path(
    post,
    path = "/import",
    tag = "import",
    security(("api_key" = [])),
    params(ImportQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "The file to import"),
    responses(
        (status = 200, description = "The report, and what was made unless it was a dry run", body = ImportResponse),
        (status = 401, description = "No valid token", body = ErrorBody),
        (status = 413, description = "The file is too large", body = ErrorBody),
        (status = 422, description = "The file or an option cannot be read", body = ErrorBody),
    )
)]
pub(super) async fn import(
    State(state): State<AppState>,
    Query(query): Query<ImportQuery>,
    request: Request,
) -> Result<Json<ImportResponse>, ApiError> {
    let format = match query
        .format
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
    {
        None => None,
        Some(text) => Some(
            text.parse::<Format>()
                .map_err(|error| ApiError::invalid(error.to_string()).field("format"))?,
        ),
    };
    let every = match query
        .every
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
    {
        None => Duration::from_secs(3600),
        Some(text) => parse_interval(text)
            .map_err(|error| ApiError::invalid(error.to_string()).field("every"))?,
    };
    let limit = state.web.max_upload_bytes;
    // Refuse at once what says it is too large; what does not say is counted as it arrives.
    let declared = request
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|len| len > limit) {
        return Err(upload_error(UploadError::TooLarge { limit }));
    }

    let mut upload =
        Upload::new(&state.web.upload_dir, limit).map_err(|e| ApiError::internal(&e))?;
    let mut body = request.into_body();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| {
            ApiError::new(
                axum::http::StatusCode::BAD_REQUEST,
                format!("the upload was cut off: {error}"),
            )
        })?;
        if let Ok(data) = frame.into_data() {
            upload.push(&data).await.map_err(upload_error)?;
        }
    }
    let spooled = upload.finish().await.map_err(|e| ApiError::internal(&e))?;

    let existing = Existing::new(
        &state.store.list_tags().await?,
        &state.store.tag_target_ids().await?,
        &state.store.list_targets().await?,
        &state.store.list_watches().await?,
    );
    let options = Options {
        every,
        allow_duplicates: query.allow_duplicates,
    };
    let reader = spooled.reader().map_err(|e| ApiError::internal(&e))?;
    let len = spooled.len;
    // Reading a large archive takes a while and is not async: it gets a thread of its own.
    let plan =
        tokio::task::spawn_blocking(move || plan_reader(reader, len, format, &options, &existing))
            .await
            .map_err(|e| ApiError::internal(&e))?
            .map_err(import_error)?;
    drop(spooled);

    if query.dry_run {
        return Ok(Json(ImportResponse {
            dry_run: true,
            report: plan.report,
            made: None,
        }));
    }
    let outcome = state.store.apply_import(&plan.batch).await?;
    if let Err(error) = state.scheduler.reload().await {
        tracing::warn!(%error, "the scheduler could not be told about the imported watches");
    }
    Ok(Json(ImportResponse {
        dry_run: false,
        report: plan.report,
        made: Some(Made {
            watches: outcome
                .watches
                .iter()
                .flatten()
                .map(|id| id.as_uuid())
                .collect(),
            tags: outcome.tags_created,
            targets: outcome.targets_created,
        }),
    }))
}
