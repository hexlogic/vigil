//! Watch persistence tests.

use vigil_core::{Conditions, NotifyOn, Processor, Schedule, SelectMode, Step};
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
    new.processor = Processor::Text;
    new.pipeline = vec![
        Step::Css {
            selector: "main".into(),
            mode: SelectMode::Include,
            required: true,
        },
        Step::Lowercase {},
    ];
    new.conditions = Conditions {
        trigger_text: vec!["sale".into()],
        block_text: vec!["/sold out/i".into()],
        notify_on: NotifyOn {
            added: true,
            removed: false,
            changed: true,
        },
    };
    let created = store.create_watch(new.clone()).await?;

    let fetched = store.get_watch(created.id).await?;
    assert_eq!(fetched, created);
    assert_eq!(fetched.name, "Example");
    assert_eq!(fetched.url, "https://example.com/page");
    assert_eq!(fetched.schedule, Schedule::Interval(300));
    assert_eq!(fetched.processor, Processor::Text);
    assert_eq!((fetched.jitter_secs, fetched.snapshot_retention), (7, 12));
    assert_eq!(fetched.pipeline, new.pipeline);
    assert_eq!(fetched.conditions, new.conditions);
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

#[tokio::test]
async fn a_watch_without_stored_steps_has_an_empty_pipeline_and_default_conditions() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let id = "00000000-0000-7000-8000-000000000001";
    insert_raw(&store, id).await?;
    let watch = store.get_watch(id.parse()?).await?;
    assert!(watch.pipeline.is_empty());
    assert_eq!(watch.conditions, Conditions::default());
    Ok(())
}

#[tokio::test]
async fn the_stored_json_has_the_documented_shape() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let mut new = NewWatch::new("https://example.com", Schedule::Interval(60));
    new.pipeline = vec![Step::RegexReplace {
        pattern: "a".into(),
        replacement: "b".into(),
    }];
    new.conditions.trigger_text = vec!["x".into()];
    let watch = store.create_watch(new).await?;
    let (pipeline, conditions): (String, String) =
        sqlx::query_as("SELECT pipeline, conditions FROM watches WHERE id = ?")
            .bind(watch.id.to_string())
            .fetch_one(store.pool())
            .await?;
    assert_eq!(
        pipeline,
        r#"[{"type":"regex_replace","pattern":"a","replacement":"b"}]"#
    );
    assert_eq!(
        conditions,
        r#"{"trigger_text":["x"],"block_text":[],"notify_on":{"added":true,"removed":true,"changed":true}}"#
    );
    Ok(())
}

#[tokio::test]
async fn update_replaces_the_settings_and_clears_the_validators() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let created = store
        .create_watch(NewWatch::new("https://example.com", Schedule::Interval(60)))
        .await?;
    // A watch that has been checked and paused, with validators from the last response.
    sqlx::query(
        "UPDATE watches SET paused = 1, etag = '\"abc\"', last_modified = 'Mon', \
         last_hash = zeroblob(32), last_checked_at = 5, consecutive_errors = 2 WHERE id = ?",
    )
    .bind(created.id.to_string())
    .execute(store.pool())
    .await?;
    let before = store.get_watch(created.id).await?;
    assert!(before.state.etag.is_some());

    let mut settings = NewWatch::from(&before);
    settings.name = "Renamed".into();
    settings.url = "https://example.com/new".into();
    settings.schedule = Schedule::Interval(900);
    settings.processor = Processor::Json;
    settings.pipeline = vec![Step::JsonPath {
        expr: "$.a".into(),
        required: false,
    }];
    settings.conditions.block_text = vec!["x".into()];
    let updated = store.update_watch(created.id, &settings).await?;

    assert_eq!(updated.name, "Renamed");
    assert_eq!(updated.url, "https://example.com/new");
    assert_eq!(updated.schedule, Schedule::Interval(900));
    assert_eq!(updated.processor, Processor::Json);
    assert_eq!(updated.pipeline, settings.pipeline);
    assert_eq!(updated.conditions, settings.conditions);
    assert!(updated.updated_at >= before.updated_at);
    assert_eq!(updated.created_at, before.created_at);
    // Cleared, so the next request is unconditional and the new pipeline gets a body.
    assert_eq!(updated.state.etag, None);
    assert_eq!(updated.state.last_modified, None);
    // Kept.
    assert!(updated.paused);
    assert_eq!(updated.state.last_hash, before.state.last_hash);
    assert_eq!(updated.state.last_checked_at, Some(5));
    assert_eq!(updated.state.consecutive_errors, 2);
    assert_eq!(store.get_watch(created.id).await?, updated);
    Ok(())
}

#[tokio::test]
async fn updating_a_missing_watch_is_not_found() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let settings = NewWatch::new("https://example.com", Schedule::Interval(60));
    let id = "00000000-0000-7000-8000-000000000009".parse()?;
    assert!(matches!(
        store.update_watch(id, &settings).await,
        Err(StoreError::NotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_row_with_an_unknown_step_is_skipped_when_listing() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let good = store
        .create_watch(NewWatch::new(
            "https://good.example",
            Schedule::Interval(60),
        ))
        .await?;
    let bad = store
        .create_watch(NewWatch::new("https://bad.example", Schedule::Interval(60)))
        .await?;
    // Written by a newer version that knows a step this one does not.
    sqlx::query("UPDATE watches SET pipeline = '[{\"type\":\"from_the_future\"}]' WHERE id = ?")
        .bind(bad.id.to_string())
        .execute(store.pool())
        .await?;
    let listed: Vec<_> = store
        .list_watches()
        .await?
        .into_iter()
        .map(|w| w.id)
        .collect();
    assert_eq!(listed, [good.id]);
    assert!(matches!(
        store.get_watch(bad.id).await,
        Err(StoreError::Json(_))
    ));
    Ok(())
}
