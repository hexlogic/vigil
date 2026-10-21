//! A watch's tags, notification targets and message template, saved together.

use vigil_core::WatchId;

use crate::{Store, StoreError, TagId, TargetId};

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
        let mut tx = self.pool.begin().await?;
        let updated =
            sqlx::query("UPDATE watches SET notification_template_override = ? WHERE id = ?")
                .bind(template)
                .bind(watch.to_string())
                .execute(&mut *tx)
                .await?;
        if updated.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("watch {watch}")));
        }
        for (delete, insert, ids) in [
            (
                "DELETE FROM watch_tags WHERE watch_id = ?",
                "INSERT OR IGNORE INTO watch_tags (watch_id, tag_id) VALUES (?, ?)",
                tags,
            ),
            (
                "DELETE FROM watch_notifications WHERE watch_id = ?",
                "INSERT OR IGNORE INTO watch_notifications (watch_id, target_id) VALUES (?, ?)",
                targets,
            ),
        ] {
            sqlx::query(delete)
                .bind(watch.to_string())
                .execute(&mut *tx)
                .await?;
            for id in ids {
                let linked = sqlx::query(insert)
                    .bind(watch.to_string())
                    .bind(id.to_string())
                    .execute(&mut *tx)
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
        tx.commit().await?;
        Ok(())
    }
}
