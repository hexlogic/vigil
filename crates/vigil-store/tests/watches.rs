//! Watch persistence tests.

use vigil_core::{Processor, Schedule};
use vigil_store::{NewWatch, Store, StoreError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn open_temp() -> Result<(tempfile::TempDir, Store), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    Ok((dir, store))
}

async fn insert_raw(store: &Store, id: &str) -> Result<(), sqlx::Error> {
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
async fn create_then_get_round_trips() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let mut new = NewWatch::new("https://example.com/page", Schedule::Interval(300));
    new.name = "Example".into();
    new.jitter_secs = 7;
    new.snapshot_retention = 12;
    let created = store.create_watch(new).await?;

    let fetched = store.get_watch(created.id).await?;
    assert_eq!(fetched, created);
    assert_eq!(fetched.name, "Example");
    assert_eq!(fetched.url, "https://example.com/page");
    assert_eq!(fetched.schedule, Schedule::Interval(300));
    assert_eq!(fetched.processor, Processor::Text);
    assert_eq!((fetched.jitter_secs, fetched.snapshot_retention), (7, 12));
    assert!(!fetched.paused);
    assert_eq!(fetched.state.consecutive_errors, 0);
    assert_eq!(fetched.state.last_hash, None);
    Ok(())
}

#[tokio::test]
async fn list_returns_watches_in_creation_order() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let a = store
        .create_watch(NewWatch::new("https://a.example", Schedule::Interval(60)))
        .await?;
    let b = store
        .create_watch(NewWatch::new("https://b.example", Schedule::Interval(60)))
        .await?;
    let ids: Vec<_> = store
        .list_watches()
        .await?
        .into_iter()
        .map(|w| w.id)
        .collect();
    assert_eq!(ids, [a.id, b.id]);
    Ok(())
}

#[tokio::test]
async fn delete_removes_the_watch_and_reports_missing() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = store
        .create_watch(NewWatch::new("https://a.example", Schedule::Interval(60)))
        .await?;
    store.delete_watch(watch.id).await?;
    assert!(store.list_watches().await?.is_empty());
    assert!(matches!(
        store.delete_watch(watch.id).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.get_watch(watch.id).await,
        Err(StoreError::NotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn resolves_a_full_id() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let watch = store
        .create_watch(NewWatch::new("https://a.example", Schedule::Interval(60)))
        .await?;
    assert_eq!(
        store.resolve_watch_id(&watch.id.to_string()).await?,
        watch.id
    );
    Ok(())
}

#[tokio::test]
async fn watches_created_back_to_back_resolve_by_eight_char_suffix() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let mut watches = Vec::new();
    for n in 0..3 {
        let url = format!("https://{n}.example");
        watches.push(
            store
                .create_watch(NewWatch::new(url, Schedule::Interval(60)))
                .await?,
        );
    }
    for watch in &watches {
        let text = watch.id.to_string();
        let suffix = &text[text.len() - 8..];
        assert_eq!(
            store.resolve_watch_id(suffix).await?,
            watch.id,
            "suffix {suffix}"
        );
        assert_eq!(
            store.resolve_watch_id(&suffix.to_uppercase()).await?,
            watch.id,
            "uppercase suffix {suffix}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn short_or_malformed_input_is_rejected() -> TestResult {
    let (_dir, store) = open_temp().await?;
    store
        .create_watch(NewWatch::new("https://a.example", Schedule::Interval(60)))
        .await?;
    for input in ["", "abc", "12345", "not-a-watch-id", "%%%%%%%%", "abcdef%"] {
        assert!(
            matches!(
                store.resolve_watch_id(input).await,
                Err(StoreError::InvalidId(_))
            ),
            "{input:?} should be invalid"
        );
    }
    Ok(())
}

#[tokio::test]
async fn unknown_suffix_is_not_found() -> TestResult {
    let (_dir, store) = open_temp().await?;
    assert!(matches!(
        store.resolve_watch_id("abcdef12").await,
        Err(StoreError::NotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn suffix_shared_by_two_watches_is_ambiguous() -> TestResult {
    let (_dir, store) = open_temp().await?;
    insert_raw(&store, "00000000-0000-7000-8000-000000abcdef").await?;
    insert_raw(&store, "00000000-0000-7000-8000-100000abcdef").await?;
    assert!(matches!(
        store.resolve_watch_id("abcdef").await,
        Err(StoreError::AmbiguousId(_))
    ));
    // A longer suffix tells them apart.
    let id = store.resolve_watch_id("100000abcdef").await?;
    assert_eq!(id.to_string(), "00000000-0000-7000-8000-100000abcdef");
    Ok(())
}

#[tokio::test]
async fn an_unreadable_watch_is_skipped_when_listing() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let good = store
        .create_watch(NewWatch::new("https://a.example", Schedule::Interval(60)))
        .await?;
    sqlx::query(
        "INSERT INTO watches (id, name, url, schedule, created_at, updated_at) \
         VALUES ('00000000-0000-7000-8000-00000000bad0', 'bad', 'https://b.example', \
                 'not json', 0, 0)",
    )
    .execute(store.pool())
    .await?;

    let ids: Vec<_> = store
        .list_watches()
        .await?
        .into_iter()
        .map(|w| w.id)
        .collect();
    assert_eq!(ids, [good.id]);

    // Asking for the broken watch directly still explains what is wrong.
    let bad = "00000000-0000-7000-8000-00000000bad0".parse()?;
    assert!(matches!(
        store.get_watch(bad).await,
        Err(StoreError::Json(_))
    ));
    Ok(())
}
