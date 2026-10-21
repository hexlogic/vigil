//! Tags: named groups of watches that can carry default notification targets.

use std::collections::HashMap;

use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteConnection};
use uuid::Uuid;
use vigil_core::WatchId;

use crate::targets::{NotificationTarget, target_from_row};
use crate::{Store, StoreError, now_ms};

/// Identifier of a tag (a UUID v7).
pub type TagId = Uuid;

/// A group of watches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// Tag id.
    pub id: TagId,
    /// Unique name.
    pub name: String,
    /// How the UI shows the tag. The store keeps the text and does not read it.
    pub color: String,
    /// Interval, in seconds, for a watch added with this tag and no interval of its own.
    pub default_interval_secs: Option<u64>,
    /// When the tag was created, in unix milliseconds.
    pub created_at: i64,
}

/// A tag that does not exist yet, or the new settings of one that does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTag {
    /// Unique name; the caller makes sure it is not empty.
    pub name: String,
    /// How the UI shows the tag.
    pub color: String,
    /// Interval, in seconds, for a watch added with this tag and no interval of its own.
    pub default_interval_secs: Option<u64>,
}

/// Insert a tag row on a connection, so that a transaction can hold several.
pub(crate) async fn insert_tag(
    conn: &mut SqliteConnection,
    id: TagId,
    new: &NewTag,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO tags (id, name, color, default_interval_secs, created_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(&new.name)
    .bind(&new.color)
    .bind(new.default_interval_secs.map(interval_to_db))
    .bind(now_ms())
    .execute(conn)
    .await
    .map(drop)
}

/// Replace a tag's name, colour and default interval on a connection. An unknown tag is
/// [`StoreError::NotFound`] and a name that another tag has is [`StoreError::Conflict`].
pub(crate) async fn update_tag_row(
    conn: &mut SqliteConnection,
    id: TagId,
    new: &NewTag,
) -> Result<(), StoreError> {
    let updated =
        sqlx::query("UPDATE tags SET name = ?, color = ?, default_interval_secs = ? WHERE id = ?")
            .bind(&new.name)
            .bind(&new.color)
            .bind(new.default_interval_secs.map(interval_to_db))
            .bind(id.to_string())
            .execute(conn)
            .await
            .map_err(|e| name_clash(e, &new.name))?;
    if updated.rows_affected() == 0 {
        return Err(StoreError::NotFound(format!("tag {id}")));
    }
    Ok(())
}

impl Store {
    /// Insert a tag with a fresh UUID v7 id.
    ///
    /// A name that is already taken is [`StoreError::Conflict`].
    pub async fn create_tag(&self, new: &NewTag) -> Result<Tag, StoreError> {
        let id = Uuid::now_v7();
        let inserted = async {
            let mut conn = self.pool.acquire().await?;
            insert_tag(&mut conn, id, new).await
        }
        .await;
        match inserted {
            Ok(_) => self.get_tag(id).await,
            Err(e) => Err(name_clash(e, &new.name)),
        }
    }

    /// Replace a tag's name, colour and default interval.
    ///
    /// A name that another tag has is [`StoreError::Conflict`].
    pub async fn update_tag(&self, id: TagId, new: &NewTag) -> Result<Tag, StoreError> {
        let mut conn = self.pool.acquire().await?;
        update_tag_row(&mut conn, id, new).await?;
        drop(conn);
        self.get_tag(id).await
    }

    /// One tag by id.
    pub async fn get_tag(&self, id: TagId) -> Result<Tag, StoreError> {
        let row = sqlx::query(
            "SELECT id, name, color, default_interval_secs, created_at FROM tags WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound(format!("tag {id}")))?;
        tag_from_row(&row)
    }

    /// All tags, ordered by name.
    pub async fn list_tags(&self) -> Result<Vec<Tag>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, name, color, default_interval_secs, created_at FROM tags ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(tag_from_row).collect()
    }

    /// Delete a tag; its links to watches and to targets go with it. The watches stay.
    pub async fn delete_tag(&self, id: TagId) -> Result<(), StoreError> {
        let result = sqlx::query("DELETE FROM tags WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("tag {id}")));
        }
        Ok(())
    }

    /// Make `tags` the tags of a watch, replacing the ones it had. Repeats are ignored.
    pub async fn set_watch_tags(&self, watch: WatchId, tags: &[TagId]) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM watch_tags WHERE watch_id = ?")
            .bind(watch.to_string())
            .execute(&mut *tx)
            .await?;
        for tag in tags {
            let linked =
                sqlx::query("INSERT OR IGNORE INTO watch_tags (watch_id, tag_id) VALUES (?, ?)")
                    .bind(watch.to_string())
                    .bind(tag.to_string())
                    .execute(&mut *tx)
                    .await;
            match linked {
                Ok(_) => {}
                Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => {
                    return Err(StoreError::NotFound(format!("watch {watch} or tag {tag}")));
                }
                Err(e) => return Err(e.into()),
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// The tags of one watch, ordered by name.
    pub async fn tags_for_watch(&self, watch: WatchId) -> Result<Vec<Tag>, StoreError> {
        let rows = sqlx::query(
            "SELECT t.id, t.name, t.color, t.default_interval_secs, t.created_at \
             FROM tags t JOIN watch_tags l ON l.tag_id = t.id \
             WHERE l.watch_id = ? ORDER BY t.name",
        )
        .bind(watch.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(tag_from_row).collect()
    }

    /// The tag ids of every tagged watch, for listing many watches with one query.
    pub async fn watch_tags_map(&self) -> Result<HashMap<WatchId, Vec<TagId>>, StoreError> {
        let rows = sqlx::query("SELECT watch_id, tag_id FROM watch_tags ORDER BY watch_id, tag_id")
            .fetch_all(&self.pool)
            .await?;
        let mut map: HashMap<WatchId, Vec<TagId>> = HashMap::new();
        for row in &rows {
            let watch: String = row.try_get("watch_id")?;
            let watch = watch
                .parse()
                .map_err(|_| StoreError::Corrupt(format!("invalid watch id {watch:?}")))?;
            let tag: String = row.try_get("tag_id")?;
            let tag = Uuid::parse_str(&tag)
                .map_err(|_| StoreError::Corrupt(format!("invalid tag id {tag:?}")))?;
            map.entry(watch).or_default().push(tag);
        }
        Ok(map)
    }

    /// How many watches carry each tag. Tags with no watches are absent.
    pub async fn tag_watch_counts(&self) -> Result<HashMap<TagId, u32>, StoreError> {
        let rows = sqlx::query("SELECT tag_id, COUNT(*) AS n FROM watch_tags GROUP BY tag_id")
            .fetch_all(&self.pool)
            .await?;
        let mut counts = HashMap::new();
        for row in &rows {
            let tag: String = row.try_get("tag_id")?;
            let tag = Uuid::parse_str(&tag)
                .map_err(|_| StoreError::Corrupt(format!("invalid tag id {tag:?}")))?;
            let n = u32::try_from(row.try_get::<i64, _>("n")?)
                .map_err(|_| StoreError::Corrupt("tag count out of range".into()))?;
            counts.insert(tag, n);
        }
        Ok(counts)
    }

    /// The enabled targets a change to a watch is sent to: those linked to the watch itself
    /// and those that are default targets of any of its tags, each once, ordered by name.
    pub async fn delivery_targets(
        &self,
        watch: WatchId,
    ) -> Result<Vec<NotificationTarget>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, name, url, enabled, default_template, created_at \
             FROM notification_targets \
             WHERE enabled = 1 AND (\
                 id IN (SELECT target_id FROM watch_notifications WHERE watch_id = ?1) \
                 OR id IN (SELECT n.target_id FROM tag_notifications n \
                           JOIN watch_tags w ON w.tag_id = n.tag_id WHERE w.watch_id = ?1)) \
             ORDER BY name",
        )
        .bind(watch.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(target_from_row).collect()
    }
}

/// An interval as stored. Intervals that do not fit are clamped; the scheduler clamps them
/// much lower anyway.
fn interval_to_db(secs: u64) -> i64 {
    i64::try_from(secs).unwrap_or(i64::MAX)
}

pub(crate) fn name_clash(error: sqlx::Error, name: &str) -> StoreError {
    match error {
        sqlx::Error::Database(e) if e.is_unique_violation() => {
            StoreError::Conflict(format!("a tag named {name:?} already exists"))
        }
        e => e.into(),
    }
}

fn tag_from_row(row: &SqliteRow) -> Result<Tag, StoreError> {
    let id: String = row.try_get("id")?;
    let interval: Option<i64> = row.try_get("default_interval_secs")?;
    Ok(Tag {
        id: Uuid::parse_str(&id)
            .map_err(|_| StoreError::Corrupt(format!("invalid tag id {id:?}")))?,
        name: row.try_get("name")?,
        color: row.try_get("color")?,
        default_interval_secs: interval
            .map(|secs| {
                u64::try_from(secs)
                    .map_err(|_| StoreError::Corrupt(format!("negative tag interval {secs}")))
            })
            .transpose()?,
        created_at: row.try_get("created_at")?,
    })
}
