//! Notifications saved until they have been delivered.

use sqlx::Row;
use vigil_core::{Schedule, WatchId};
use vigil_store::{NewPending, NewTarget, NewWatch, Store, StoreError, TargetId};

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

#[tokio::test]
async fn saved_notifications_come_back_in_order_until_completed() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = watch(&store).await?;
    let target = store.create_target(&target("t")).await?;
    let snapshot = uuid_v7();

    let payload = "{\"diff\":\"é ".to_owned() + &"x".repeat(100_000) + "\"}";
    let first = store
        .enqueue_notification(&NewPending {
            watch,
            target: target.id,
            snapshot,
            payload: payload.clone(),
        })
        .await?;
    let second = store
        .enqueue_notification(&NewPending {
            watch,
            target: target.id,
            snapshot,
            payload: "{}".into(),
        })
        .await?;
    assert!(second > first);

    let pending = store.pending_notifications().await?;
    assert_eq!(
        pending.iter().map(|p| p.id).collect::<Vec<_>>(),
        [first, second]
    );
    assert_eq!(pending[0].payload, payload);
    assert_eq!(
        (pending[0].watch, pending[0].target, pending[0].snapshot),
        (watch, target.id, snapshot)
    );
    assert!(pending[0].created_at > 0);

    store.complete_notification(first).await?;
    store.complete_notification(first).await?;
    let ids: Vec<i64> = store
        .pending_notifications()
        .await?
        .iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids, [second]);
    Ok(())
}

#[tokio::test]
async fn saved_notifications_go_with_their_watch_or_target() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let (w1, w2) = (watch(&store).await?, watch(&store).await?);
    let (t1, t2) = (
        store.create_target(&target("a")).await?,
        store.create_target(&target("b")).await?,
    );
    for (w, t) in [(w1, t1.id), (w2, t2.id)] {
        store
            .enqueue_notification(&NewPending {
                watch: w,
                target: t,
                snapshot: uuid_v7(),
                payload: "{}".into(),
            })
            .await?;
    }
    store.delete_watch(w1).await?;
    assert_eq!(store.pending_notifications().await?.len(), 1);
    store.delete_target(t2.id).await?;
    assert!(store.pending_notifications().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn an_unreadable_saved_notification_is_dropped() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = watch(&store).await?;
    let target = store.create_target(&target("t")).await?;
    sqlx::query(
        "INSERT INTO notification_outbox (watch_id, target_id, snapshot_id, created_at, payload) \
         VALUES (?, ?, 'not-a-snapshot', 0, x'00')",
    )
    .bind(watch.to_string())
    .bind(target.id.to_string())
    .execute(store.pool())
    .await?;
    assert!(store.pending_notifications().await?.is_empty());
    let left: i64 = sqlx::query("SELECT COUNT(*) AS n FROM notification_outbox")
        .fetch_one(store.pool())
        .await?
        .get("n");
    assert_eq!(left, 0);
    Ok(())
}

fn uuid_v7() -> vigil_store::SnapshotId {
    TargetId::now_v7()
}
