//! Applying an import: one transaction, names and addresses matched again at apply time.

use vigil_core::Schedule;
use vigil_store::{
    ImportBatch, ImportTag, ImportTarget, ImportWatch, NewTag, NewTarget, NewWatch, Store,
    free_name, match_name,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn open_temp() -> Result<(tempfile::TempDir, Store), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    Ok((dir, store))
}

fn watch(url: &str) -> ImportWatch {
    ImportWatch {
        settings: NewWatch::new(url, Schedule::Interval(300)),
        paused: false,
        tags: Vec::new(),
        target_urls: Vec::new(),
        template: None,
    }
}

#[test]
fn names_match_exactly_first_and_by_case_only_when_that_is_unambiguous() {
    assert_eq!(match_name(["News", "news"], "news"), Some("news"));
    assert_eq!(match_name(["News"], "news"), Some("News"));
    assert_eq!(match_name(["News", "NEWS"], "news"), None);
    assert_eq!(match_name(["Shops"], "news"), None);
    assert_eq!(match_name([], "news"), None);
}

#[test]
fn a_taken_name_gets_a_number() {
    let taken: std::collections::HashSet<String> = ["phone", "phone 2"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert_eq!(free_name(&taken, "mail"), "mail");
    assert_eq!(free_name(&taken, "phone"), "phone 3");
}

#[tokio::test]
async fn watches_tags_and_targets_are_made_together_and_linked() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let mut shop = watch("https://shop.example/");
    shop.paused = true;
    shop.tags = vec!["shops".into()];
    shop.target_urls = vec!["ntfy://h/t".into()];
    shop.template = Some("{{ watch_name }}".into());
    let batch = ImportBatch {
        tags: vec![ImportTag {
            name: "shops".into(),
            default_interval_secs: Some(900),
            target_urls: vec!["ntfy://h/t".into()],
        }],
        targets: vec![ImportTarget {
            url: "ntfy://h/t".into(),
            name: "phone".into(),
        }],
        watches: vec![shop, watch("https://news.example/")],
        allow_duplicates: false,
    };
    let outcome = store.apply_import(&batch).await?;
    assert_eq!((outcome.tags_created, outcome.targets_created), (1, 1));
    let ids: Vec<_> = outcome.watches.iter().flatten().copied().collect();
    assert_eq!(ids.len(), 2);

    let made = store.get_watch(ids[0]).await?;
    assert!(made.paused);
    assert_eq!(
        made.notification_template.as_deref(),
        Some("{{ watch_name }}")
    );
    assert_eq!(made.schedule, Schedule::Interval(300));
    let tags = store.tags_for_watch(ids[0]).await?;
    assert_eq!(
        tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["shops"]
    );
    assert_eq!(tags[0].default_interval_secs, Some(900));
    assert_eq!(store.tag_targets(tags[0].id).await?.len(), 1);
    let linked = store.targets_for_watch(ids[0], false).await?;
    assert_eq!(
        (linked[0].name.as_str(), linked[0].url.as_str()),
        ("phone", "ntfy://h/t")
    );
    assert!(!store.get_watch(ids[1]).await?.paused);
    Ok(())
}

