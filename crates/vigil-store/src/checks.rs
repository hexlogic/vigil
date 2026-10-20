//! Recording the result of a check: snapshots with hash dedupe, watch state and the check log.

use sqlx::Row;
use uuid::Uuid;
use vigil_core::{ContentHash, WatchId};

use crate::snapshots::{SnapshotId, compress, decompress};
use crate::{Store, StoreError};

/// Check-log rows kept per watch.
pub const CHECK_LOG_KEEP: i64 = 500;

/// Longest error message stored in the check log, in characters.
const MAX_ERROR_CHARS: usize = 1000;

/// Processed text of a fetched body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Content {
    /// The processed, normalised text that is hashed and diffed.
    pub text: String,
    /// Size of the fetched body in bytes, before processing.
    pub raw_size: u64,
}

/// A check that reached the server and got a usable answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSuccess {
    /// When the check finished, unix milliseconds.
    pub at: i64,
    /// How long it took.
    pub duration_ms: i64,
    /// HTTP status of the response.
    pub http_status: Option<u16>,
    /// `Content-Type` of the response.
    pub content_type: Option<String>,
    /// `ETag` of the response. Replaces the stored value, except on a 304 where `None` keeps it.
    pub etag: Option<String>,
    /// `Last-Modified` of the response, handled like `etag`.
    pub last_modified: Option<String>,
    /// The new text, or `None` when the server answered 304 Not Modified.
    pub content: Option<Content>,
}

/// What recording a check did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOutcome {
    /// First successful check: the first snapshot was stored, there is nothing to compare to.
    Baseline {
        /// The new snapshot.
        snapshot: SnapshotId,
    },
    /// The text differs from the previous snapshot.
    Changed {
        /// The snapshot that was current before this check. Retention may already have
        /// pruned it, so use `previous_text` rather than reading it back.
        previous: SnapshotId,
        /// The new snapshot.
        current: SnapshotId,
        /// The text of `previous`, read before pruning.
        previous_text: String,
    },
    /// The text is identical, or the server answered 304.
    Unchanged,
}

impl Store {
    /// Record a successful check in one transaction.
    ///
    /// A snapshot is stored only when the text hash differs from the watch's `last_hash`.
    /// Older snapshots beyond the watch's retention (at least one is always kept) and check-log
    /// rows beyond [`CHECK_LOG_KEEP`] are pruned.
    pub async fn record_success(
        &self,
        watch: WatchId,
        check: &CheckSuccess,
    ) -> Result<RecordOutcome, StoreError> {
        let id = watch.to_string();
        // Take the write lock up front so a concurrent `vigil check` and `vigil serve` queue
        // instead of failing with SQLITE_BUSY_SNAPSHOT halfway through.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;

        let row = sqlx::query("SELECT last_hash, snapshot_retention FROM watches WHERE id = ?")
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| StoreError::NotFound(format!("watch {watch}")))?;
        let last_hash = row.try_get::<Option<Vec<u8>>, _>("last_hash")?;
        let retention = row.try_get::<i64, _>("snapshot_retention")?.max(1);

