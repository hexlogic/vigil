//! One check of one watch: fetch, render through the pipeline, record, judge.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use vigil_core::{CompiledConditions, LineDiff, Verdict, Watch, WatchId};
use vigil_fetch::{FetchOutcome, FetchRequest, Fetcher, host_key};
use vigil_store::{CheckSuccess, Content, RecordOutcome, SnapshotId, Store, StoreError};

use crate::backoff::{MAX_INTERVAL, default_jitter};
use crate::describe::describe;
use crate::render::{render, validate};
use crate::scheduler::{Backend, Job, RunOutcome};

/// How long processing one response may take before the check fails.
const RENDER_TIMEOUT: Duration = Duration::from_secs(30);

/// Shortest interval the scheduler accepts, so a zero interval cannot spin. The longest is
/// [`MAX_INTERVAL`].
const MIN_INTERVAL: Duration = Duration::from_secs(1);

/// What one check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    /// First successful check: the baseline snapshot was stored.
    Baseline,
    /// Same content as before, or the server answered 304.
    Unchanged,
    /// The content differs from the previous snapshot.
    Changed {
        /// Snapshot that was current before this check.
        previous: SnapshotId,
        /// The new snapshot.
        current: SnapshotId,
        /// Line diff from the previous to the new text.
        diff: LineDiff,
        /// Whether the watch's conditions want a notification for this change. The change is
        /// stored either way.
        verdict: Verdict,
    },
    /// The page could not be fetched or processed; the error was recorded.
    Failed {
        /// What went wrong.
        message: String,
        /// Failed checks in a row, including this one.
        consecutive_errors: u32,
    },
}

/// The outcome of [`Checker::check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    /// The watch that was checked.
    pub watch: WatchId,
    /// How long fetching and processing took.
    pub duration: Duration,
    /// What was found.
    pub result: CheckResult,
}

/// Runs checks against the store using a shared fetcher.
#[derive(Debug, Clone)]
pub struct Checker {
    store: Store,
    fetcher: Fetcher,
    render_timeout: Duration,
}

impl Checker {
    /// Build a checker.
    pub fn new(store: Store, fetcher: Fetcher) -> Self {
        Self {
            store,
            fetcher,
            render_timeout: RENDER_TIMEOUT,
        }
    }

    /// Change how long processing one response may take (30 seconds by default).
    #[must_use]
    pub fn with_render_timeout(mut self, timeout: Duration) -> Self {
        self.render_timeout = timeout;
        self
    }

    /// Check one watch now and record the result.
    ///
    /// Problems with the page (network, status, processing) are recorded on the watch and come
    /// back as [`CheckResult::Failed`]. `Err` means the store itself failed, including when the
    /// watch does not exist.
    pub async fn check(&self, id: WatchId) -> Result<CheckReport, StoreError> {
        let watch = self.store.get_watch(id).await?;
        let started = Instant::now();
        let fetched = match watch.conditions.compile() {
            Ok(conditions) => self
                .fetch_and_process(&watch)
                .await
                .map(|success| (success, conditions)),
            Err(error) => Err(format!("conditions: {}", describe(&error))),
        };
        let duration = started.elapsed();
        let duration_ms = i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
        let at = now_ms();

        let result = match fetched {
            Ok((mut success, conditions)) => {
                success.at = at;
                success.duration_ms = duration_ms;
                self.record(id, &success, &conditions).await?
            }
            Err(message) => {
                let consecutive_errors = self
                    .store
                    .record_error(id, at, duration_ms, &message)
                    .await?;
                tracing::warn!(watch = %id, url = %watch.url, %message, "check failed");
                CheckResult::Failed {
                    message,
                    consecutive_errors,
                }
            }
        };
        match &result {
            CheckResult::Baseline => {
                tracing::info!(watch = %id, url = %watch.url, "baseline stored")
            }
            CheckResult::Changed { diff, verdict, .. } => tracing::info!(
                watch = %id,
                url = %watch.url,
                added = diff.added_count(),
                removed = diff.removed_count(),
                notify = verdict.notify,
                reason = %verdict.reason,
                "change detected"
            ),
            CheckResult::Unchanged => tracing::debug!(watch = %id, "unchanged"),
            CheckResult::Failed { .. } => {}
        }
        Ok(CheckReport {
            watch: id,
            duration,
            result,
        })
    }