#[tokio::test]
async fn existing_tags_and_targets_are_reused_and_left_alone() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let tag = store
        .create_tag(&NewTag {
            name: "News".into(),
            color: "red".into(),
            default_interval_secs: Some(60),
        })
        .await?;
    let target = store
        .create_target(&NewTarget {
            name: "mine".into(),
            url: "ntfy://h/t".into(),
            default_template: Some("x".into()),
        })
        .await?;
    let mut w = watch("https://a.example/");
    w.tags = vec!["news".into()];
    w.target_urls = vec!["ntfy://h/t".into()];
    let batch = ImportBatch {
        tags: vec![ImportTag {
            name: "news".into(),
            default_interval_secs: Some(999),
            target_urls: vec![],
        }],
        targets: vec![ImportTarget {
            url: "ntfy://h/t".into(),
            name: "imported".into(),
        }],
        watches: vec![w],
        allow_duplicates: false,
    };
    let outcome = store.apply_import(&batch).await?;
    assert_eq!((outcome.tags_created, outcome.targets_created), (0, 0));
    let id = outcome.watches[0].ok_or("skipped")?;
    assert_eq!(store.tags_for_watch(id).await?[0].id, tag.id);
    assert_eq!(store.targets_for_watch(id, false).await?[0].id, target.id);
    // What was there is not changed by the import.
    let kept = store.get_tag(tag.id).await?;
    assert_eq!(
        (kept.color.as_str(), kept.default_interval_secs),
        ("red", Some(60))
    );
    assert_eq!(
        store
            .get_target(target.id)
            .await?
            .default_template
            .as_deref(),
        Some("x")
    );
    assert_eq!(store.list_tags().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_new_target_with_a_taken_name_gets_a_number() -> TestResult {
    let (_dir, store) = open_temp().await?;
    store
        .create_target(&NewTarget {
            name: "ntfy h".into(),
            url: "ntfy://h/old".into(),
            default_template: None,
        })
        .await?;
    let mut w = watch("https://a.example/");
    w.target_urls = vec!["ntfy://h/new".into()];
    let batch = ImportBatch {
        targets: vec![ImportTarget {
            url: "ntfy://h/new".into(),
            name: "ntfy h".into(),
        }],
        watches: vec![w],
        ..ImportBatch::default()
    };
    store.apply_import(&batch).await?;
    let mut names: Vec<String> = store
        .list_targets()
        .await?
        .into_iter()
        .map(|t| t.name)
        .collect();
    names.sort();
    assert_eq!(names, ["ntfy h", "ntfy h 2"]);
    Ok(())
}

#[tokio::test]
async fn a_watched_address_is_skipped_unless_duplicates_are_allowed() -> TestResult {
    let (_dir, store) = open_temp().await?;
    store
        .create_watch(NewWatch::new("https://a.example/", Schedule::Interval(60)))
        .await?;
    // Two of the batch's own watches with one address are both made: they may differ.
    let batch = ImportBatch {
        watches: vec![
            watch("https://a.example/"),
            watch("https://b.example/"),
            watch("https://b.example/"),
        ],
        ..ImportBatch::default()
    };
    let outcome = store.apply_import(&batch).await?;
    assert_eq!(
        outcome
            .watches
            .iter()
            .map(Option::is_some)
            .collect::<Vec<_>>(),
        [false, true, true]
    );
    assert_eq!(store.list_watches().await?.len(), 3);

    let again = ImportBatch {
        allow_duplicates: true,
        ..batch
    };
    let outcome = store.apply_import(&again).await?;
    assert!(outcome.watches.iter().all(Option::is_some));
    assert_eq!(store.list_watches().await?.len(), 6);
    Ok(())
}

#[tokio::test]
async fn a_batch_that_cannot_be_applied_changes_nothing() -> TestResult {
    let (_dir, store) = open_temp().await?;
    let mut bad = watch("https://b.example/");
    bad.tags = vec!["not listed".into()];
    let batch = ImportBatch {
        tags: vec![ImportTag {
            name: "new".into(),
            default_interval_secs: None,
            target_urls: vec![],
        }],
        targets: vec![ImportTarget {
            url: "ntfy://h/t".into(),
            name: "phone".into(),
        }],
        watches: vec![watch("https://a.example/"), bad],
        allow_duplicates: false,
    };
    assert!(store.apply_import(&batch).await.is_err());
    assert!(store.list_watches().await?.is_empty());
    assert!(store.list_tags().await?.is_empty());
    assert!(store.list_targets().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn an_existing_tag_keeps_its_targets_and_the_batchs_are_not_made() -> TestResult {
    let (_dir, store) = open_temp().await?;
    store
        .create_tag(&NewTag {
            name: "News".into(),
            color: "red".into(),
            default_interval_secs: None,
        })
        .await?;
    let mut news = watch("https://news.example/");
    news.tags = vec!["news".into()];
    let batch = ImportBatch {
        tags: vec![ImportTag {
            name: "news".into(),
            default_interval_secs: None,
            target_urls: vec!["tgram://bot/chat".into()],
        }],
        targets: vec![ImportTarget {
            url: "tgram://bot/chat".into(),
            name: "telegram".into(),
        }],
        watches: vec![news],
        allow_duplicates: false,
    };
    let outcome = store.apply_import(&batch).await?;
    assert_eq!((outcome.tags_created, outcome.targets_created), (0, 0));
    assert!(store.list_targets().await?.is_empty());
    assert_eq!(store.list_tags().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_tag_and_a_target_that_only_a_skipped_watch_uses_are_not_made() -> TestResult {
    let (_dir, store) = open_temp().await?;
    // The watch became a duplicate after the plan was made.
    store
        .create_watch(NewWatch::new("https://a.example/", Schedule::Interval(60)))
        .await?;
    let mut skipped = watch("https://a.example/");
    skipped.tags = vec!["shops".into()];
    skipped.target_urls = vec!["ntfy://h/t".into()];
    let mut made = watch("https://b.example/");
    made.tags = vec!["news".into()];
    made.target_urls = vec!["ntfy://h/other".into()];
    let tag = |name: &str, target: &str| ImportTag {
        name: name.into(),
        default_interval_secs: None,
        target_urls: vec![target.into()],
    };
    let target = |url: &str, name: &str| ImportTarget {
        url: url.into(),
        name: name.into(),
    };
    let batch = ImportBatch {
        tags: vec![tag("shops", "ntfy://h/t"), tag("news", "ntfy://h/other")],
        targets: vec![target("ntfy://h/t", "one"), target("ntfy://h/other", "two")],
        watches: vec![skipped, made],
        allow_duplicates: false,
    };
    let outcome = store.apply_import(&batch).await?;
    assert_eq!((outcome.tags_created, outcome.targets_created), (1, 1));
    let tags: Vec<_> = store
        .list_tags()
        .await?
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(tags, ["news"]);
    let urls: Vec<_> = store
        .list_targets()
        .await?
        .into_iter()
        .map(|t| t.url)
        .collect();
    assert_eq!(urls, ["ntfy://h/other"]);
    Ok(())
}
