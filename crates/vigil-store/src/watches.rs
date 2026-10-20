//! Watch persistence.

use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use uuid::Uuid;
use vigil_core::{ContentHash, FetchSettings, Processor, Schedule, Watch, WatchId, WatchState};

use crate::{Store, StoreError, now_ms};

/// Shortest id suffix accepted by [`Store::resolve_watch_id`].
const MIN_SUFFIX_LEN: usize = 6;

macro_rules! watch_columns {
    () => {
        "id, name, url, paused, fetch, schedule, jitter_secs, processor, snapshot_retention, \
         last_checked_at, last_changed_at, last_error, consecutive_errors, last_hash, etag, \
         last_modified, created_at, updated_at"
    };
}

/// Settings for a watch that does not exist yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWatch {
    /// Display name.
    pub name: String,
    /// The URL to fetch.
    pub url: String,
    /// HTTP settings.
    pub fetch: FetchSettings,
    /// When to check.
    pub schedule: Schedule,
    /// Maximum random delay per run in seconds; 0 selects the scheduler default.
    pub jitter_secs: u32,
    /// How bodies are turned into text.
    pub processor: Processor,
    /// How many snapshots to keep.
    pub snapshot_retention: u32,
}

impl NewWatch {
    /// A watch on `url` named after it, with the `text` processor and 100 snapshots kept.
    pub fn new(url: impl Into<String>, schedule: Schedule) -> Self {
        let url = url.into();
        Self {
            name: url.clone(),
            url,
            fetch: FetchSettings::default(),
            schedule,
            jitter_secs: 0,
            processor: Processor::Text,
            snapshot_retention: 100,
        }
    }
}

impl Store {
    /// Insert a watch with a fresh UUID v7 id and return it as stored.
    pub async fn create_watch(&self, new: NewWatch) -> Result<Watch, StoreError> {
        let id = WatchId::from_uuid(Uuid::now_v7());
        let now = now_ms();
        sqlx::query(
            "INSERT INTO watches \
             (id, name, url, fetch, schedule, jitter_secs, processor, snapshot_retention, \
              created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
        )
        .bind(id.to_string())
        .bind(&new.name)
        .bind(&new.url)
        .bind(serde_json::to_string(&new.fetch)?)
        .bind(serde_json::to_string(&new.schedule)?)
        .bind(i64::from(new.jitter_secs))
        .bind(new.processor.as_str())
        .bind(i64::from(new.snapshot_retention))
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.get_watch(id).await
    }

    /// Load one watch.
    pub async fn get_watch(&self, id: WatchId) -> Result<Watch, StoreError> {
        let row = sqlx::query(concat!(
            "SELECT ",
            watch_columns!(),
            " FROM watches WHERE id = ?"
        ))
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => watch_from_row(&row),
            None => Err(StoreError::NotFound(format!("watch {id}"))),
        }
    }

    /// All watches, oldest first.
    pub async fn list_watches(&self) -> Result<Vec<Watch>, StoreError> {
        let rows = sqlx::query(concat!(
            "SELECT ",
            watch_columns!(),
            " FROM watches ORDER BY created_at, id"
        ))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(watch_from_row).collect()
    }

    /// Delete a watch together with its snapshots and check log.
    pub async fn delete_watch(&self, id: WatchId) -> Result<(), StoreError> {
        let result = sqlx::query("DELETE FROM watches WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("watch {id}")));
        }
        Ok(())
    }

    /// Resolve user input to a watch id.
    ///
    /// Accepts a full id, or the trailing characters of one (at least 6 hex digits, hyphens
    /// allowed). Suffixes are matched instead of prefixes because the leading bits of a UUID v7
    /// are a timestamp: watches added around the same time share long prefixes, and only the
    /// random tail tells them apart.
    pub async fn resolve_watch_id(&self, input: &str) -> Result<WatchId, StoreError> {
        let input = input.trim();
        if let Ok(id) = input.parse::<WatchId>() {
            return self.get_watch(id).await.map(|w| w.id);
        }

        let suffix = input.to_ascii_lowercase();
        let usable = suffix.len() >= MIN_SUFFIX_LEN
            && suffix.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        if !usable {
            return Err(StoreError::InvalidId(input.to_owned()));
        }

        // The suffix only holds hex digits and hyphens, so it has no LIKE wildcards in it.
        let rows = sqlx::query("SELECT id FROM watches WHERE id LIKE ? LIMIT 2")
            .bind(format!("%{suffix}"))
            .fetch_all(&self.pool)
            .await?;
        match rows.as_slice() {
            [] => Err(StoreError::NotFound(format!("watch matching {input:?}"))),
            [row] => parse_id(&row.try_get::<String, _>("id")?),
            _ => Err(StoreError::AmbiguousId(input.to_owned())),
        }
    }
}

fn parse_id(text: &str) -> Result<WatchId, StoreError> {
    text.parse()
        .map_err(|_| StoreError::Corrupt(format!("invalid watch id {text:?}")))
}

fn watch_from_row(row: &SqliteRow) -> Result<Watch, StoreError> {
    let processor: String = row.try_get("processor")?;
    let last_hash = row
        .try_get::<Option<Vec<u8>>, _>("last_hash")?
        .map(|bytes| {
            ContentHash::from_slice(&bytes)
                .ok_or_else(|| StoreError::Corrupt("last_hash is not 32 bytes".into()))
        })
        .transpose()?;

    Ok(Watch {
        id: parse_id(&row.try_get::<String, _>("id")?)?,
        name: row.try_get("name")?,
        url: row.try_get("url")?,
        paused: row.try_get::<i64, _>("paused")? != 0,
        fetch: serde_json::from_str(&row.try_get::<String, _>("fetch")?)?,
        schedule: serde_json::from_str(&row.try_get::<String, _>("schedule")?)?,
        jitter_secs: to_u32(row, "jitter_secs")?,
        processor: processor
            .parse()
            .map_err(|_| StoreError::Corrupt(format!("unknown processor {processor:?}")))?,
        snapshot_retention: to_u32(row, "snapshot_retention")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        state: WatchState {
            last_checked_at: row.try_get("last_checked_at")?,
            last_changed_at: row.try_get("last_changed_at")?,
            last_error: row.try_get("last_error")?,
            consecutive_errors: to_u32(row, "consecutive_errors")?,
            last_hash,
            etag: row.try_get("etag")?,
            last_modified: row.try_get("last_modified")?,
        },
    })
}

/// Read an integer column that must fit in a `u32`.
pub(crate) fn to_u32(row: &SqliteRow, column: &str) -> Result<u32, StoreError> {
    let value: i64 = row.try_get(column)?;
    u32::try_from(value).map_err(|_| StoreError::Corrupt(format!("{column} out of range: {value}")))
}
