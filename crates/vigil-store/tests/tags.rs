//! Tags, their links to watches and targets, and the targets a change is sent to.

use uuid::Uuid;
use vigil_core::{Schedule, WatchId};
use vigil_store::{NewTag, NewTarget, NewWatch, Store, StoreError, TargetId};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn open_temp() -> Result<(tempfile::TempDir, Store), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    Ok((dir, store))
}

fn tag(name: &str) -> NewTag {
    NewTag {
        name: name.to_owned(),
        color: "blue".to_owned(),
        default_interval_secs: None,
    }
}

async fn watch(store: &Store) -> Result<WatchId, StoreError> {
    let new = NewWatch::new("https://example.com", Schedule::Interval(60));
    Ok(store.create_watch(new).await?.id)
}

async fn target(store: &Store, name: &str) -> Result<TargetId, StoreError> {
    let new = NewTarget {
        name: name.to_owned(),
        url: format!("ntfy://ntfy.example/{name}"),
        default_template: None,
    };
    Ok(store.create_target(&new).await?.id)
}

#[tokio::test]
async fn a_tag_can_be_created_listed_edited_and_deleted() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let work = store
        .create_tag(&NewTag {
            name: "work".into(),
            color: "green".into(),
            default_interval_secs: Some(900),
        })
        .await?;
    store.create_tag(&tag("alpha")).await?;

    let names: Vec<_> = store
        .list_tags()
        .await?
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["alpha", "work"]);
    assert_eq!(work.default_interval_secs, Some(900));
    assert_eq!(store.get_tag(work.id).await?, work);

    let edited = store
        .update_tag(
            work.id,
            &NewTag {
                name: "office".into(),
                color: "red".into(),
                default_interval_secs: None,
            },
        )
        .await?;
    assert_eq!(
        (
            edited.name.as_str(),
            edited.color.as_str(),
            edited.default_interval_secs
        ),
        ("office", "red", None)
    );
    assert_eq!(edited.created_at, work.created_at);

    store.delete_tag(work.id).await?;
    assert!(matches!(
        store.get_tag(work.id).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.delete_tag(work.id).await,
        Err(StoreError::NotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_tag_name_must_be_unique() -> TestResult {
    let (_dir, store) = open_temp().await?;
    store.create_tag(&tag("news")).await?;
    let other = store.create_tag(&tag("shops")).await?;

    assert!(matches!(
        store.create_tag(&tag("news")).await,
        Err(StoreError::Conflict(_))
    ));
    assert!(matches!(
        store.update_tag(other.id, &tag("news")).await,
        Err(StoreError::Conflict(_))
    ));
    // Keeping its own name is not a clash.
    store.update_tag(other.id, &tag("shops")).await?;
    assert!(matches!(
        store.update_tag(Uuid::now_v7(), &tag("x")).await,
        Err(StoreError::NotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn watch_tags_are_replaced_as_a_set() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let (a, b) = (watch(&store).await?, watch(&store).await?);
    let (news, shops) = (
        store.create_tag(&tag("news")).await?,
        store.create_tag(&tag("shops")).await?,
    );

    store
        .set_watch_tags(a, &[news.id, shops.id, news.id])
        .await?;
    store.set_watch_tags(b, &[shops.id]).await?;

    let names: Vec<_> = store
        .tags_for_watch(a)
        .await?
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["news", "shops"]);

    let map = store.watch_tags_map().await?;
    assert_eq!(map[&a].len(), 2);
    assert_eq!(map[&b], [shops.id]);
    let counts = store.tag_watch_counts().await?;
    assert_eq!((counts[&news.id], counts[&shops.id]), (1, 2));

    store.set_watch_tags(a, &[]).await?;
    assert!(store.tags_for_watch(a).await?.is_empty());
    assert!(!store.watch_tags_map().await?.contains_key(&a));
    assert!(!store.tag_watch_counts().await?.contains_key(&news.id));
    Ok(())
}

#[tokio::test]
async fn an_unknown_tag_or_watch_is_not_found_and_changes_nothing() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let w = watch(&store).await?;
    let news = store.create_tag(&tag("news")).await?;
    store.set_watch_tags(w, &[news.id]).await?;

    let result = store.set_watch_tags(w, &[Uuid::now_v7()]).await;
    assert!(matches!(result, Err(StoreError::NotFound(_))));
    // The failed replacement was rolled back.
    assert_eq!(store.tags_for_watch(w).await?.len(), 1);

    let missing = WatchId::from_uuid(Uuid::now_v7());
    assert!(matches!(
        store.set_watch_tags(missing, &[news.id]).await,
        Err(StoreError::NotFound(_))
    ));
    Ok(())
}

#[tokio::test]
async fn deleting_a_tag_or_a_watch_removes_the_links_only() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let w = watch(&store).await?;
    let news = store.create_tag(&tag("news")).await?;
    let t = target(&store, "phone").await?;
    store.set_watch_tags(w, &[news.id]).await?;
    store.set_tag_targets(news.id, &[t]).await?;

    store.delete_tag(news.id).await?;
    assert!(store.tags_for_watch(w).await?.is_empty());
    assert!(store.get_watch(w).await.is_ok());
    assert!(store.get_target(t).await.is_ok());

    let again = store.create_tag(&tag("again")).await?;
    store.set_watch_tags(w, &[again.id]).await?;
    store.delete_watch(w).await?;
    assert_eq!(store.tag_watch_counts().await?.len(), 0);
    assert!(store.get_tag(again.id).await.is_ok());
    Ok(())
}

#[tokio::test]
async fn tag_targets_are_replaced_as_a_set() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let news = store.create_tag(&tag("news")).await?;
    let (one, two) = (target(&store, "one").await?, target(&store, "two").await?);

    store.set_tag_targets(news.id, &[one, two, one]).await?;
    let mut linked = store.tag_targets(news.id).await?;
    linked.sort();
    let mut expected = vec![one, two];
    expected.sort();
    assert_eq!(linked, expected);

    store.set_tag_targets(news.id, &[two]).await?;
    assert_eq!(store.tag_targets(news.id).await?, [two]);

    let result = store.set_tag_targets(news.id, &[Uuid::now_v7()]).await;
    assert!(matches!(result, Err(StoreError::NotFound(_))));
    assert_eq!(store.tag_targets(news.id).await?, [two]);
    Ok(())
}

#[tokio::test]
async fn a_change_goes_to_the_watch_targets_and_the_tag_targets_once_each() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let w = watch(&store).await?;
    let (direct, shared, via_tag, off) = (
        target(&store, "direct").await?,
        target(&store, "shared").await?,
        target(&store, "via-tag").await?,
        target(&store, "off").await?,
    );
    let (news, shops) = (
        store.create_tag(&tag("news")).await?,
        store.create_tag(&tag("shops")).await?,
    );
    store.link_target(w, direct).await?;
    store.link_target(w, shared).await?;
    store.set_watch_tags(w, &[news.id, shops.id]).await?;
    // `shared` is reached by the watch and by both tags; `off` is disabled.
    store.set_tag_targets(news.id, &[shared, via_tag]).await?;
    store.set_tag_targets(shops.id, &[shared, off]).await?;
    store.set_target_enabled(off, false).await?;

    let names: Vec<_> = store
        .delivery_targets(w)
        .await?
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["direct", "shared", "via-tag"]);

    // A watch without those tags is not reached through them.
    let other = watch(&store).await?;
    assert!(store.delivery_targets(other).await?.is_empty());
    // `targets_for_watch` still means the direct links only.
    let direct_only: Vec<_> = store
        .targets_for_watch(w, true)
        .await?
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(direct_only, ["direct", "shared"]);
    Ok(())
}

