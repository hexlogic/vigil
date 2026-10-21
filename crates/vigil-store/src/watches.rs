//! Watch persistence.

use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use uuid::Uuid;
use vigil_core::{
    Conditions, ContentHash, FetchSettings, Processor, Schedule, Step, Watch, WatchId, WatchState,
};

use crate::{Store, StoreError, now_ms};

/// Shortest id suffix accepted by [`Store::resolve_watch_id`].
const MIN_SUFFIX_LEN: usize = 6;

macro_rules! watch_columns {
    () => {
        "id, name, url, paused, fetch, schedule, jitter_secs, processor, pipeline, conditions, \
         snapshot_retention, notification_template_override, last_checked_at, last_changed_at, last_error, consecutive_errors, \
         last_hash, etag, last_modified, created_at, updated_at"
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
    /// Steps applied after the processor, in order.
    pub pipeline: Vec<Step>,
    /// Which changes are worth a notification.
    pub conditions: Conditions,
    /// How many snapshots to keep.
    pub snapshot_retention: u32,
}

impl NewWatch {
    /// A watch on `url` named after it, with the `text` processor, no steps, no conditions and
    /// 100 snapshots kept.
    pub fn new(url: impl Into<String>, schedule: Schedule) -> Self {
        let url = url.into();
        Self {
            name: url.clone(),
            url,
            fetch: FetchSettings::default(),
            schedule,
            jitter_secs: 0,
            processor: Processor::Text,
            pipeline: Vec::new(),
            conditions: Conditions::default(),
            snapshot_retention: 100,
        }
    }
}

impl From<&Watch> for NewWatch {
    /// The editable settings of an existing watch, to change a few and pass to
    /// [`Store::update_watch`].
    fn from(watch: &Watch) -> Self {
        Self {
            name: watch.name.clone(),
            url: watch.url.clone(),
            fetch: watch.fetch.clone(),
            schedule: watch.schedule,
            jitter_secs: watch.jitter_secs,
            processor: watch.processor,
            pipeline: watch.pipeline.clone(),
            conditions: watch.conditions.clone(),
            snapshot_retention: watch.snapshot_retention,
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
             (id, name, url, fetch, schedule, jitter_secs, processor, pipeline, conditions, \
              snapshot_retention, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        )
        .bind(id.to_string())
        .bind(&new.name)
        .bind(&new.url)
        .bind(serde_json::to_string(&new.fetch)?)
        .bind(serde_json::to_string(&new.schedule)?)
        .bind(i64::from(new.jitter_secs))
        .bind(new.processor.as_str())
        .bind(serde_json::to_string(&new.pipeline)?)
        .bind(serde_json::to_string(&new.conditions)?)
        .bind(i64::from(new.snapshot_retention))
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.get_watch(id).await
    }

    /// Replace a watch's settings and return it as stored.
    ///
    /// The pause flag, the hash and the snapshots and log are kept. The remembered `ETag` and
    /// `Last-Modified` are cleared: a server that answers 304 to a conditional request would
    /// otherwise never send the body again, and an edited pipeline would never run. The
    /// failure streak and last error are cleared too, so an edit that fixes a failing watch
    /// is not made to wait out its backoff. The first check after an edit may report the
    /// new output as a change.
    pub async fn update_watch(
        &self,
        id: WatchId,
        settings: &NewWatch,
    ) -> Result<Watch, StoreError> {
        let result = sqlx::query(
            "UPDATE watches SET name = ?2, url = ?3, fetch = ?4, schedule = ?5, \
             jitter_secs = ?6, processor = ?7, pipeline = ?8, conditions = ?9, \
             snapshot_retention = ?10, updated_at = MAX(?11, updated_at + 1), etag = NULL, \
             last_modified = NULL, \
             consecutive_errors = 0, last_error = NULL WHERE id = ?1",
        )
        .bind(id.to_string())
        .bind(&settings.name)
        .bind(&settings.url)
        .bind(serde_json::to_string(&settings.fetch)?)
        .bind(serde_json::to_string(&settings.schedule)?)
        .bind(i64::from(settings.jitter_secs))
        .bind(settings.processor.as_str())
        .bind(serde_json::to_string(&settings.pipeline)?)
        .bind(serde_json::to_string(&settings.conditions)?)
        .bind(i64::from(settings.snapshot_retention))
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("watch {id}")));
        }
        self.get_watch(id).await
    }

    /// Set or clear a watch's notification template.
    ///
    /// Unlike [`Store::update_watch`] this leaves `updated_at`, the remembered validators and
    /// the failure streak alone: a message template does not change what a check fetches or
    /// when it runs, so there is nothing to reschedule or re-baseline.
    pub async fn set_notification_template(
        &self,
        id: WatchId,
        template: Option<&str>,
    ) -> Result<(), StoreError> {
        let result =
            sqlx::query("UPDATE watches SET notification_template_override = ? WHERE id = ?")
                .bind(template)
                .bind(id.to_string())
                .execute(&self.pool)
                .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("watch {id}")));
        }
        Ok(())
    }

    /// Pause or resume a watch.
    ///
    /// Like [`Store::set_notification_template`] this leaves `updated_at` and the check state
    /// alone: pausing changes whether a watch is scheduled, not what a check does. A scheduler
    /// notices the flag the next time it loads the watch list.
    pub async fn set_paused(&self, id: WatchId, paused: bool) -> Result<(), StoreError> {
        let result = sqlx::query("UPDATE watches SET paused = ? WHERE id = ?")
            .bind(i64::from(paused))
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("watch {id}")));
        }
        Ok(())
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

    /// All readable watches, oldest first.
    ///
    /// A row that cannot be interpreted (corrupt JSON, a value from another version) is logged
    /// and skipped, so one bad row cannot stop every other watch from being scheduled or
    /// listed. [`Store::get_watch`] still reports the error for that row.
    pub async fn list_watches(&self) -> Result<Vec<Watch>, StoreError> {
        let rows = sqlx::query(concat!(
            "SELECT ",
            watch_columns!(),
            " FROM watches ORDER BY created_at, id"
        ))
        .fetch_all(&self.pool)
        .await?;
        let mut watches = Vec::with_capacity(rows.len());
        for row in &rows {
            match watch_from_row(row) {
                Ok(watch) => watches.push(watch),
                Err(error) => {
                    let id = row.try_get::<String, _>("id").unwrap_or_default();
                    tracing::warn!(watch = %id, %error, "skipping unreadable watch");
                }
            }
        }
        Ok(watches)
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
        pipeline: serde_json::from_str(&row.try_get::<String, _>("pipeline")?)?,
        conditions: serde_json::from_str(&row.try_get::<String, _>("conditions")?)?,
        snapshot_retention: to_u32(row, "snapshot_retention")?,
        notification_template: row.try_get("notification_template_override")?,
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
