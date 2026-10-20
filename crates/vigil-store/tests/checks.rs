//! Check recording tests: dedupe, state updates, retention and the check log.

use vigil_core::{ContentHash, Schedule, Watch};
use vigil_store::{
    CHECK_LOG_KEEP, CheckSuccess, Content, NewWatch, RecordOutcome, Store, StoreError,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn open_temp() -> Result<(tempfile::TempDir, Store), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    Ok((dir, store))
}

async fn new_watch(store: &Store, retention: u32) -> Result<Watch, StoreError> {
    let mut new = NewWatch::new("https://example.com", Schedule::Interval(60));
    new.snapshot_retention = retention;
    store.create_watch(new).await
}

fn success(at: i64, text: &str) -> CheckSuccess {
    CheckSuccess {
        at,
        duration_ms: 12,
        http_status: Some(200),
        content_type: Some("text/html".into()),
        etag: None,
        last_modified: None,
        content: Some(Content {
            text: text.into(),
            raw_size: text.len() as u64 + 100,
        }),
    }
}

fn not_modified(at: i64) -> CheckSuccess {
    CheckSuccess {
        http_status: Some(304),
        content_type: None,
        content: None,
        ..success(at, "")
    }
}

async fn count(store: &Store, table: &str, watch: &Watch) -> Result<i64, sqlx::Error> {
    let sql = match table {
        "snapshots" => "SELECT COUNT(*) FROM snapshots WHERE watch_id = ?",
        _ => "SELECT COUNT(*) FROM check_log WHERE watch_id = ?",
    };
    sqlx::query_scalar(sql)
        .bind(watch.id.to_string())
        .fetch_one(store.pool())
        .await
}

async fn log_outcomes(store: &Store, watch: &Watch) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT outcome FROM check_log WHERE watch_id = ? ORDER BY id")
        .bind(watch.id.to_string())
        .fetch_all(store.pool())
        .await
}

#[tokio::test]
async fn baseline_then_unchanged_then_changed() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = new_watch(&store, 100).await?;

    let first = store
        .record_success(watch.id, success(1_000, "v1\n"))
        .await?;
    let RecordOutcome::Baseline { snapshot: first_id } = first else {
        return Err(format!("expected baseline, got {first:?}").into());
    };
    assert_eq!(count(&store, "snapshots", &watch).await?, 1);

    let same = store
        .record_success(watch.id, success(2_000, "v1\n"))
        .await?;
    assert_eq!(same, RecordOutcome::Unchanged);
    assert_eq!(count(&store, "snapshots", &watch).await?, 1);

    let second = store
        .record_success(watch.id, success(3_000, "v2\n"))
        .await?;
    let RecordOutcome::Changed { previous, current } = second else {
        return Err(format!("expected change, got {second:?}").into());
    };
    assert_eq!(previous, first_id);
    assert_eq!(count(&store, "snapshots", &watch).await?, 2);

    let snapshots = store.list_snapshots(watch.id, 10).await?;
    assert_eq!(
        snapshots.iter().map(|s| s.id).collect::<Vec<_>>(),
        [current, first_id]
    );
    assert_eq!(store.snapshot_content(first_id).await?, "v1\n");
    assert_eq!(store.snapshot_content(current).await?, "v2\n");
    assert_eq!(snapshots[0].hash, ContentHash::of("v2\n"));
    assert_eq!(snapshots[0].http_status, Some(200));
    assert_eq!(snapshots[0].content_type.as_deref(), Some("text/html"));
    assert_eq!(snapshots[0].raw_size, 103);

    assert_eq!(
        log_outcomes(&store, &watch).await?,
        ["changed", "unchanged", "changed"]
    );

    let state = store.get_watch(watch.id).await?.state;
    assert_eq!(state.last_checked_at, Some(3_000));
    assert_eq!(state.last_changed_at, Some(3_000));
    assert_eq!(state.last_hash, Some(ContentHash::of("v2\n")));
    Ok(())
}

#[tokio::test]
async fn identical_content_keeps_last_changed_at() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = new_watch(&store, 100).await?;
    store
        .record_success(watch.id, success(1_000, "same\n"))
        .await?;
    store
        .record_success(watch.id, success(5_000, "same\n"))
        .await?;

    let state = store.get_watch(watch.id).await?.state;
    assert_eq!(state.last_checked_at, Some(5_000));
    assert_eq!(state.last_changed_at, Some(1_000));
    Ok(())
}

