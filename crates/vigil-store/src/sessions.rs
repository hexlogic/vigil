//! Browser sessions of the web UI.
//!
//! The store never sees a cookie value, only the hash the caller derives from it, so nothing in
//! the database (or in a backup of it) can be used to sign in.

use sqlx::Row;

use crate::{Store, StoreError};

/// A signed-in browser session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session {
    /// When the session was created, in unix milliseconds.
    pub created_at: i64,
    /// When the session was last seen, in unix milliseconds.
    pub last_seen_at: i64,
    /// When the session ends, in unix milliseconds.
    pub expires_at: i64,
}

impl Store {
    /// Start a session identified by `token_hash`, valid until `expires_at`.
    pub async fn create_session(
        &self,
        token_hash: &[u8],
        now: i64,
        expires_at: i64,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO sessions (token_hash, created_at, last_seen_at, expires_at) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(token_hash)
        .bind(now)
        .bind(now)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The session identified by `token_hash`, unless it is unknown or over at `now`.
    pub async fn session(
        &self,
        token_hash: &[u8],
        now: i64,
    ) -> Result<Option<Session>, StoreError> {
        let row = sqlx::query(
            "SELECT created_at, last_seen_at, expires_at FROM sessions \
             WHERE token_hash = ? AND expires_at > ?",
        )
        .bind(token_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(Session {
                created_at: row.try_get("created_at")?,
                last_seen_at: row.try_get("last_seen_at")?,
                expires_at: row.try_get("expires_at")?,
            })
        })
        .transpose()
    }

    /// Record that the session was seen at `now` and move its end to `expires_at`. A session
    /// that is gone or already over is left alone.
    pub async fn touch_session(
        &self,
        token_hash: &[u8],
        now: i64,
        expires_at: i64,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE sessions SET last_seen_at = ?, expires_at = ? \
             WHERE token_hash = ? AND expires_at > ?",
        )
        .bind(now)
        .bind(expires_at)
        .bind(token_hash)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// End one session. Ending one that is not there is not an error.
    pub async fn delete_session(&self, token_hash: &[u8]) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// End every session except `keep`, if given. Returns how many were ended.
    pub async fn delete_sessions_except(&self, keep: Option<&[u8]>) -> Result<u64, StoreError> {
        let result = sqlx::query("DELETE FROM sessions WHERE token_hash IS NOT ?")
            .bind(keep)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Remove the sessions that were over at `now`. Returns how many there were.
    pub async fn purge_expired_sessions(&self, now: i64) -> Result<u64, StoreError> {
        let result = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}
