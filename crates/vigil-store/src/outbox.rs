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
    /// How many tries have run out of attempts so far.
    pub rounds: u32,
    /// When to try again, in unix milliseconds; `None` while a delivery is in progress.
    pub next_attempt_at: Option<i64>,
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
            "SELECT id, watch_id, target_id, snapshot_id, created_at, rounds, next_attempt_at, \
             payload FROM notification_outbox ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        self.read_all(&rows).await
    }

    /// The saved notifications that are due to be tried again at `now`, oldest first.
    pub async fn due_notifications(
        &self,
        now: i64,
    ) -> Result<Vec<PendingNotification>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, watch_id, target_id, snapshot_id, created_at, rounds, next_attempt_at, \
             payload FROM notification_outbox \
             WHERE next_attempt_at IS NOT NULL AND next_attempt_at <= ? ORDER BY id",
        )
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        self.read_all(&rows).await
    }

    async fn read_all(
        &self,
        rows: &[sqlx::sqlite::SqliteRow],
    ) -> Result<Vec<PendingNotification>, StoreError> {
        let mut pending = Vec::new();
        for row in rows {
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

    /// Take a notification that is due for another try, so that nothing else sends it. True if
    /// this call took it; false if it is not due, or was taken or removed meanwhile.
    pub async fn claim_notification(&self, id: i64, now: i64) -> Result<bool, StoreError> {
        let result = sqlx::query(
            "UPDATE notification_outbox SET next_attempt_at = NULL \
             WHERE id = ? AND next_attempt_at IS NOT NULL AND next_attempt_at <= ?",
        )
        .bind(id)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Put a notification whose tries ran out back to wait until `next_attempt_at`, as the
    /// `rounds`-th round that ran out.
    pub async fn defer_notification(
        &self,
        id: i64,
        next_attempt_at: i64,
        rounds: u32,
    ) -> Result<(), StoreError> {
        sqlx::query("UPDATE notification_outbox SET next_attempt_at = ?, rounds = ? WHERE id = ?")
            .bind(next_attempt_at)
            .bind(i64::from(rounds))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
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
        rounds: u32::try_from(row.try_get::<i64, _>("rounds")?)
            .map_err(|_| StoreError::Corrupt("rounds out of range".into()))?,
        next_attempt_at: row.try_get("next_attempt_at")?,
        payload: decompress(&row.try_get::<Vec<u8>, _>("payload")?)?,
    })
}
