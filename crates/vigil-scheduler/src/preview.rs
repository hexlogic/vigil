//! Previewing a watch: what a check would store, without storing anything.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vigil_core::{LineDiff, Verdict, Watch, WatchId};
use vigil_fetch::FetchOutcome;
use vigil_store::{SnapshotMeta, StoreError};

use crate::check::{Checker, request_for};
use crate::describe::describe;
use crate::render::{render, validate};

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

/// Why a watch could not be previewed or validated.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct PreviewError {
    /// The number, counted from 1, of the pipeline step the problem is in; `None` for a
    /// problem with the page, the conditions or the processing as a whole.
    pub step: Option<usize>,
    /// What went wrong, in one line. A problem in a step starts with the step.
    pub message: String,
}

impl From<String> for PreviewError {
    fn from(message: String) -> Self {
        Self {
            step: None,
            message,
        }
    }
}

/// The outcome of [`Checker::preview`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewReport {
    /// How long fetching and processing took (only processing, for
    /// [`Checker::preview_page`]).
    pub duration: Duration,
    /// The preview, or what went wrong.
    pub result: Result<Preview, PreviewError>,
}

/// A page fetched for a preview, kept so that it can be previewed again with other settings
/// without fetching it again. Cheap to clone: the body is shared.
#[derive(Debug, Clone)]
pub struct FetchedPage {
    status: u16,
    content_type: Option<String>,
    body: Arc<Vec<u8>>,
}

impl FetchedPage {
    /// HTTP status of the response.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// `Content-Type` of the response.
    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    /// Size of the response body in bytes.
    pub fn size(&self) -> u64 {
        self.body.len() as u64
    }
}

impl Checker {
    /// Fetch and process `watch` as a check would, and compare the result with its latest
    /// snapshot, without recording anything.
    ///
    /// The watch may differ from the stored one (unsaved edits, command-line overrides); only
    /// its id is used, to find the snapshot to compare with. The request carries no validators
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

    async fn preview_inner(
        &self,
        watch: &Watch,
    ) -> Result<Result<Preview, PreviewError>, StoreError> {
        // Nothing is requested from a site for a watch whose settings do not build.
        if let Err(error) = self.validate_watch(watch, watch.id).await {
            return Ok(Err(error));
        }
        match self.fetch_for_preview(watch).await {
            Ok(page) => self.preview_of(watch, &page, watch.id).await,
            Err(message) => Ok(Err(message.into())),
        }
    }

    /// Check that the watch's conditions and pipeline build, without fetching anything. An
    /// error in a step says which one.
    ///
    /// `slot` is the key under which the pipeline is built on its own thread, of which at most
    /// one per key is alive at a time. Pass a key that belongs to the preview, not the id of
    /// a watch that is also checked on a schedule: a check refused for want of the slot would
    /// be recorded as a failure.
    pub async fn validate_watch(&self, watch: &Watch, slot: WatchId) -> Result<(), PreviewError> {
        if let Err(error) = watch.conditions.compile() {
            return Err(format!("conditions: {}", describe(&error)).into());
        }
        validate(slot, watch, self.render_timeout()).await
    }

    /// Fetch the page of a watch for [`preview_page`](Self::preview_page). Fetch problems
    /// come back as one line.
    pub async fn fetch_for_preview(&self, watch: &Watch) -> Result<FetchedPage, String> {
        match self.fetch(&request_for(watch, false)).await? {
            FetchOutcome::Body(body) => Ok(FetchedPage {
                status: body.status,
                content_type: body.content_type,
                body: Arc::new(body.body),
            }),
            FetchOutcome::NotModified { .. } => Err("the server answered 304 Not Modified, so \
                                                     there is no content to preview (is a \
                                                     conditional header set?)"
                .to_owned()),
        }
    }

    /// Process an already fetched `page` with the settings of `watch` and compare the result
    /// with the watch's latest snapshot, as [`preview`](Self::preview) does after fetching.
    /// Nothing is recorded. `slot` is as for [`validate_watch`](Self::validate_watch).
    pub async fn preview_page(
        &self,
        watch: &Watch,
        page: &FetchedPage,
        slot: WatchId,
    ) -> Result<PreviewReport, StoreError> {
        let started = Instant::now();
        let result = self.preview_of(watch, page, slot).await;
        let duration = started.elapsed();
        Ok(PreviewReport {
            duration,
            result: result?,
        })
    }

    async fn preview_of(
        &self,
        watch: &Watch,
        page: &FetchedPage,
        slot: WatchId,
    ) -> Result<Result<Preview, PreviewError>, StoreError> {
        let conditions = match watch.conditions.compile() {
            Ok(conditions) => conditions,
            Err(error) => {
                return Ok(Err(format!("conditions: {}", describe(&error)).into()));
            }
        };
        let text = match render(
            slot,
            watch,
            Arc::clone(&page.body),
            page.content_type.clone(),
            self.render_timeout(),
        )
        .await
        {
            Ok(text) => text,
            Err(error) => return Ok(Err(error)),
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
            http_status: page.status,
            content_type: page.content_type.clone(),
            raw_size: page.size(),
            latest,
            diff,
            verdict,
        }))
    }
}
