//! Integration tests for opening the store and the initial schema.

use sqlx::Row;
use vigil_store::{Store, StoreError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn open_temp() -> Result<(tempfile::TempDir, Store), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    Ok((dir, store))
}

async fn insert_watch(store: &Store, id: &str) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO watches (id, name, url, schedule, created_at, updated_at) \
         VALUES (?, 'w', 'https://example.com', '{\"interval\":300}', 0, 0)",
    )
    .bind(id)
    .execute(store.pool())
    .await?;
    Ok(())
}

#[tokio::test]
async fn open_applies_migrations() -> TestResult {
    let (_dir, store) = open_temp().await?;
    store.ping().await?;

    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table'")
        .fetch_all(store.pool())
        .await?;
    let tables: Vec<String> = rows.iter().map(|r| r.get("name")).collect();
    for expected in [
        "watches",
        "snapshots",
        "tags",
        "notification_targets",
        "check_log",
        "watch_tags",
        "watch_notifications",
        "tag_notifications",
    ] {
        assert!(
            tables.iter().any(|t| t == expected),
            "missing table {expected}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn open_enables_wal_and_foreign_keys() -> TestResult {
    let (_dir, store) = open_temp().await?;

    let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(store.pool())
        .await?;
    assert_eq!(mode, "wal");

    let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(store.pool())
        .await?;
    assert_eq!(fk, 1);
    Ok(())
}

#[tokio::test]
async fn reopen_is_idempotent() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("vigil.db");
    drop(Store::open(&path).await?);
    let store = Store::open(&path).await?;
    let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(store.pool())
        .await?;
    let embedded = sqlx::migrate!("../../migrations").iter().count();
    assert_eq!(usize::try_from(applied)?, embedded);
    let backups = std::fs::read_dir(dir.path())?
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".backup-"))
        .count();
    assert_eq!(backups, 0, "an up-to-date database is not backed up");
    Ok(())
}

#[tokio::test]
async fn deleting_a_watch_cascades() -> TestResult {
    let (_dir, store) = open_temp().await?;
    insert_watch(&store, "w1").await?;
    sqlx::query(
        "INSERT INTO snapshots (id, watch_id, taken_at, hash, content, raw_size) \
         VALUES ('s1', 'w1', 0, x'00', x'00', 1)",
    )
    .execute(store.pool())
    .await?;
    sqlx::query(
        "INSERT INTO check_log (watch_id, at, duration_ms, outcome) VALUES ('w1', 0, 1, 'changed')",
    )
    .execute(store.pool())
    .await?;

    sqlx::query("DELETE FROM watches WHERE id = 'w1'")
        .execute(store.pool())
        .await?;

    let snapshots: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM snapshots")
        .fetch_one(store.pool())
        .await?;
    let check_log: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM check_log")
        .fetch_one(store.pool())
        .await?;
    assert_eq!(snapshots, 0, "snapshots should cascade");
    assert_eq!(check_log, 0, "check_log should cascade");
    Ok(())
}

#[tokio::test]
async fn invalid_check_outcome_is_rejected() -> TestResult {
    let (_dir, store) = open_temp().await?;
    insert_watch(&store, "w1").await?;
    let result = sqlx::query(
        "INSERT INTO check_log (watch_id, at, duration_ms, outcome) VALUES ('w1', 0, 1, 'bogus')",
    )
    .execute(store.pool())
    .await;
    assert!(result.is_err());
    Ok(())
}
