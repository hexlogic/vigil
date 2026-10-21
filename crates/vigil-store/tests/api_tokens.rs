//! API tokens: kept by hash, named, revocable, and noted when used.

use vigil_store::{Store, StoreError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn open_temp() -> Result<(tempfile::TempDir, Store), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    Ok((dir, store))
}

#[tokio::test]
async fn a_token_is_found_by_its_hash_and_never_stored_whole() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let hash = [7u8; 32];
    let made = store.create_api_token("ci", &hash, "vigil_ab12").await?;
    assert_eq!(
        (made.name.as_str(), made.prefix.as_str()),
        ("ci", "vigil_ab12")
    );
    assert_eq!(made.last_used_at, None);

    let found = store
        .use_api_token(&hash, 1_000_000)
        .await?
        .ok_or("token not found")?;
    assert_eq!(found.id, made.id);
    assert_eq!(found.last_used_at, Some(1_000_000));
    assert!(store.use_api_token(&[8u8; 32], 1_000_000).await?.is_none());

    // The row holds the hash and a prefix, nothing that could be sent as a token.
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('api_tokens')")
            .fetch_all(store.pool())
            .await?;
    assert_eq!(
        columns,
        [
            "id",
            "name",
            "token_hash",
            "prefix",
            "created_at",
            "last_used_at"
        ]
    );
    Ok(())
}

#[tokio::test]
async fn use_is_noted_at_most_once_a_minute() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let hash = [1u8; 32];
    store.create_api_token("ci", &hash, "vigil_00").await?;
    let first = store
        .use_api_token(&hash, 10_000_000)
        .await?
        .ok_or("missing")?;
    assert_eq!(first.last_used_at, Some(10_000_000));
    let soon = store
        .use_api_token(&hash, 10_030_000)
        .await?
        .ok_or("missing")?;
    assert_eq!(soon.last_used_at, Some(10_000_000));
    let later = store
        .use_api_token(&hash, 10_060_000)
        .await?
        .ok_or("missing")?;
    assert_eq!(later.last_used_at, Some(10_060_000));
    Ok(())
}

#[tokio::test]
async fn names_are_unique_and_tokens_can_be_resolved_listed_and_revoked() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let a = store
        .create_api_token("alpha", &[1u8; 32], "vigil_aa")
        .await?;
    let b = store
        .create_api_token("beta", &[2u8; 32], "vigil_bb")
        .await?;

    let error = store
        .create_api_token("alpha", &[3u8; 32], "vigil_cc")
        .await
        .err()
        .ok_or("a taken name was accepted")?;
    assert!(matches!(error, StoreError::Conflict(_)), "{error}");

    let names: Vec<String> = store
        .list_api_tokens()
        .await?
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["alpha", "beta"]);

    assert_eq!(store.resolve_api_token("beta").await?.id, b.id);
    assert_eq!(store.resolve_api_token(&a.id.to_string()).await?.id, a.id);
    let id = a.id.to_string();
    assert_eq!(store.resolve_api_token(&id[id.len() - 8..]).await?.id, a.id);
    assert!(matches!(
        store.resolve_api_token("abc").await,
        Err(StoreError::NotFound(_))
    ));

    store.delete_api_token(a.id).await?;
    assert!(store.use_api_token(&[1u8; 32], 1).await?.is_none());
    assert!(matches!(
        store.delete_api_token(a.id).await,
        Err(StoreError::NotFound(_))
    ));
    assert_eq!(store.list_api_tokens().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn using_a_token_that_was_noted_a_moment_ago_needs_no_write() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let hash = [3u8; 32];
    store.create_api_token("ci", &hash, "vigil_03").await?;
    store
        .use_api_token(&hash, 20_000_000)
        .await?
        .ok_or("missing")?;

    // Another connection holds the one write lock, as the scheduler does while it records a
    // check. A request that only reads must not wait for it.
    let mut writer = store.pool().acquire().await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *writer).await?;
    let started = std::time::Instant::now();
    let found = store
        .use_api_token(&hash, 20_010_000)
        .await?
        .ok_or("missing")?;
    let waited = started.elapsed();
    sqlx::query("ROLLBACK").execute(&mut *writer).await?;

    assert_eq!(found.last_used_at, Some(20_000_000));
    assert!(waited < std::time::Duration::from_secs(2), "{waited:?}");
    // A token that is not there is still not there, and revoking one in between is no error.
    assert!(store.use_api_token(&[9u8; 32], 20_010_000).await?.is_none());
    Ok(())
}
