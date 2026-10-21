//! A watch's tags, notification targets and message template, saved together.

use std::collections::HashMap;

use sqlx::{Row, SqliteConnection};
use uuid::Uuid;
use vigil_core::{Watch, WatchId};

use crate::watches::{NewWatch, insert_watch, write_settings};
use crate::{Store, StoreError, TagId, TargetId, now_ms};

/// The tags, notification targets and message template a watch is saved with.
#[derive(Debug, Clone, Copy, Default)]
pub struct WatchLinks<'a> {
    /// The watch's tags. Repeats are ignored.
    pub tags: &'a [TagId],
    /// The targets notified about the watch. Repeats are ignored.
    pub targets: &'a [TargetId],
    /// The watch's message template.
    pub template: Option<&'a str>,
}

/// Make `links` what a watch has, on a connection that holds a transaction. An unknown watch,
/// tag or target is [`StoreError::NotFound`].
async fn write_links(
    conn: &mut SqliteConnection,
    watch: WatchId,
    links: &WatchLinks<'_>,
) -> Result<(), StoreError> {
    let updated = sqlx::query("UPDATE watches SET notification_template_override = ? WHERE id = ?")
        .bind(links.template)
        .bind(watch.to_string())
        .execute(&mut *conn)
        .await?;
    if updated.rows_affected() == 0 {
        return Err(StoreError::NotFound(format!("watch {watch}")));
    }
    for (delete, insert, ids) in [
        (
            "DELETE FROM watch_tags WHERE watch_id = ?",
            "INSERT OR IGNORE INTO watch_tags (watch_id, tag_id) VALUES (?, ?)",
            links.tags,
        ),
        (
            "DELETE FROM watch_notifications WHERE watch_id = ?",
            "INSERT OR IGNORE INTO watch_notifications (watch_id, target_id) VALUES (?, ?)",
            links.targets,
        ),
    ] {
        sqlx::query(delete)
            .bind(watch.to_string())
            .execute(&mut *conn)
            .await?;
        for id in ids {
            let linked = sqlx::query(insert)
                .bind(watch.to_string())
                .bind(id.to_string())
                .execute(&mut *conn)
                .await;
            match linked {
                Ok(_) => {}
                Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => {
                    return Err(StoreError::NotFound(format!("tag or target {id}")));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
    Ok(())
}

impl Store {
    /// Make `tags` and `targets` the ones a watch has, and `template` its message template,
    /// all or nothing. Repeats are ignored.
    ///
    /// Like [`Store::set_notification_template`] this leaves `updated_at`, the remembered
    /// validators and the failure streak alone. An unknown watch, tag or target is
    /// [`StoreError::NotFound`] and changes nothing.
    pub async fn set_watch_links(
        &self,
        watch: WatchId,
        tags: &[TagId],
        targets: &[TargetId],
        template: Option<&str>,
    ) -> Result<(), StoreError> {
        let links = WatchLinks {
            tags,
            targets,
            template,
        };
        let mut tx = self.pool.begin().await?;
        write_links(&mut tx, watch, &links).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Insert a watch with a fresh UUID v7 id together with its links, all or nothing, and
    /// return it as stored. A watch made `paused` is never stored unpaused, so a scheduler
    /// reload cannot pick it up. An unknown tag or target is [`StoreError::NotFound`] and
    /// leaves no watch.
    pub async fn create_watch_with_links(
        &self,
        new: &NewWatch,
        paused: bool,
        links: &WatchLinks<'_>,
    ) -> Result<Watch, StoreError> {
        let id = WatchId::from_uuid(Uuid::now_v7());
        let mut tx = self.pool.begin().await?;
        insert_watch(&mut tx, id, new, paused, now_ms()).await?;
        write_links(&mut tx, id, links).await?;
        tx.commit().await?;
        self.get_watch(id).await
    }

    /// Replace a watch's settings, pause flag and links, all or nothing.
    ///
    /// `settings` and `paused` that are `None` are left as they are. Writing settings has the
    /// side effects of [`Store::update_watch`], so a caller passes them only when they
    /// changed. An unknown watch, tag or target is [`StoreError::NotFound`] and changes
    /// nothing.
    pub async fn replace_watch(
        &self,
        id: WatchId,
        settings: Option<&NewWatch>,
        paused: Option<bool>,
        links: &WatchLinks<'_>,
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        if let Some(settings) = settings {
            write_settings(&mut tx, id, settings, now_ms()).await?;
        }
        if let Some(paused) = paused {
            sqlx::query("UPDATE watches SET paused = ? WHERE id = ?")
                .bind(i64::from(paused))
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        write_links(&mut tx, id, links).await?;
        tx.commit().await?;
        Ok(())
    }

    /// The ids of the targets each watch is linked to, for every watch that has any. Disabled
    /// targets are included.
    pub async fn watch_target_ids(&self) -> Result<HashMap<WatchId, Vec<TargetId>>, StoreError> {
        let rows =
            sqlx::query("SELECT watch_id, target_id FROM watch_notifications ORDER BY target_id")
                .fetch_all(&self.pool)
                .await?;
        let mut map: HashMap<WatchId, Vec<TargetId>> = HashMap::new();
        for row in &rows {
            let watch: String = row.try_get("watch_id")?;
            let target: String = row.try_get("target_id")?;
            let (Ok(watch), Ok(target)) = (watch.parse::<WatchId>(), Uuid::parse_str(&target))
            else {
                return Err(StoreError::Corrupt(
                    "a notification link holds a bad id".to_owned(),
                ));
            };
            map.entry(watch).or_default().push(target);
        }
        Ok(map)
    }
}
