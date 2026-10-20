//! The delivery log: what happened to each notification.

use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use vigil_core::WatchId;

use crate::snapshots::SnapshotId;
use crate::targets::{TargetId, parse_id};
use crate::{Store, StoreError};

/// Delivery-log rows kept per watch (and per target for test notifications).
pub const DELIVERY_LOG_KEEP: i64 = 500;

/// Longest error message stored, in characters.
const MAX_ERROR_CHARS: usize = 1000;

/// How a notification ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// The channel accepted the message.
    Sent,
    /// Every attempt failed, or the notification could not be built.
    Failed,
}

impl DeliveryOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Failed => "failed",
        }
    }
}

/// One notification to one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRecord {
    /// The watch that changed; `None` for a test notification.
    pub watch: Option<WatchId>,
    /// Where it was sent.
    pub target: TargetId,
    /// The snapshot that triggered it, if any.
    pub snapshot: Option<SnapshotId>,
    /// When the last attempt ended, in unix milliseconds.
    pub at: i64,
    /// How many times sending was tried; 0 when the notification could not be built.
    pub attempts: u32,
    /// How it ended.
    pub outcome: DeliveryOutcome,
    /// What went wrong, or a note such as a template fallback. It must not hold secrets.
    pub error: Option<String>,
}

impl Store {
    /// Append a delivery-log row and trim the log to [`DELIVERY_LOG_KEEP`] rows for its watch
    /// (or for its target, when there is no watch).
    pub async fn record_delivery(&self, record: &DeliveryRecord) -> Result<(), StoreError> {
        let watch = record.watch.map(|w| w.to_string());
        let target = record.target.to_string();
        let error = record
            .error
            .as_deref()
            .map(|e| e.chars().take(MAX_ERROR_CHARS).collect::<String>());
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "INSERT INTO delivery_log \
             (watch_id, target_id, snapshot_id, at, attempts, outcome, error_message) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(watch.as_deref())
        .bind(&target)
        .bind(record.snapshot.map(|s| s.to_string()))
        .bind(record.at)
        .bind(i64::from(record.attempts))
        .bind(record.outcome.as_str())
        .bind(error)
        .execute(&mut *tx)
        .await?;
        let prune = match watch {
            Some(_) => {
                "DELETE FROM delivery_log WHERE watch_id = ?1 AND id NOT IN \
                 (SELECT id FROM delivery_log WHERE watch_id = ?1 ORDER BY id DESC LIMIT ?2)"
            }
            None => {
                "DELETE FROM delivery_log WHERE watch_id IS NULL AND target_id = ?1 AND id NOT IN \
                 (SELECT id FROM delivery_log WHERE watch_id IS NULL AND target_id = ?1 \
                  ORDER BY id DESC LIMIT ?2)"
            }
        };
        sqlx::query(prune)
            .bind(watch.as_deref().unwrap_or(&target))
            .bind(DELIVERY_LOG_KEEP)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// A watch's most recent deliveries, newest first.
    pub async fn list_deliveries(
        &self,
        watch: WatchId,
        limit: u32,
    ) -> Result<Vec<DeliveryRecord>, StoreError> {
        let rows = sqlx::query(
            "SELECT watch_id, target_id, snapshot_id, at, attempts, outcome, error_message \
             FROM delivery_log WHERE watch_id = ? ORDER BY id DESC LIMIT ?",
        )
        .bind(watch.to_string())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(record_from_row).collect()
    }
}

fn record_from_row(row: &SqliteRow) -> Result<DeliveryRecord, StoreError> {
    let outcome = match row.try_get::<String, _>("outcome")?.as_str() {
        "sent" => DeliveryOutcome::Sent,
        "failed" => DeliveryOutcome::Failed,
        other => {
            return Err(StoreError::Corrupt(format!(
                "unknown delivery outcome {other:?}"
            )));
        }
    };
    let corrupt = |what: &str, text: &str| StoreError::Corrupt(format!("invalid {what} {text:?}"));
    let watch = row
        .try_get::<Option<String>, _>("watch_id")?
        .map(|text| text.parse().map_err(|_| corrupt("watch id", &text)))
        .transpose()?;
    let snapshot = row
        .try_get::<Option<String>, _>("snapshot_id")?
        .map(|text| text.parse().map_err(|_| corrupt("snapshot id", &text)))
        .transpose()?;
    let attempts = u32::try_from(row.try_get::<i64, _>("attempts")?)
        .map_err(|_| StoreError::Corrupt("attempts out of range".into()))?;
    Ok(DeliveryRecord {
        watch,
        target: parse_id(&row.try_get::<String, _>("target_id")?)?,
        snapshot,
        at: row.try_get("at")?,
        attempts,
        outcome,
        error: row.try_get("error_message")?,
    })
}
