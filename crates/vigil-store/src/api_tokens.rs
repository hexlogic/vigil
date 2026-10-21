//! Tokens for the REST API.
//!
//! Like sessions, the store never sees a token, only the hash the caller derives from it, so
//! nothing in the database (or in a backup of it) can be used to call the API.

use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use uuid::Uuid;

use crate::{Store, StoreError, now_ms};

/// Identifier of an API token (a UUID v7).
pub type ApiTokenId = Uuid;

/// Shortest id suffix accepted by [`Store::resolve_api_token`].
const MIN_SUFFIX_LEN: usize = 6;

/// How long a token may go unrecorded as used, in milliseconds. Recording each request would
/// write to the database on every call.
const TOUCH_AFTER_MS: i64 = 60 * 1000;

/// What the database keeps about an API token. The token itself is not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiToken {
    /// Token id.
    pub id: ApiTokenId,
    /// Unique, human-chosen name.
    pub name: String,
    /// The first characters of the token, to tell tokens apart.
    pub prefix: String,
    /// When the token was made, in unix milliseconds.
    pub created_at: i64,
    /// When a request last used it, in unix milliseconds; `None` if it never was. Kept to
    /// the minute.
    pub last_used_at: Option<i64>,
}

fn token_from_row(row: &SqliteRow) -> Result<ApiToken, StoreError> {
    let id: String = row.try_get("id")?;
    Ok(ApiToken {
        id: Uuid::parse_str(&id)
            .map_err(|_| StoreError::Corrupt(format!("api token id {id:?}")))?,
        name: row.try_get("name")?,
        prefix: row.try_get("prefix")?,
        created_at: row.try_get("created_at")?,
        last_used_at: row.try_get("last_used_at")?,
    })
}

impl Store {
    /// Save a token identified by `token_hash`.
    ///
    /// A name that is already taken is [`StoreError::Conflict`].
    pub async fn create_api_token(
        &self,
        name: &str,
        token_hash: &[u8],
        prefix: &str,
    ) -> Result<ApiToken, StoreError> {
        let id = Uuid::now_v7();
        let inserted = sqlx::query(
            "INSERT INTO api_tokens (id, name, token_hash, prefix, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(token_hash)
        .bind(prefix)
        .bind(now_ms())
        .execute(&self.pool)
        .await;
        match inserted {
            Ok(_) => self.get_api_token(id).await,
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(StoreError::Conflict(
                format!("an API token named {name:?} already exists"),
            )),
            Err(e) => Err(e.into()),
        }
    }

    /// One token by id.
    pub async fn get_api_token(&self, id: ApiTokenId) -> Result<ApiToken, StoreError> {
        let row = sqlx::query(
            "SELECT id, name, prefix, created_at, last_used_at FROM api_tokens WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound(format!("API token {id}")))?;
        token_from_row(&row)
    }

    /// All tokens, oldest first.
    pub async fn list_api_tokens(&self) -> Result<Vec<ApiToken>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, name, prefix, created_at, last_used_at FROM api_tokens \
             ORDER BY created_at, id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(token_from_row).collect()
    }

    /// Revoke a token.
    pub async fn delete_api_token(&self, id: ApiTokenId) -> Result<(), StoreError> {
        let result = sqlx::query("DELETE FROM api_tokens WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("API token {id}")));
        }
        Ok(())
    }

    /// Resolve user input to a token: an exact name, a full id, or the last 6 or more hex
    /// digits of one, as for targets.
    pub async fn resolve_api_token(&self, input: &str) -> Result<ApiToken, StoreError> {
        let input = input.trim();
        let by_name = sqlx::query(
            "SELECT id, name, prefix, created_at, last_used_at FROM api_tokens WHERE name = ?",
        )
        .bind(input)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = by_name {
            return token_from_row(&row);
        }
        if let Ok(id) = Uuid::parse_str(input) {
            return self.get_api_token(id).await;
        }
        let suffix = input.to_ascii_lowercase();
        let usable = suffix.len() >= MIN_SUFFIX_LEN
            && suffix.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        if !usable {
            return Err(StoreError::NotFound(format!("API token {input:?}")));
        }
        // The suffix only holds hex digits and hyphens, so it has no LIKE wildcards in it.
        let rows = sqlx::query(
            "SELECT id, name, prefix, created_at, last_used_at FROM api_tokens \
             WHERE id LIKE ? LIMIT 2",
        )
        .bind(format!("%{suffix}"))
        .fetch_all(&self.pool)
        .await?;
        match rows.as_slice() {
            [] => Err(StoreError::NotFound(format!("API token {input:?}"))),
            [row] => token_from_row(row),
            _ => Err(StoreError::AmbiguousId(input.to_owned())),
        }
    }

    /// The token identified by `token_hash`, and a note that it was used at `now`. The note is
    /// written at most once a minute.
    ///
    /// The token is read first and the note is only written when it is due: a write takes
    /// SQLite's single write lock even when it changes no row, and almost every request of a
    /// busy client finds the note fresh.
    pub async fn use_api_token(
        &self,
        token_hash: &[u8],
        now: i64,
    ) -> Result<Option<ApiToken>, StoreError> {
        let Some(mut token) = self.api_token_by_hash(token_hash).await? else {
            return Ok(None);
        };
        if token
            .last_used_at
            .is_some_and(|used| used > now - TOUCH_AFTER_MS)
        {
            return Ok(Some(token));
        }
        // The same condition again: a request that was here at the same time may have noted it.
        let noted = sqlx::query(
            "UPDATE api_tokens SET last_used_at = ?1 \
             WHERE token_hash = ?2 AND (last_used_at IS NULL OR last_used_at <= ?1 - ?3)",
        )
        .bind(now)
        .bind(token_hash)
        .bind(TOUCH_AFTER_MS)
        .execute(&self.pool)
        .await?;
        if noted.rows_affected() == 1 {
            token.last_used_at = Some(now);
            return Ok(Some(token));
        }
        self.api_token_by_hash(token_hash).await
    }

    async fn api_token_by_hash(&self, token_hash: &[u8]) -> Result<Option<ApiToken>, StoreError> {
        let row = sqlx::query(
            "SELECT id, name, prefix, created_at, last_used_at FROM api_tokens \
             WHERE token_hash = ?",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(token_from_row).transpose()
    }
}
