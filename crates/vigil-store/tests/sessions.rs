//! Instance settings and the sessions of the web UI.

use vigil_store::Store;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn open_temp() -> Result<(tempfile::TempDir, Store), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    Ok((dir, store))
}

#[tokio::test]
async fn a_setting_can_be_set_replaced_and_removed() -> TestResult {
    let (_dir, store) = open_temp().await?;
    assert_eq!(store.get_setting("admin_password").await?, None);

    store.set_setting("admin_password", "one").await?;
    assert_eq!(
        store.get_setting("admin_password").await?.as_deref(),
        Some("one")
    );

    store.set_setting("admin_password", "two").await?;
    assert_eq!(
        store.get_setting("admin_password").await?.as_deref(),
        Some("two")
    );

    store.delete_setting("admin_password").await?;
    assert_eq!(store.get_setting("admin_password").await?, None);
    // Removing it again is fine.
    store.delete_setting("admin_password").await?;
    Ok(())
}

#[tokio::test]
async fn a_session_is_found_until_it_expires() -> TestResult {
    let (_dir, store) = open_temp().await?;
    store.create_session(b"hash-a", 1_000, 5_000).await?;

    let session = store.session(b"hash-a", 2_000).await?;
    let session = session.ok_or("session not found")?;
    assert_eq!(
        (session.created_at, session.last_seen_at, session.expires_at),
        (1_000, 1_000, 5_000)
    );
    assert!(store.session(b"hash-a", 4_999).await?.is_some());
    assert!(store.session(b"hash-a", 5_000).await?.is_none());
    assert!(store.session(b"other", 2_000).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn touching_a_session_extends_it_but_not_after_it_ended() -> TestResult {
    let (_dir, store) = open_temp().await?;
    store.create_session(b"hash-a", 1_000, 5_000).await?;

    store.touch_session(b"hash-a", 4_000, 9_000).await?;
    let session = store.session(b"hash-a", 4_000).await?.ok_or("gone")?;
    assert_eq!((session.last_seen_at, session.expires_at), (4_000, 9_000));
    assert_eq!(session.created_at, 1_000);

    // A session that has ended stays ended.
    store.create_session(b"hash-b", 1_000, 2_000).await?;
    store.touch_session(b"hash-b", 3_000, 9_000).await?;
    assert!(store.session(b"hash-b", 3_000).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn sessions_can_be_ended_one_by_one_or_all_but_one() -> TestResult {
    let (_dir, store) = open_temp().await?;
    for hash in [b"a", b"b", b"c"] {
        store.create_session(hash, 0, 10_000).await?;
    }

    store.delete_session(b"a").await?;
    store.delete_session(b"a").await?;
    assert!(store.session(b"a", 1).await?.is_none());

    assert_eq!(store.delete_sessions_except(Some(b"b")).await?, 1);
    assert!(store.session(b"b", 1).await?.is_some());
    assert!(store.session(b"c", 1).await?.is_none());

    assert_eq!(store.delete_sessions_except(None).await?, 1);
    assert!(store.session(b"b", 1).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn expired_sessions_are_purged() -> TestResult {
    let (_dir, store) = open_temp().await?;
    store.create_session(b"old", 0, 1_000).await?;
    store.create_session(b"new", 0, 9_000).await?;

    assert_eq!(store.purge_expired_sessions(5_000).await?, 1);
    assert_eq!(store.purge_expired_sessions(5_000).await?, 0);
    assert!(store.session(b"new", 5_000).await?.is_some());
    Ok(())
}

#[tokio::test]
async fn the_new_tables_exist_after_open() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let tables: Vec<String> = sqlx_names(&store).await?;
    for table in ["settings", "sessions"] {
        assert!(tables.iter().any(|t| t == table), "missing table {table}");
    }
    Ok(())
}

async fn sqlx_names(store: &Store) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'")
        .fetch_all(store.pool())
        .await
}
