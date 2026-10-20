//! Previewing a watch: what a check would store, without storing anything.

use std::time::{Duration, Instant};

use vigil_core::{LineDiff, Verdict, Watch};
use vigil_fetch::FetchOutcome;
use vigil_store::{SnapshotMeta, StoreError};

use crate::check::{Checker, describe, render, request_for, validate};

/// What a check of a watch would have stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// The processed text.
    pub text: String,
    /// HTTP status of the response.
    pub http_status: u16,
    /// `Content-Type` of the response.
    pub content_type: Option<String>,
    /// Size of the response body in bytes, before processing.
    pub raw_size: u64,
    /// The newest stored snapshot of the watch, if it has one.
    pub latest: Option<SnapshotMeta>,
    /// How the text differs from the latest snapshot; `None` without one.
    pub diff: Option<LineDiff>,
    /// What the conditions would decide about that difference; `None` unless the text differs.
    pub verdict: Option<Verdict>,
}

/// The outcome of [`Checker::preview`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewReport {
    /// How long fetching and processing took.
    pub duration: Duration,
    /// The preview, or what went wrong.
    pub result: Result<Preview, String>,
}

impl Checker {
    /// Fetch and process `watch` as a check would, and compare the result with its latest
    /// snapshot, without recording anything.
    ///
    /// The watch may differ from the stored one (unsaved edits, command-line overrides); only
    /// its id is used to find the snapshot to compare with. The request carries no validators
    /// (`If-None-Match`, `If-Modified-Since`): a preview needs the body. Problems with the
    /// page or the configuration come back in the report; `Err` means the store failed.
    pub async fn preview(&self, watch: &Watch) -> Result<PreviewReport, StoreError> {
        let started = Instant::now();
        let result = self.preview_inner(watch).await;
        let duration = started.elapsed();
        Ok(PreviewReport {
            duration,
            result: result?,
        })
    }

    async fn preview_inner(&self, watch: &Watch) -> Result<Result<Preview, String>, StoreError> {
        let conditions = match watch.conditions.compile() {
            Ok(conditions) => conditions,
            Err(error) => return Ok(Err(format!("conditions: {}", describe(&error)))),
        };
        if let Err(message) = validate(watch, self.render_timeout()).await {
            return Ok(Err(message));
        }
        let body = match self.fetch(&request_for(watch, false)).await {
            Ok(FetchOutcome::Body(body)) => body,
            Ok(FetchOutcome::NotModified { .. }) => {
                return Ok(Err("the server answered 304 Not Modified, so there is no \
                                content to preview (is a conditional header set?)"
                    .to_owned()));
            }
            Err(message) => return Ok(Err(message)),
        };
        let raw_size = body.body.len() as u64;
        let text = match render(
            watch,
            body.body,
            body.content_type.clone(),
            self.render_timeout(),
        )
        .await
        {
            Ok(text) => text,
            Err(message) => return Ok(Err(message)),
        };

        let latest = self.store().list_snapshots(watch.id, 1).await?.pop();
        let diff = match &latest {
            Some(meta) => Some(LineDiff::between(
                &self.store().snapshot_content(meta.id).await?,
                &text,
            )),
            None => None,
        };
        let verdict = diff
            .as_ref()
            .filter(|diff| !diff.is_empty())
            .map(|diff| conditions.evaluate(diff));
        Ok(Ok(Preview {
            text,
            http_status: body.status,
            content_type: body.content_type,
            raw_size,
            latest,
            diff,
            verdict,
        }))
    }
}
