//! One check of one watch: fetch, decode, process, normalise, record.

use std::error::Error;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use vigil_core::decode::decode;
use vigil_core::normalize::normalize;
use vigil_core::process::process;
use vigil_core::{LineDiff, Watch, WatchId};
use vigil_fetch::{FetchOutcome, FetchRequest, Fetcher, host_key};
use vigil_store::{CheckSuccess, Content, RecordOutcome, SnapshotId, Store, StoreError};

use crate::backoff::{MAX_INTERVAL, default_jitter};
use crate::scheduler::{Backend, Job, RunOutcome};

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
}

impl Checker {
    /// Build a checker.
    pub fn new(store: Store, fetcher: Fetcher) -> Self {
        Self { store, fetcher }
    }

    /// Check one watch now and record the result.
    ///
    /// Problems with the page (network, status, processing) are recorded on the watch and come
    /// back as [`CheckResult::Failed`]. `Err` means the store itself failed, including when the
    /// watch does not exist.
    pub async fn check(&self, id: WatchId) -> Result<CheckReport, StoreError> {
        let watch = self.store.get_watch(id).await?;
        let started = Instant::now();
        let fetched = self.fetch_and_process(&watch).await;
        let duration = started.elapsed();
        let duration_ms = i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
        let at = now_ms();

        let result = match fetched {
            Ok(mut success) => {
                success.at = at;
                success.duration_ms = duration_ms;
                self.record(id, &success).await?
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
            CheckResult::Changed { diff, .. } => tracing::info!(
                watch = %id,
                url = %watch.url,
                added = diff.added_count(),
                removed = diff.removed_count(),
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

    async fn record(&self, id: WatchId, success: &CheckSuccess) -> Result<CheckResult, StoreError> {
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
                CheckResult::Changed {
                    previous,
                    current,
                    diff: LineDiff::between(&previous_text, new_text),
                }
            }
        })
    }

    /// Fetch and process the page. `at` and `duration_ms` of the result are placeholders that
    /// the caller fills in once the work is done.
    async fn fetch_and_process(&self, watch: &Watch) -> Result<CheckSuccess, String> {
        let request = FetchRequest {
            url: watch.url.clone(),
            timeout: watch.fetch.timeout_secs.map(Duration::from_secs),
            user_agent: watch.fetch.user_agent.clone(),
            headers: watch
                .fetch
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            etag: watch.state.etag.clone(),
            last_modified: watch.state.last_modified.clone(),
        };
        match self
            .fetcher
            .fetch(&request)
            .await
            .map_err(|e| describe(&e))?
        {
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
            }),
            FetchOutcome::Body(body) => {
                let content_type = body.content_type.as_deref();
                let text = decode(&body.body, content_type);
                let processed =
                    process(watch.processor, &text, content_type).map_err(|e| describe(&e))?;
                Ok(CheckSuccess {
                    at: 0,
                    duration_ms: 0,
                    http_status: Some(body.status),
                    content_type: body.content_type.clone(),
                    etag: body.etag.clone(),
                    last_modified: body.last_modified.clone(),
                    content: Some(Content {
                        text: normalize(&processed),
                        raw_size: body.body.len() as u64,
                    }),
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

/// An error and its causes on one line: `outer: inner: root`.
///
/// Many errors already include their cause in their own message (`#[error("...: {0}")]`
/// with `#[from]`); a cause whose text is already present is not repeated.
fn describe(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(inner) = source {
        let inner_text = inner.to_string();
        if !text.contains(&inner_text) {
            text.push_str(": ");
            text.push_str(&inner_text);
        }
        source = inner.source();
    }
    text
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::describe;

    #[derive(Debug)]
    struct Leaf(&'static str);
    impl fmt::Display for Leaf {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }
    impl std::error::Error for Leaf {}

    /// Mentions its cause in its own message, like `#[error("...: {0}")]` with `#[from]`.
    #[derive(Debug)]
    struct Embedding(Leaf);
    impl fmt::Display for Embedding {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "outer failed: {}", self.0)
        }
    }
    impl std::error::Error for Embedding {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    /// Leaves its cause to the source chain, like `FetchError::Http`.
    #[derive(Debug)]
    struct Chained(Leaf);
    impl fmt::Display for Chained {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("request failed")
        }
    }
    impl std::error::Error for Chained {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn a_plain_error_is_just_its_message() {
        assert_eq!(describe(&Leaf("boom")), "boom");
    }

    #[test]
    fn causes_are_appended_when_the_message_leaves_them_out() {
        assert_eq!(
            describe(&Chained(Leaf("connection refused"))),
            "request failed: connection refused"
        );
    }

    #[test]
    fn causes_already_in_the_message_are_not_repeated() {
        assert_eq!(
            describe(&Embedding(Leaf("inner boom"))),
            "outer failed: inner boom"
        );
    }
}