        let outcome = match &check.content {
            None => RecordOutcome::Unchanged,
            Some(content) => {
                let hash = ContentHash::of(&content.text);
                if last_hash.as_deref() == Some(hash.as_bytes().as_slice()) {
                    RecordOutcome::Unchanged
                } else {
                    let previous = latest_snapshot(&mut tx, &id).await?;
                    let current = Uuid::now_v7();
                    sqlx::query(
                        "INSERT INTO snapshots \
                         (id, watch_id, taken_at, hash, content, raw_size, http_status, \
                          content_type) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(current.hyphenated().to_string())
                    .bind(&id)
                    .bind(check.at)
                    .bind(hash.as_bytes().as_slice())
                    .bind(compress(&content.text)?)
                    .bind(i64::try_from(content.raw_size).unwrap_or(i64::MAX))
                    .bind(check.http_status.map(i64::from))
                    .bind(&check.content_type)
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query(
                        "DELETE FROM snapshots WHERE watch_id = ?1 AND id NOT IN \
                         (SELECT id FROM snapshots WHERE watch_id = ?1 \
                          ORDER BY taken_at DESC, id DESC LIMIT ?2)",
                    )
                    .bind(&id)
                    .bind(retention)
                    .execute(&mut *tx)
                    .await?;
                    match previous {
                        Some((previous, previous_text)) => RecordOutcome::Changed {
                            previous,
                            current,
                            previous_text,
                        },
                        None => RecordOutcome::Baseline { snapshot: current },
                    }
                }
            }
        };

        match (&outcome, &check.content) {
            (RecordOutcome::Baseline { .. } | RecordOutcome::Changed { .. }, Some(content)) => {
                sqlx::query(
                    "UPDATE watches SET last_checked_at = ?1, last_changed_at = ?1, \
                     last_hash = ?2, etag = ?3, last_modified = ?4, \
                     consecutive_errors = 0, last_error = NULL WHERE id = ?5",
                )
                .bind(check.at)
                .bind(ContentHash::of(&content.text).as_bytes().as_slice())
                .bind(&check.etag)
                .bind(&check.last_modified)
                .bind(&id)
                .execute(&mut *tx)
                .await?;
            }
            (_, Some(_)) => {
                sqlx::query(
                    "UPDATE watches SET last_checked_at = ?1, etag = ?2, last_modified = ?3, \
                     consecutive_errors = 0, last_error = NULL WHERE id = ?4",
                )
                .bind(check.at)
                .bind(&check.etag)
                .bind(&check.last_modified)
                .bind(&id)
                .execute(&mut *tx)
                .await?;
            }
            (_, None) => {
                sqlx::query(
                    "UPDATE watches SET last_checked_at = ?1, \
                     etag = COALESCE(?2, etag), last_modified = COALESCE(?3, last_modified), \
                     consecutive_errors = 0, last_error = NULL WHERE id = ?4",
                )
                .bind(check.at)
                .bind(&check.etag)
                .bind(&check.last_modified)
                .bind(&id)
                .execute(&mut *tx)
                .await?;
            }
        }

        // The check_log outcome column only knows unchanged/changed/error, so the baseline
        // check is logged as a change.
        let logged = match outcome {
            RecordOutcome::Unchanged => "unchanged",
            RecordOutcome::Baseline { .. } | RecordOutcome::Changed { .. } => "changed",
        };
        log_check(&mut tx, &id, check.at, check.duration_ms, logged, None).await?;
        tx.commit().await?;
        Ok(outcome)
    }

    /// Record a failed check in one transaction and return the new consecutive-error count.
    pub async fn record_error(
        &self,
        watch: WatchId,
        at: i64,
        duration_ms: i64,
        message: &str,
    ) -> Result<u32, StoreError> {
        let id = watch.to_string();
        let message: String = message.chars().take(MAX_ERROR_CHARS).collect();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;

        let row = sqlx::query(
            "UPDATE watches SET last_checked_at = ?, last_error = ?, \
             consecutive_errors = consecutive_errors + 1 WHERE id = ? \
             RETURNING consecutive_errors",
        )
        .bind(at)
        .bind(&message)
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NotFound(format!("watch {watch}")))?;
        let errors: i64 = row.try_get("consecutive_errors")?;

        log_check(&mut tx, &id, at, duration_ms, "error", Some(&message)).await?;
        tx.commit().await?;
        u32::try_from(errors)
            .map_err(|_| StoreError::Corrupt(format!("consecutive_errors out of range: {errors}")))
    }
}

/// The watch's newest snapshot with its text, if it has one.
async fn latest_snapshot(
    tx: &mut sqlx::SqliteConnection,
    watch_id: &str,
) -> Result<Option<(SnapshotId, String)>, StoreError> {
    let row = sqlx::query(
        "SELECT id, content FROM snapshots WHERE watch_id = ? \
         ORDER BY taken_at DESC, id DESC LIMIT 1",
    )
    .bind(watch_id)
    .fetch_optional(tx)
    .await?;
    row.map(|row| {
        let id: String = row.try_get("id")?;
        let id = Uuid::parse_str(&id)
            .map_err(|_| StoreError::Corrupt(format!("invalid snapshot id {id:?}")))?;
        Ok((id, decompress(&row.try_get::<Vec<u8>, _>("content")?)?))
    })
    .transpose()
}

/// Append a check-log row and trim the log to [`CHECK_LOG_KEEP`] rows.
async fn log_check(
    tx: &mut sqlx::SqliteConnection,
    watch_id: &str,
    at: i64,
    duration_ms: i64,
    outcome: &str,
    error: Option<&str>,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO check_log (watch_id, at, duration_ms, outcome, error_message) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(watch_id)
    .bind(at)
    .bind(duration_ms)
    .bind(outcome)
    .bind(error)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM check_log WHERE watch_id = ?1 AND id NOT IN \
         (SELECT id FROM check_log WHERE watch_id = ?1 ORDER BY id DESC LIMIT ?2)",
    )
    .bind(watch_id)
    .bind(CHECK_LOG_KEEP)
    .execute(&mut *tx)
    .await?;
    Ok(())
}
