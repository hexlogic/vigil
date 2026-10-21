//! Reading the check log.

use sqlx::Row;
use vigil_core::WatchId;

use crate::{Store, StoreError};

/// How a logged check ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckOutcome {
    /// The text was the same as before, or the server answered 304.
    Unchanged,
    /// The text changed. The first check of a watch, which stores its baseline, is logged
    /// as a change too.
    Changed,
    /// The check failed.
    Error,
}

/// One row of a watch's check log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckEntry {
    /// When the check finished, in unix milliseconds.
    pub at: i64,
    /// How long it took.
    pub duration_ms: i64,
    /// How it ended.
    pub outcome: CheckOutcome,
    /// What went wrong, for a failed check.
    pub error: Option<String>,
}

impl Store {
    /// A watch's most recent checks, newest first, at most `limit`.
    ///
    /// The log keeps [`crate::CHECK_LOG_KEEP`] rows per watch.
    pub async fn list_checks(
        &self,
        watch: WatchId,
        limit: u32,
    ) -> Result<Vec<CheckEntry>, StoreError> {
        let rows = sqlx::query(
            "SELECT at, duration_ms, outcome, error_message FROM check_log \
             WHERE watch_id = ? ORDER BY id DESC LIMIT ?",
        )
        .bind(watch.to_string())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let outcome: String = row.try_get("outcome")?;
                Ok(CheckEntry {
                    at: row.try_get("at")?,
                    duration_ms: row.try_get("duration_ms")?,
                    outcome: match outcome.as_str() {
                        "unchanged" => CheckOutcome::Unchanged,
                        "changed" => CheckOutcome::Changed,
                        "error" => CheckOutcome::Error,
                        other => {
                            return Err(StoreError::Corrupt(format!(
                                "unknown check outcome {other:?}"
                            )));
                        }
                    },
                    error: row.try_get("error_message")?,
                })
            })
            .collect()
    }
}
