//! Notification targets and their links to watches.

use std::collections::HashMap;

use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use uuid::Uuid;
use vigil_core::WatchId;

use crate::{Store, StoreError, now_ms};

/// Identifier of a notification target (a UUID v7).
pub type TargetId = Uuid;

/// Shortest id suffix accepted by [`Store::resolve_target`].
const MIN_SUFFIX_LEN: usize = 6;

macro_rules! select_targets {
    ($tail:literal) => {
        concat!(
            "SELECT id, name, url, enabled, default_template, created_at \
             FROM notification_targets ",
            $tail
        )
    };
}

/// A place notifications are sent to: a URL naming a channel, plus an optional template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationTarget {
    /// Target id.
    pub id: TargetId,
    /// Unique, human-chosen name.
    pub name: String,
    /// The notification URL. It usually holds credentials: never log or print it unredacted.
    pub url: String,
    /// Disabled targets stay linked to their watches but are skipped when notifying.
    pub enabled: bool,
    /// Message template used when the watch has none of its own.
    pub default_template: Option<String>,
    /// When the target was created, in unix milliseconds.
    pub created_at: i64,
}

/// A target that does not exist yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTarget {
    /// Unique name; the caller makes sure it is not empty.
    pub name: String,
    /// The notification URL.
    pub url: String,
    /// Message template used when the watch has none of its own.
    pub default_template: Option<String>,
}

impl Store {
    /// Insert an enabled target with a fresh UUID v7 id.
    ///
    /// A name that is already taken is [`StoreError::Conflict`].
    pub async fn create_target(&self, new: &NewTarget) -> Result<NotificationTarget, StoreError> {
        let id = Uuid::now_v7();
        let inserted = sqlx::query(
            "INSERT INTO notification_targets (id, name, url, default_template, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(&new.name)
        .bind(&new.url)
        .bind(new.default_template.as_deref())
        .bind(now_ms())
        .execute(&self.pool)
        .await;
        match inserted {
            Ok(_) => self.get_target(id).await,
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(StoreError::Conflict(
                format!("a notification target named {:?} already exists", new.name),
            )),
            Err(e) => Err(e.into()),
        }
    }

    /// All targets, ordered by name.
    pub async fn list_targets(&self) -> Result<Vec<NotificationTarget>, StoreError> {
        let rows = sqlx::query(select_targets!("ORDER BY name"))
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(target_from_row).collect()
    }

    /// One target by id.
    pub async fn get_target(&self, id: TargetId) -> Result<NotificationTarget, StoreError> {
        let row = sqlx::query(select_targets!("WHERE id = ?"))
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound(format!("notification target {id}")))?;
        target_from_row(&row)
    }

    /// Replace a target's name, URL and default template. Whether it is enabled and which
    /// watches it is linked to stay as they are.
    ///
    /// A name that another target has is [`StoreError::Conflict`].
    pub async fn update_target(
        &self,
        id: TargetId,
        new: &NewTarget,
    ) -> Result<NotificationTarget, StoreError> {
        let updated = sqlx::query(
            "UPDATE notification_targets SET name = ?, url = ?, default_template = ? WHERE id = ?",
        )
        .bind(&new.name)
        .bind(&new.url)
        .bind(new.default_template.as_deref())
        .bind(id.to_string())
        .execute(&self.pool)
        .await;
        match updated {
            Ok(result) if result.rows_affected() == 0 => {
                Err(StoreError::NotFound(format!("notification target {id}")))
            }
            Ok(_) => self.get_target(id).await,
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(StoreError::Conflict(
                format!("a notification target named {:?} already exists", new.name),
            )),
            Err(e) => Err(e.into()),
        }
    }

    /// Delete a target; its links to watches and its delivery log go with it.
    pub async fn delete_target(&self, id: TargetId) -> Result<(), StoreError> {
        let result = sqlx::query("DELETE FROM notification_targets WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("notification target {id}")));
        }
        Ok(())
    }

