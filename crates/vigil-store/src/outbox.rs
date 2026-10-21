//! Notifications waiting to be delivered.
//!
//! A notification is saved here before delivery starts and removed when delivery is over, so
//! one that is cut off by a restart or a crash is still there to be sent on the next start.

use sqlx::Row;
use vigil_core::WatchId;

use crate::snapshots::{SnapshotId, compress, decompress};
use crate::targets::{TargetId, parse_id};
use crate::{Store, StoreError, now_ms};

/// A notification to save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPending {
    /// The watch that changed.
    pub watch: WatchId,
    /// Where to send it.
    pub target: TargetId,
    /// The snapshot that triggered it.
    pub snapshot: SnapshotId,
    /// What the message is built from; the store keeps it compressed and does not read it.
    pub payload: String,
}

/// A saved notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingNotification {
    /// Identifies the row, for [`Store::complete_notification`].
    pub id: i64,
    /// The watch that changed.
    pub watch: WatchId,
    /// Where to send it.
    pub target: TargetId,
    /// The snapshot that triggered it.
    pub snapshot: SnapshotId,
    /// When it was saved, in unix milliseconds.
    pub created_at: i64,
    /// What the message is built from.
    pub payload: String,
}

impl Store {
    /// Save a notification and return its row id.
    pub async fn enqueue_notification(&self, new: &NewPending) -> Result<i64, StoreError> {
        let result = sqlx::query(
            "INSERT INTO notification_outbox \
             (watch_id, target_id, snapshot_id, created_at, payload) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(new.watch.to_string())
        .bind(new.target.to_string())
        .bind(new.snapshot.hyphenated().to_string())
        .bind(now_ms())
        .bind(compress(&new.payload)?)
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    /// Every saved notification, oldest first.
    ///
    /// A row that cannot be read (damaged by hand) is removed and left out: it could never be
    /// delivered and would be found again at every start.
    pub async fn pending_notifications(&self) -> Result<Vec<PendingNotification>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, watch_id, target_id, snapshot_id, created_at, payload \
             FROM notification_outbox ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut pending = Vec::new();
        for row in &rows {
            let id: i64 = row.try_get("id")?;
            match read_row(row) {
                Ok(item) => pending.push(item),
                Err(error) => {
                    tracing::warn!(id, %error, "dropping an unreadable saved notification");
                    self.complete_notification(id).await?;
                }
            }
        }
        Ok(pending)
    }

    /// Remove a saved notification once it has been delivered, or has failed for good.
    /// Removing one that is already gone is not an error.
    pub async fn complete_notification(&self, id: i64) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM notification_outbox WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn read_row(row: &sqlx::sqlite::SqliteRow) -> Result<PendingNotification, StoreError> {
    let corrupt = |what: &str, text: &str| StoreError::Corrupt(format!("invalid {what} {text:?}"));
    let watch: String = row.try_get("watch_id")?;
    let snapshot: String = row.try_get("snapshot_id")?;
    Ok(PendingNotification {
        id: row.try_get("id")?,
        watch: watch.parse().map_err(|_| corrupt("watch id", &watch))?,
        target: parse_id(&row.try_get::<String, _>("target_id")?)?,
        snapshot: snapshot
            .parse()
            .map_err(|_| corrupt("snapshot id", &snapshot))?,
        created_at: row.try_get("created_at")?,
        payload: decompress(&row.try_get::<Vec<u8>, _>("payload")?)?,
    })
}
