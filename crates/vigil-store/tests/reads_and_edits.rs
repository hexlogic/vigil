//! What the web UI needs on top of the CLI: pausing, editing a target, reading one snapshot
//! and reading the check log.

use uuid::Uuid;
use vigil_core::{Schedule, Watch, WatchId};
use vigil_store::{
    CheckOutcome, CheckSuccess, Content, NewTarget, NewWatch, RecordOutcome, Store, StoreError,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn open_temp() -> Result<(tempfile::TempDir, Store), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    Ok((dir, store))
}

async fn watch(store: &Store) -> Result<Watch, StoreError> {
    let new = NewWatch::new("https://example.com", Schedule::Interval(60));
    store.create_watch(new).await
}

fn target(name: &str, url: &str) -> NewTarget {
    NewTarget {
        name: name.to_owned(),
        url: url.to_owned(),
        default_template: None,
    }
}

fn success(at: i64, text: &str) -> CheckSuccess {
    CheckSuccess {
        at,
        duration_ms: 7,
        http_status: Some(200),
        content_type: Some("text/html".into()),
        etag: None,
        last_modified: None,
        content: Some(Content {
            text: text.into(),
            raw_size: 42,
        }),
        settings_version: None,
    }
}

#[tokio::test]
async fn pausing_keeps_everything_else_about_the_watch() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = watch(&store).await?;
    store.record_error(watch.id, 5, 1, "boom").await?;
    let before = store.get_watch(watch.id).await?;

    store.set_paused(watch.id, true).await?;
    let paused = store.get_watch(watch.id).await?;
    assert!(paused.paused);
    assert_eq!(paused.updated_at, before.updated_at);
    assert_eq!(paused.state, before.state);

    store.set_paused(watch.id, false).await?;
    assert!(!store.get_watch(watch.id).await?.paused);
    Ok(())
}

#[tokio::test]
async fn pausing_a_missing_watch_is_not_found() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let missing = WatchId::from_uuid(Uuid::now_v7());
    assert!(matches!(
        store.set_paused(missing, true).await,
        Err(StoreError::NotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn editing_a_target_keeps_its_links_and_enabled_flag() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = watch(&store).await?;
    let created = store
        .create_target(&target("phone", "ntfy://ntfy.example/old"))
        .await?;
    store.link_target(watch.id, created.id).await?;
    store.set_target_enabled(created.id, false).await?;

    let edited = store
        .update_target(
            created.id,
            &NewTarget {
                name: "mobile".into(),
                url: "ntfy://ntfy.example/new".into(),
                default_template: Some("{{ watch_name }}".into()),
            },
        )
        .await?;

    assert_eq!(edited.id, created.id);
    assert_eq!(edited.name, "mobile");
    assert_eq!(edited.url, "ntfy://ntfy.example/new");
    assert_eq!(edited.default_template.as_deref(), Some("{{ watch_name }}"));
    assert!(!edited.enabled);
    assert_eq!(edited.created_at, created.created_at);
    let linked = store.targets_for_watch(watch.id, false).await?;
    assert_eq!(linked.len(), 1);
    assert_eq!(linked[0].id, created.id);
    Ok(())
}

#[tokio::test]
async fn editing_a_target_to_a_taken_name_is_a_conflict() -> TestResult {
    let (_dir, store) = open_temp().await?;
    store.create_target(&target("one", "ntfy://h/one")).await?;
    let two = store.create_target(&target("two", "ntfy://h/two")).await?;

    let result = store
        .update_target(two.id, &target("one", "ntfy://h/two"))
        .await;
    assert!(matches!(result, Err(StoreError::Conflict(_))));
    // Keeping its own name is not a clash.
    store
        .update_target(two.id, &target("two", "ntfy://h/other"))
        .await?;
    Ok(())
}

#[tokio::test]
async fn editing_a_missing_target_is_not_found() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let result = store
        .update_target(Uuid::now_v7(), &target("x", "ntfy://h/x"))
        .await;
    assert!(matches!(result, Err(StoreError::NotFound(_))));
    Ok(())
}

#[tokio::test]
async fn one_snapshot_can_be_read_by_id() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = watch(&store).await?;
    let RecordOutcome::Baseline { snapshot } = store
        .record_success(watch.id, &success(1_000, "hello"))
        .await?
    else {
        return Err("expected a baseline".into());
    };

    let meta = store.get_snapshot(snapshot).await?;
    assert_eq!(meta.id, snapshot);
    assert_eq!(meta.watch_id, watch.id);
    assert_eq!(meta.taken_at, 1_000);
    assert_eq!(meta.http_status, Some(200));
    assert_eq!(meta.content_type.as_deref(), Some("text/html"));
    assert_eq!(store.list_snapshots(watch.id, 1).await?, [meta]);

    assert!(matches!(
        store.get_snapshot(Uuid::now_v7()).await,
        Err(StoreError::NotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_snapshot_removed_by_retention_is_not_found() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let mut new = NewWatch::new("https://example.com", Schedule::Interval(60));
    new.snapshot_retention = 1;
    let watch = store.create_watch(new).await?;
    let RecordOutcome::Baseline { snapshot: first } =
        store.record_success(watch.id, &success(1, "one")).await?
    else {
        return Err("expected a baseline".into());
    };
    store.record_success(watch.id, &success(2, "two")).await?;

    assert!(matches!(
        store.get_snapshot(first).await,
        Err(StoreError::NotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn the_check_log_is_read_newest_first_with_errors() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = watch(&store).await?;
    store.record_success(watch.id, &success(10, "a")).await?;
    store.record_success(watch.id, &success(20, "a")).await?;
    store
        .record_error(watch.id, 30, 99, "connection refused")
        .await?;
    store.record_success(watch.id, &success(40, "b")).await?;

    let checks = store.list_checks(watch.id, 10).await?;
    let summary: Vec<_> = checks.iter().map(|c| (c.at, c.outcome)).collect();
    assert_eq!(
        summary,
        [
            (40, CheckOutcome::Changed),
            (30, CheckOutcome::Error),
            (20, CheckOutcome::Unchanged),
            // The first check stores the baseline and is logged as a change.
            (10, CheckOutcome::Changed),
        ]
    );
    assert_eq!(checks[1].duration_ms, 99);
    assert_eq!(checks[1].error.as_deref(), Some("connection refused"));
    assert_eq!(checks[0].error, None);

    assert_eq!(store.list_checks(watch.id, 2).await?.len(), 2);
    let other = self::watch(&store).await?;
    assert!(store.list_checks(other.id, 10).await?.is_empty());
    Ok(())
}