    /// Enable or disable a target.
    pub async fn set_target_enabled(&self, id: TargetId, enabled: bool) -> Result<(), StoreError> {
        let result = sqlx::query("UPDATE notification_targets SET enabled = ? WHERE id = ?")
            .bind(i64::from(enabled))
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("notification target {id}")));
        }
        Ok(())
    }

    /// Resolve user input to a target.
    ///
    /// An exact name wins. Otherwise the input is a full id, or the trailing characters of one
    /// (at least 6 hex digits, hyphens allowed), matched as a suffix for the same reason as
    /// [`Store::resolve_watch_id`].
    pub async fn resolve_target(&self, input: &str) -> Result<NotificationTarget, StoreError> {
        let input = input.trim();
        let by_name = sqlx::query(select_targets!("WHERE name = ?"))
            .bind(input)
            .fetch_optional(&self.pool)
            .await?;
        if let Some(row) = by_name {
            return target_from_row(&row);
        }
        if let Ok(id) = Uuid::parse_str(input) {
            return self.get_target(id).await;
        }

        let suffix = input.to_ascii_lowercase();
        let usable = suffix.len() >= MIN_SUFFIX_LEN
            && suffix.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        if !usable {
            return Err(StoreError::NotFound(format!(
                "notification target {input:?}"
            )));
        }
        // The suffix only holds hex digits and hyphens, so it has no LIKE wildcards in it.
        let rows = sqlx::query(select_targets!("WHERE id LIKE ? LIMIT 2"))
            .bind(format!("%{suffix}"))
            .fetch_all(&self.pool)
            .await?;
        match rows.as_slice() {
            [] => Err(StoreError::NotFound(format!(
                "notification target {input:?}"
            ))),
            [row] => target_from_row(row),
            _ => Err(StoreError::AmbiguousTarget(input.to_owned())),
        }
    }

    /// Send a watch's notifications to a target. Linking twice is not an error.
    pub async fn link_target(&self, watch: WatchId, target: TargetId) -> Result<(), StoreError> {
        let linked = sqlx::query(
            "INSERT OR IGNORE INTO watch_notifications (watch_id, target_id) VALUES (?, ?)",
        )
        .bind(watch.to_string())
        .bind(target.to_string())
        .execute(&self.pool)
        .await;
        match linked {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => Err(
                StoreError::NotFound(format!("watch {watch} or notification target {target}")),
            ),
            Err(e) => Err(e.into()),
        }
    }

    /// Stop sending a watch's notifications to a target. Unlinking twice is not an error.
    pub async fn unlink_target(&self, watch: WatchId, target: TargetId) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM watch_notifications WHERE watch_id = ? AND target_id = ?")
            .bind(watch.to_string())
            .bind(target.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The targets linked to a watch, ordered by name. `enabled_only` leaves out disabled ones.
    pub async fn targets_for_watch(
        &self,
        watch: WatchId,
        enabled_only: bool,
    ) -> Result<Vec<NotificationTarget>, StoreError> {
        let rows = sqlx::query(
            "SELECT t.id, t.name, t.url, t.enabled, t.default_template, t.created_at \
             FROM notification_targets t \
             JOIN watch_notifications l ON l.target_id = t.id \
             WHERE l.watch_id = ?1 AND (?2 = 0 OR t.enabled = 1) \
             ORDER BY t.name",
        )
        .bind(watch.to_string())
        .bind(i64::from(enabled_only))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(target_from_row).collect()
    }

    /// How many watches each target is linked to. Targets with no links are absent.
    pub async fn linked_watch_counts(&self) -> Result<HashMap<TargetId, u32>, StoreError> {
        let rows = sqlx::query(
            "SELECT target_id, COUNT(*) AS n FROM watch_notifications GROUP BY target_id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut counts = HashMap::new();
        for row in &rows {
            let id = parse_id(&row.try_get::<String, _>("target_id")?)?;
            let n = u32::try_from(row.try_get::<i64, _>("n")?)
                .map_err(|_| StoreError::Corrupt("link count out of range".into()))?;
            counts.insert(id, n);
        }
        Ok(counts)
    }
}

pub(crate) fn parse_id(text: &str) -> Result<TargetId, StoreError> {
    Uuid::parse_str(text)
        .map_err(|_| StoreError::Corrupt(format!("invalid notification target id {text:?}")))
}

fn target_from_row(row: &SqliteRow) -> Result<NotificationTarget, StoreError> {
    Ok(NotificationTarget {
        id: parse_id(&row.try_get::<String, _>("id")?)?,
        name: row.try_get("name")?,
        url: row.try_get("url")?,
        enabled: row.try_get::<i64, _>("enabled")? != 0,
        default_template: row.try_get("default_template")?,
        created_at: row.try_get("created_at")?,
    })
}
