//! A tag's default notification targets, saved with the tag.

use sqlx::{Row, SqliteConnection};
use uuid::Uuid;

use crate::tags::{NewTag, Tag, TagId, insert_tag, name_clash, update_tag_row};
use crate::targets::{TargetId, parse_id};
use crate::{Store, StoreError};

/// Make `targets` the default targets of a tag on a connection that holds a transaction,
/// replacing the ones it had. An unknown tag or target is [`StoreError::NotFound`].
async fn write_tag_targets(
    conn: &mut SqliteConnection,
    tag: TagId,
    targets: &[TargetId],
) -> Result<(), StoreError> {
    sqlx::query("DELETE FROM tag_notifications WHERE tag_id = ?")
        .bind(tag.to_string())
        .execute(&mut *conn)
        .await?;
    for target in targets {
        let linked = sqlx::query(
            "INSERT OR IGNORE INTO tag_notifications (tag_id, target_id) VALUES (?, ?)",
        )
        .bind(tag.to_string())
        .bind(target.to_string())
        .execute(&mut *conn)
        .await;
        match linked {
            Ok(_) => {}
            Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => {
                return Err(StoreError::NotFound(format!(
                    "tag {tag} or notification target {target}"
                )));
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

impl Store {
    /// Make `targets` the default notification targets of a tag, replacing the ones it had.
    /// Repeats are ignored.
    pub async fn set_tag_targets(
        &self,
        tag: TagId,
        targets: &[TargetId],
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        write_tag_targets(&mut tx, tag, targets).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Insert a tag with a fresh UUID v7 id together with its default targets, all or nothing.
    ///
    /// A name that is already taken is [`StoreError::Conflict`], and an unknown target is
    /// [`StoreError::NotFound`]; neither leaves a tag.
    pub async fn create_tag_with_targets(
        &self,
        new: &NewTag,
        targets: &[TargetId],
    ) -> Result<Tag, StoreError> {
        let id = Uuid::now_v7();
        let mut tx = self.pool.begin().await?;
        insert_tag(&mut tx, id, new)
            .await
            .map_err(|e| name_clash(e, &new.name))?;
        write_tag_targets(&mut tx, id, targets).await?;
        tx.commit().await?;
        self.get_tag(id).await
    }

    /// Replace a tag's name, colour, default interval and default targets, all or nothing.
    ///
    /// An unknown tag or target is [`StoreError::NotFound`] and a name that another tag has
    /// is [`StoreError::Conflict`]; neither changes anything.
    pub async fn replace_tag(
        &self,
        id: TagId,
        new: &NewTag,
        targets: &[TargetId],
    ) -> Result<Tag, StoreError> {
        let mut tx = self.pool.begin().await?;
        update_tag_row(&mut tx, id, new).await?;
        write_tag_targets(&mut tx, id, targets).await?;
        tx.commit().await?;
        self.get_tag(id).await
    }

    /// The ids of a tag's default notification targets.
    pub async fn tag_targets(&self, tag: TagId) -> Result<Vec<TargetId>, StoreError> {
        let rows = sqlx::query("SELECT target_id FROM tag_notifications WHERE tag_id = ?")
            .bind(tag.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| parse_id(&row.try_get::<String, _>("target_id")?))
            .collect()
    }
}
