//! One-shot CLI commands that work directly on the database.

pub mod check;
pub mod diff;
pub mod notify;
pub mod preview;
pub mod settings;
pub mod token;
pub mod watch;

use anyhow::Context;
use vigil_core::WatchId;
use vigil_store::Store;

use crate::config::Config;

/// Open the database in the configured data directory, creating it if needed.
pub async fn open_store(config: &Config) -> anyhow::Result<Store> {
    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("creating data dir {}", config.data_dir.display()))?;
    let path = config.db_path();
    Store::open(&path)
        .await
        .with_context(|| format!("opening database {}", path.display()))
}

/// Turn a full watch id or a unique suffix of one into the id of an existing watch.
pub async fn resolve(store: &Store, input: &str) -> anyhow::Result<WatchId> {
    store.resolve_watch_id(input).await.map_err(Into::into)
}
