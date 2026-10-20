//! Notification targets, watch links, the delivery log and the migration that adds them.

use sqlx::Row;
use vigil_core::{Schedule, WatchId};
use vigil_store::{
    DELIVERY_LOG_KEEP, DeliveryOutcome, DeliveryRecord, NewTarget, NewWatch, Store, StoreError,
    TargetId,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn open_temp() -> Result<(tempfile::TempDir, Store), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    Ok((dir, store))
}

fn target(name: &str) -> NewTarget {
    NewTarget {
        name: name.to_owned(),
        url: format!("ntfy://ntfy.example/{name}"),
        default_template: None,
    }
}

async fn watch(store: &Store) -> Result<WatchId, StoreError> {
    let new = NewWatch::new("https://example.com", Schedule::Interval(60));
    Ok(store.create_watch(new).await?.id)
}

fn record(watch: Option<WatchId>, target: TargetId, at: i64) -> DeliveryRecord {
    DeliveryRecord {
        watch,
        target,
        snapshot: None,
        at,
        attempts: 1,
        outcome: DeliveryOutcome::Sent,
        error: None,
    }
}

#[tokio::test]
async fn targets_round_trip() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let created = store
        .create_target(&NewTarget {
            default_template: Some("{{ watch_name }}".into()),
            ..target("phone")
        })
        .await?;
    assert_eq!(created.name, "phone");
    assert!(created.enabled);
    assert_eq!(
        created.default_template.as_deref(),
        Some("{{ watch_name }}")
    );
    assert_eq!(store.get_target(created.id).await?, created);

    store.create_target(&target("a-first")).await?;
    let names: Vec<_> = store
        .list_targets()
        .await?
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["a-first", "phone"]);

    store.delete_target(created.id).await?;
    assert!(matches!(
        store.get_target(created.id).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.delete_target(created.id).await,
        Err(StoreError::NotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_taken_name_is_a_conflict() -> TestResult {
    let (_dir, store) = open_temp().await?;
    store.create_target(&target("phone")).await?;
    let error = store.create_target(&target("phone")).await;
    assert!(matches!(error, Err(StoreError::Conflict(_))), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn resolve_by_name_id_and_suffix() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let phone = store.create_target(&target("phone")).await?;
    let id = phone.id.to_string();

    assert_eq!(store.resolve_target("phone").await?.id, phone.id);
    assert_eq!(store.resolve_target(&id).await?.id, phone.id);
    assert_eq!(
        store.resolve_target(&id[id.len() - 8..]).await?.id,
        phone.id
    );
    for bad in ["nope", "abc", "deadbeefdeadbeef"] {
        let error = store.resolve_target(bad).await;
        assert!(matches!(error, Err(StoreError::NotFound(_))), "{bad}");
    }
    Ok(())
}

#[tokio::test]
async fn a_name_wins_over_an_id_suffix() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let other = store.create_target(&target("other")).await?;
    let suffix = other.id.to_string()[28..].to_owned();
    let named = store.create_target(&target(&suffix)).await?;
    assert_eq!(store.resolve_target(&suffix).await?.id, named.id);
    Ok(())
}

#[tokio::test]
async fn links_are_idempotent_and_filtered_by_enabled() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = watch(&store).await?;
    let a = store.create_target(&target("a")).await?;
    let b = store.create_target(&target("b")).await?;

    store.link_target(watch, a.id).await?;
    store.link_target(watch, a.id).await?;
    store.link_target(watch, b.id).await?;
    assert_eq!(store.targets_for_watch(watch, false).await?.len(), 2);

    store.set_target_enabled(b.id, false).await?;
    let enabled = store.targets_for_watch(watch, true).await?;
    assert_eq!(enabled.iter().map(|t| t.id).collect::<Vec<_>>(), [a.id]);
    assert_eq!(store.targets_for_watch(watch, false).await?.len(), 2);

    store.unlink_target(watch, a.id).await?;
    store.unlink_target(watch, a.id).await?;
    assert!(store.targets_for_watch(watch, true).await?.is_empty());

    let counts = store.linked_watch_counts().await?;
    assert_eq!(counts.get(&b.id), Some(&1));
    assert_eq!(counts.get(&a.id), None);
    Ok(())
}

#[tokio::test]
async fn linking_something_missing_is_not_found() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = watch(&store).await?;
    let ghost = TargetId::now_v7();
    let error = store.link_target(watch, ghost).await;
    assert!(matches!(error, Err(StoreError::NotFound(_))), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn deleting_a_watch_or_target_removes_links_and_log() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let (w1, w2) = (watch(&store).await?, watch(&store).await?);
    let (t1, t2) = (
        store.create_target(&target("one")).await?,
        store.create_target(&target("two")).await?,
    );
    for (w, t) in [(w1, t1.id), (w2, t2.id)] {
        store.link_target(w, t).await?;
        store.record_delivery(&record(Some(w), t, 1)).await?;
    }

    store.delete_watch(w1).await?;
    store.delete_target(t2.id).await?;
    let links: i64 = sqlx::query("SELECT COUNT(*) AS n FROM watch_notifications")
        .fetch_one(store.pool())
        .await?
        .get("n");
    let log: i64 = sqlx::query("SELECT COUNT(*) AS n FROM delivery_log")
        .fetch_one(store.pool())
        .await?
        .get("n");
    assert_eq!((links, log), (0, 0));
    Ok(())
}

#[tokio::test]
async fn deliveries_are_listed_newest_first_and_errors_are_capped() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = watch(&store).await?;
    let t = store.create_target(&target("t")).await?;

    store.record_delivery(&record(Some(watch), t.id, 1)).await?;
    store
        .record_delivery(&DeliveryRecord {
            attempts: 4,
            outcome: DeliveryOutcome::Failed,
            error: Some("x".repeat(5000)),
            ..record(Some(watch), t.id, 2)
        })
        .await?;

    let rows = store.list_deliveries(watch, 10).await?;
    assert_eq!(rows.iter().map(|r| r.at).collect::<Vec<_>>(), [2, 1]);
    assert_eq!(rows[0].outcome, DeliveryOutcome::Failed);
    assert_eq!(rows[0].attempts, 4);
    assert_eq!(rows[0].error.as_deref().map(str::len), Some(1000));
    assert_eq!(store.list_deliveries(watch, 1).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn the_delivery_log_is_trimmed_per_watch_and_per_target_for_tests() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = watch(&store).await?;
    let (a, b) = (
        store.create_target(&target("a")).await?,
        store.create_target(&target("b")).await?,
    );
    for i in 0..DELIVERY_LOG_KEEP + 5 {
        store.record_delivery(&record(Some(watch), a.id, i)).await?;
        store.record_delivery(&record(None, a.id, i)).await?;
    }
    store.record_delivery(&record(None, b.id, 0)).await?;

    let count = |sql: &'static str| {
        let pool = store.pool().clone();
        async move {
            let n: i64 = sqlx::query(sql).fetch_one(&pool).await?.get("n");
            Ok::<_, sqlx::Error>(n)
        }
    };
    assert_eq!(
        count("SELECT COUNT(*) AS n FROM delivery_log WHERE watch_id IS NOT NULL").await?,
        DELIVERY_LOG_KEEP
    );
    assert_eq!(
        count("SELECT COUNT(*) AS n FROM delivery_log WHERE watch_id IS NULL").await?,
        DELIVERY_LOG_KEEP + 1
    );
    let oldest = store
        .list_deliveries(watch, u32::MAX)
        .await?
        .last()
        .map(|r| r.at);
    assert_eq!(oldest, Some(5));
    Ok(())
}

#[tokio::test]
async fn opening_a_version_one_database_backs_it_up_and_migrates_it() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("vigil.db");

    // Build a database as the previous release left it: only migration 1 applied.
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = sqlx::SqlitePool::connect_with(options).await?;
    sqlx::migrate!("../../migrations").run_to(1, &pool).await?;
    pool.close().await;

    let store = Store::open(&path).await?;
    let tables: Vec<String> = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table'")
        .fetch_all(store.pool())
        .await?
        .iter()
        .map(|r| r.get("name"))
        .collect();
    assert!(tables.contains(&"delivery_log".to_owned()));

    let backups: Vec<_> = std::fs::read_dir(dir.path())?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("vigil.db.backup-v1-"))
        .collect();
    assert_eq!(backups.len(), 1, "{backups:?}");

    // Opening again has nothing left to migrate, so no second backup appears.
    drop(store);
    Store::open(&path).await?;
    let count = std::fs::read_dir(dir.path())?
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".backup-"))
        .count();
    assert_eq!(count, 1);
    Ok(())
}