    async fn record(
        &self,
        id: WatchId,
        success: &CheckSuccess,
        conditions: &CompiledConditions,
    ) -> Result<CheckResult, StoreError> {
        Ok(match self.store.record_success(id, success).await? {
            RecordOutcome::Baseline { .. } => CheckResult::Baseline,
            RecordOutcome::Unchanged => CheckResult::Unchanged,
            RecordOutcome::Changed {
                previous,
                current,
                previous_text,
            } => {
                // Diff against the text the store read before pruning: with a retention of 1
                // the previous snapshot is already gone by now.
                let new_text = success.content.as_ref().map_or("", |c| c.text.as_str());
                let diff = LineDiff::between(&previous_text, new_text);
                let verdict = conditions.evaluate(&diff);
                CheckResult::Changed {
                    previous,
                    current,
                    diff,
                    verdict,
                }
            }
        })
    }

    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    pub(crate) fn render_timeout(&self) -> Duration {
        self.render_timeout
    }

    /// Fetch a page, with errors flattened to one line for the check log or a report.
    pub(crate) async fn fetch(&self, request: &FetchRequest) -> Result<FetchOutcome, String> {
        self.fetcher.fetch(request).await.map_err(|e| describe(&e))
    }

    /// Fetch and process the page. `at` and `duration_ms` of the result are placeholders that
    /// the caller fills in once the work is done.
    async fn fetch_and_process(&self, watch: &Watch) -> Result<CheckSuccess, String> {
        validate(watch, self.render_timeout).await?;
        let request = request_for(watch, true);
        match self.fetch(&request).await? {
            FetchOutcome::NotModified {
                etag,
                last_modified,
            } => Ok(CheckSuccess {
                at: 0,
                duration_ms: 0,
                http_status: Some(304),
                content_type: None,
                etag,
                last_modified,
                content: None,
                settings_version: Some(watch.updated_at),
            }),
            FetchOutcome::Body(body) => {
                let raw_size = body.body.len() as u64;
                let text = render(
                    watch,
                    body.body,
                    body.content_type.clone(),
                    self.render_timeout,
                )
                .await?;
                Ok(CheckSuccess {
                    at: 0,
                    duration_ms: 0,
                    http_status: Some(body.status),
                    content_type: body.content_type,
                    etag: body.etag,
                    last_modified: body.last_modified,
                    content: Some(Content { text, raw_size }),
                    settings_version: Some(watch.updated_at),
                })
            }
        }
    }
}

impl Backend for Checker {
    async fn load(&self) -> Result<Vec<Job>, String> {
        let watches = self.store.list_watches().await.map_err(|e| describe(&e))?;
        Ok(watches
            .into_iter()
            .filter(|watch| !watch.paused)
            .map(job_from)
            .collect())
    }

    async fn run(&self, id: WatchId) -> RunOutcome {
        match self.check(id).await {
            Ok(report) => RunOutcome::Done {
                consecutive_errors: match report.result {
                    CheckResult::Failed {
                        consecutive_errors, ..
                    } => consecutive_errors,
                    _ => 0,
                },
            },
            Err(StoreError::NotFound(_)) => RunOutcome::Gone,
            Err(error) => {
                tracing::error!(watch = %id, error = %describe(&error), "recording check failed");
                RunOutcome::Failed
            }
        }
    }
}

/// The request for a watch.
///
/// With `conditional`, the validators from the last response are sent, so the server can
/// answer 304. A preview leaves them out: it needs a body.
pub(crate) fn request_for(watch: &Watch, conditional: bool) -> FetchRequest {
    FetchRequest {
        url: watch.url.clone(),
        timeout: watch.fetch.timeout_secs.map(Duration::from_secs),
        user_agent: watch.fetch.user_agent.clone(),
        headers: watch
            .fetch
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        etag: conditional.then(|| watch.state.etag.clone()).flatten(),
        last_modified: conditional
            .then(|| watch.state.last_modified.clone())
            .flatten(),
    }
}

fn job_from(watch: Watch) -> Job {
    let interval = watch.schedule.interval().clamp(MIN_INTERVAL, MAX_INTERVAL);
    let jitter = if watch.jitter_secs > 0 {
        Duration::from_secs(u64::from(watch.jitter_secs))
    } else {
        default_jitter(interval)
    };
    Job {
        id: watch.id,
        host: host_key(&watch.url),
        interval,
        jitter,
        last_checked_at: watch.state.last_checked_at,
        consecutive_errors: watch.state.consecutive_errors,
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}