#[tokio::test]
async fn a_watchs_links_are_replaced_together_or_not_at_all() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let id = watch(&store).await?;
    let (news, shops) = (
        store.create_tag(&tag("news")).await?,
        store.create_tag(&tag("shops")).await?,
    );
    let (phone, mail) = (
        target(&store, "phone").await?,
        target(&store, "mail").await?,
    );

    store
        .set_watch_links(id, &[news.id, news.id], &[phone], Some("{{ watch_name }}"))
        .await?;
    let names = |tags: Vec<vigil_store::Tag>| tags.into_iter().map(|t| t.name).collect::<Vec<_>>();
    assert_eq!(names(store.tags_for_watch(id).await?), ["news"]);
    let linked = store.targets_for_watch(id, false).await?;
    assert_eq!(
        linked.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["phone"]
    );
    assert_eq!(
        store.get_watch(id).await?.notification_template.as_deref(),
        Some("{{ watch_name }}")
    );

    store
        .set_watch_links(id, &[shops.id], &[mail, phone], None)
        .await?;
    assert_eq!(names(store.tags_for_watch(id).await?), ["shops"]);
    assert_eq!(store.targets_for_watch(id, false).await?.len(), 2);
    assert_eq!(store.get_watch(id).await?.notification_template, None);

    // An unknown target refuses the whole change, including the tags and the template.
    let ghost = Uuid::now_v7();
    let error = store
        .set_watch_links(id, &[news.id], &[ghost], Some("x"))
        .await
        .err()
        .ok_or("an unknown target was accepted")?;
    assert!(matches!(error, StoreError::NotFound(_)), "{error}");
    assert_eq!(names(store.tags_for_watch(id).await?), ["shops"]);
    assert_eq!(store.get_watch(id).await?.notification_template, None);

    // An unknown watch is not found either, and an edit's check state is left alone.
    let missing = WatchId::from_uuid(Uuid::now_v7());
    assert!(matches!(
        store.set_watch_links(missing, &[], &[], None).await,
        Err(StoreError::NotFound(_))
    ));
    let before = store.get_watch(id).await?.updated_at;
    store.set_watch_links(id, &[], &[], None).await?;
    assert_eq!(store.get_watch(id).await?.updated_at, before);
    Ok(())
}
