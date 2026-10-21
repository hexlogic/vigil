//! Instance settings: small named values such as the admin password hash.

use crate::{Store, StoreError, now_ms};

impl Store {
    /// The value stored under `key`, if any.
    pub async fn get_setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        let value = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(value)
    }

    /// Store `value` under `key`, replacing what was there.
    pub async fn set_setting(&self, key: &str, value: &str) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value, \
             updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(value)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Remove the value stored under `key`. Removing one that is not there is not an error.
    pub async fn delete_setting(&self, key: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM settings WHERE key = ?")
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