#[tokio::test]
async fn validators_are_stored_and_a_304_keeps_them() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = new_watch(&store, 100).await?;
    let mut first = success(1_000, "v1\n");
    first.etag = Some("\"abc\"".into());
    first.last_modified = Some("Mon, 01 Jan 2024 00:00:00 GMT".into());
    store.record_success(watch.id, first).await?;

    let outcome = store.record_success(watch.id, not_modified(2_000)).await?;
    assert_eq!(outcome, RecordOutcome::Unchanged);
    assert_eq!(count(&store, "snapshots", &watch).await?, 1);

    let state = store.get_watch(watch.id).await?.state;
    assert_eq!(state.etag.as_deref(), Some("\"abc\""));
    assert_eq!(
        state.last_modified.as_deref(),
        Some("Mon, 01 Jan 2024 00:00:00 GMT")
    );
    assert_eq!(state.last_checked_at, Some(2_000));
    assert_eq!(state.last_changed_at, Some(1_000));
    assert_eq!(
        log_outcomes(&store, &watch).await?,
        ["changed", "unchanged"]
    );

    // A fresh 200 without validators clears the old ones.
    store
        .record_success(watch.id, success(3_000, "v1\n"))
        .await?;
    let state = store.get_watch(watch.id).await?.state;
    assert_eq!((state.etag, state.last_modified), (None, None));
    Ok(())
}

#[tokio::test]
async fn old_snapshots_are_pruned_to_the_retention_limit() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = new_watch(&store, 2).await?;
    for n in 1..=4 {
        let text = format!("version {n}\n");
        store
            .record_success(watch.id, success(n * 1_000, &text))
            .await?;
    }
    let snapshots = store.list_snapshots(watch.id, 10).await?;
    assert_eq!(snapshots.len(), 2);
    assert_eq!(
        store.snapshot_content(snapshots[0].id).await?,
        "version 4\n"
    );
    assert_eq!(
        store.snapshot_content(snapshots[1].id).await?,
        "version 3\n"
    );
    Ok(())
}

#[tokio::test]
async fn zero_retention_still_keeps_the_latest_snapshot() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = new_watch(&store, 0).await?;
    store
        .record_success(watch.id, success(1_000, "a\n"))
        .await?;
    store
        .record_success(watch.id, success(2_000, "b\n"))
        .await?;
    let snapshots = store.list_snapshots(watch.id, 10).await?;
    assert_eq!(snapshots.len(), 1);
    assert_eq!(store.snapshot_content(snapshots[0].id).await?, "b\n");
    Ok(())
}

#[tokio::test]
async fn check_log_is_trimmed_to_the_newest_rows() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = new_watch(&store, 100).await?;
    let extra = 5;
    for n in 0..(CHECK_LOG_KEEP + extra) {
        store
            .record_success(watch.id, success(n, "steady\n"))
            .await?;
    }
    assert_eq!(count(&store, "check_log", &watch).await?, CHECK_LOG_KEEP);
    let oldest: i64 = sqlx::query_scalar("SELECT MIN(at) FROM check_log WHERE watch_id = ?")
        .bind(watch.id.to_string())
        .fetch_one(store.pool())
        .await?;
    assert_eq!(oldest, extra);
    Ok(())
}

#[tokio::test]
async fn errors_are_counted_and_cleared_by_the_next_success() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = new_watch(&store, 100).await?;

    assert_eq!(
        store
            .record_error(watch.id, 1_000, 30, "connection refused")
            .await?,
        1
    );
    assert_eq!(
        store.record_error(watch.id, 2_000, 31, "timed out").await?,
        2
    );
    let state = store.get_watch(watch.id).await?.state;
    assert_eq!(state.consecutive_errors, 2);
    assert_eq!(state.last_error.as_deref(), Some("timed out"));
    assert_eq!(state.last_checked_at, Some(2_000));

    store
        .record_success(watch.id, success(3_000, "ok\n"))
        .await?;
    let state = store.get_watch(watch.id).await?.state;
    assert_eq!(state.consecutive_errors, 0);
    assert_eq!(state.last_error, None);

    assert_eq!(
        log_outcomes(&store, &watch).await?,
        ["error", "error", "changed"]
    );
    let message: Option<String> = sqlx::query_scalar(
        "SELECT error_message FROM check_log WHERE watch_id = ? ORDER BY id LIMIT 1",
    )
    .bind(watch.id.to_string())
    .fetch_one(store.pool())
    .await?;
    assert_eq!(message.as_deref(), Some("connection refused"));
    Ok(())
}

#[tokio::test]
async fn long_error_messages_are_truncated() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = new_watch(&store, 100).await?;
    store
        .record_error(watch.id, 1_000, 1, &"x".repeat(5_000))
        .await?;
    let state = store.get_watch(watch.id).await?.state;
    assert_eq!(state.last_error.map(|e| e.chars().count()), Some(1_000));
    Ok(())
}

#[tokio::test]
async fn recording_for_a_deleted_watch_is_not_found() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = new_watch(&store, 100).await?;
    store.delete_watch(watch.id).await?;
    assert!(matches!(
        store.record_success(watch.id, success(1, "x\n")).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.record_error(watch.id, 1, 1, "boom").await,
        Err(StoreError::NotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn unknown_snapshot_is_not_found() -> TestResult {
    let (_dir, store) = open_temp().await?;
    assert!(matches!(
        store.snapshot_content(uuid::Uuid::nil()).await,
        Err(StoreError::NotFound(_))
    ));
    Ok(())
}
