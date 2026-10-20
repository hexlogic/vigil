//! SQLite repository, migrations and snapshot storage.

mod checks;
mod snapshots;
mod watches;

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

pub use checks::{CHECK_LOG_KEEP, CheckSuccess, Content, RecordOutcome};
pub use snapshots::{SnapshotId, SnapshotMeta};
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
    /// A watch id suffix matches more than one watch.
    #[error("{0:?} matches more than one watch; use more characters")]
    AmbiguousId(String),
    /// The text is neither a full watch id nor a usable id suffix.
    #[error("{0:?} is not a watch id: give the full id or at least 6 trailing hex characters")]
    InvalidId(String),
}

/// Handle to the Vigil SQLite database. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Open (creating if needed) the database at `path` and apply pending migrations.
    ///
    /// The connection uses WAL journaling and enforces foreign keys.
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
        sqlx::migrate!("../../migrations").run(&pool).await?;
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
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}
