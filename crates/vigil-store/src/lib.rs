//! SQLite repository, migrations and snapshot storage.

mod api_tokens;
mod check_log;
mod checks;
mod deliveries;
mod import;
mod links;
mod migrate;
mod outbox;
mod sessions;
mod settings;
mod snapshots;
mod tags;
mod targets;
mod watches;

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

pub use api_tokens::{ApiToken, ApiTokenId};
pub use check_log::{CheckEntry, CheckOutcome};
pub use checks::{CHECK_LOG_KEEP, CheckSuccess, Content, RecordOutcome};
pub use deliveries::{DELIVERY_LOG_KEEP, DeliveryOutcome, DeliveryRecord};
pub use import::{
    ImportBatch, ImportOutcome, ImportTag, ImportTarget, ImportWatch, free_name, match_name,
};
pub use links::WatchLinks;
pub use outbox::{NewPending, PendingNotification};
pub use sessions::Session;
pub use snapshots::{SnapshotId, SnapshotMeta};
pub use tags::{NewTag, Tag, TagId};
pub use targets::{NewTarget, NotificationTarget, TargetId};
pub use watches::NewWatch;

/// Errors returned by the storage layer.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A database query or connection failed.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    /// Applying the embedded migrations failed.
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// The copy of the database made before migrating could not be written.
    #[error("could not back up the database before migrating: {0}")]
    Backup(String),
    /// Stored JSON could not be read or written.
    #[error("invalid stored json: {0}")]
    Json(#[from] serde_json::Error),
    /// Compressing or decompressing snapshot content failed.
    #[error("snapshot compression error: {0}")]
    Compression(std::io::Error),
    /// A row holds a value this version cannot interpret.
    #[error("corrupt data: {0}")]
    Corrupt(String),
    /// No watch or snapshot matches.
    #[error("not found: {0}")]
    NotFound(String),
    /// A notification target id suffix matches more than one target.
    #[error("{0:?} matches more than one notification target; use more characters")]
    AmbiguousTarget(String),
    /// The change would clash with something that already exists, such as a taken name.
    #[error("{0}")]
    Conflict(String),
    /// A watch id suffix matches more than one watch.
    #[error("{0:?} matches more than one watch; use more characters")]
    AmbiguousId(String),
    /// The text is neither a full watch id nor a usable id suffix.
    #[error("{0:?} is not a watch id: give the full id or at least 6 trailing hex characters")]
    InvalidId(String),
}

/// The migrations embedded in the binary.
static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

/// Handle to the Vigil SQLite database. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Open (creating if needed) the database at `path` and apply pending migrations.
    ///
    /// The connection uses WAL journaling and enforces foreign keys. When an existing database
    /// has migrations to apply, a copy named `<file>.backup-v<version>-<unix ms>` is written
    /// first; if that fails, so does `open`.
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;
        if let Some(backup) = migrate::backup_if_pending(&pool, path, &MIGRATOR).await? {
            tracing::info!(backup = %backup.display(), "backed up the database before migrating");
        }
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// The underlying connection pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Check that the database answers a trivial query.
    pub async fn ping(&self) -> Result<(), StoreError> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }
}

/// Current time as unix milliseconds.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}
