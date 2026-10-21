//! Reading backups: the two layouts, and archives that are not what they claim to be.

use std::io::Write;

use vigil_import::{Existing, Format, ImportError, Options, plan};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ID: &str = "11111111-2222-4333-8444-555555555555";

fn zip_of(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in files {
        writer.start_file(name.as_str(), zip::write::SimpleFileOptions::default())?;
        writer.write_all(bytes)?;
    }
    Ok(writer.finish()?.into_inner())
}

fn read(bytes: &[u8]) -> Result<vigil_import::ImportPlan, ImportError> {
    plan(bytes, None, &Options::default(), &Existing::default())
}

fn watch(url: &str) -> Vec<u8> {
    format!(r#"{{"uuid": "{ID}", "url": "{url}", "title": "Shop"}}"#).into_bytes()
}

#[test]
fn the_older_single_file_holds_every_watch_and_the_tags() -> TestResult {
    let old = include_bytes!("fixtures/backup/old-datastore.json");
    let plan = read(old)?;
    assert_eq!(plan.report.format, Format::Backup);
    assert_eq!(plan.report.totals.found, 2);
    assert!(
        plan.report
            .notes
            .iter()
            .any(|n| n.message.contains("older single-file datastore"))
    );

    let single = serde_json::json!({
        "watching": {ID: {"url": "https://a.example/", "title": "A", "tags": ["t1"]}},
        "settings": {"application": {"tags": {"t1": {"title": "Shops"}}}}
    });
    let plan = read(single.to_string().as_bytes())?;
    assert_eq!(plan.batch.watches.len(), 1);
    assert_eq!(plan.batch.watches[0].tags, ["Shops"]);
    assert_eq!(plan.batch.tags.len(), 1);
    Ok(())
}

#[test]
fn only_a_folder_named_like_an_id_holds_a_watch() -> TestResult {
    let archive = zip_of(&[
        (format!("{ID}/watch.json"), watch("https://a.example/")),
        // None of these is read, whatever they hold.
        (
            "../escape/watch.json".to_owned(),
            watch("https://evil.example/"),
        ),
        (
            "not-an-id/watch.json".to_owned(),
            watch("https://evil.example/"),
        ),
        (
            format!("{ID}/nested/watch.json"),
            watch("https://evil.example/"),
        ),
        (format!("{ID}/history.txt"), b"1,abc.txt".to_vec()),
    ])?;
    let plan = read(&archive)?;
    assert_eq!(plan.batch.watches.len(), 1);
    assert_eq!(plan.batch.watches[0].settings.url, "https://a.example/");
    Ok(())
}

#[test]
fn an_archive_with_nothing_to_import_is_an_error() -> TestResult {
    for archive in [
        zip_of(&[("readme.txt".to_owned(), b"hello".to_vec())])?,
        b"PK\x03\x04this is not really a zip".to_vec(),
    ] {
        assert!(
            matches!(
                read(&archive),
                Err(ImportError::Invalid {
                    format: Format::Backup,
                    ..
                })
            ),
            "{:?}",
            read(&archive).err()
        );
    }
    // An archive with no entries at all does not even start like one.
    assert!(matches!(
        read(&zip_of(&[])?),
        Err(ImportError::UnknownFormat)
    ));
    Ok(())
}

#[test]
fn a_file_that_is_too_large_is_refused_whatever_it_claims() -> TestResult {
    // 11 MiB of text that compresses to almost nothing: a small archive, a large file.
    let mut big = br#"{"url": "https://a.example/", "pad": ""#.to_vec();
    big.extend(std::iter::repeat_n(b'x', 11 * 1024 * 1024));
    big.extend_from_slice(br#""}"#);
    let archive = zip_of(&[(format!("{ID}/watch.json"), big)])?;
    assert!(
        archive.len() < 100_000,
        "the archive is {} bytes",
        archive.len()
    );
    let error = read(&archive)
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(error.contains("too large"), "{error}");
    Ok(())
}

#[test]
fn a_watch_file_that_is_not_an_object_is_reported_and_the_rest_imported() -> TestResult {
    let other = "22222222-2222-4333-8444-555555555555";
    let archive = zip_of(&[
        (format!("{ID}/watch.json"), b"[1, 2]".to_vec()),
        (format!("{other}/watch.json"), watch("https://b.example/")),
    ])?;
    let plan = read(&archive)?;
    assert_eq!(plan.batch.watches.len(), 1);
    assert_eq!(plan.report.totals.skipped, 1);
    assert!(plan.report.items.iter().any(|i| {
        i.notes
            .iter()
            .any(|n| n.message.contains("not a JSON object"))
    }));
    Ok(())
}

const ID_LOW: &str = "00000000-2222-4333-8444-555555555555";
const ID_HIGH: &str = "ffffffff-2222-4333-8444-555555555555";
const SHOPS_TARGET: &str = "ntfy://ntfy.example/shops";

/// A single-file backup with a tag "Shops" that notifies, a watch "muted" that the source mutes
/// and a watch "loud" that it does not, both with the tag. `muted_id` decides which of the two
/// is read first.
fn muted_and_loud(
    muted_id: &str,
    loud_id: &str,
    all_muted: bool,
) -> Result<vigil_import::ImportPlan, ImportError> {
    let backup = serde_json::json!({
        "watching": {
            muted_id: {"url": "https://muted.example/", "title": "muted", "tags": ["t1"],
                       "notification_muted": true},
            loud_id: {"url": "https://loud.example/", "title": "loud", "tags": ["t1"]},
        },
        "settings": {"application": {
            "all_muted": all_muted,
            "tags": {"t1": {"title": "Shops", "notification_urls": [SHOPS_TARGET]}},
        }}
    });
    read(backup.to_string().as_bytes())
}

fn item<'a>(
    plan: &'a vigil_import::ImportPlan,
    source: &str,
) -> Result<&'a vigil_import::report::ItemReport, Box<dyn std::error::Error>> {
    plan.report
        .items
        .iter()
        .find(|i| i.source == source)
        .ok_or_else(|| format!("no item {source}").into())
}

fn tags_of<'a>(
    plan: &'a vigil_import::ImportPlan,
    url: &str,
) -> Result<&'a [String], Box<dyn std::error::Error>> {
    plan.batch
        .watches
        .iter()
        .find(|w| w.settings.url == url)
        .map(|w| w.tags.as_slice())
        .ok_or_else(|| format!("no watch {url}").into())
}

#[test]
fn a_muted_watch_is_left_off_a_tag_that_notifies_whichever_watch_is_read_first() -> TestResult {
    for (muted_id, loud_id) in [(ID_LOW, ID_HIGH), (ID_HIGH, ID_LOW)] {
        let plan = muted_and_loud(muted_id, loud_id, false)?;
        assert!(tags_of(&plan, "https://muted.example/")?.is_empty());
        assert_eq!(tags_of(&plan, "https://loud.example/")?, ["Shops"]);
        // The tag still notifies the watch that is not muted.
        assert_eq!(plan.batch.tags.len(), 1);
        assert_eq!(plan.batch.tags[0].target_urls, [SHOPS_TARGET]);
        assert_eq!(plan.batch.targets.len(), 1);

        let muted = item(&plan, "muted")?;
        assert!(
            muted
                .notes
                .iter()
                .any(|n| { n.option == "notification_muted" && n.message.contains("\"Shops\"") })
        );
        assert!(item(&plan, "loud")?.notes.is_empty());
    }
    Ok(())
}

#[test]
fn when_everything_is_muted_tags_are_made_without_targets() -> TestResult {
    let plan = muted_and_loud(ID_LOW, ID_HIGH, true)?;
    // Neither watch gets a tag that notifies, and nothing makes a target.
    assert!(plan.batch.targets.is_empty());
    assert_eq!(plan.report.totals.targets_created, 0);
    for url in ["https://muted.example/", "https://loud.example/"] {
        assert!(plan.batch.watches.iter().any(|w| w.settings.url == url));
    }
    assert!(plan.batch.tags.iter().all(|t| t.target_urls.is_empty()));
    // Tags that do not notify stay on the watches.
    assert_eq!(tags_of(&plan, "https://loud.example/")?, ["Shops"]);
    assert!(
        plan.report
            .notes
            .iter()
            .any(|n| n.option == "all_muted" && n.message.contains("tags are made without any"))
    );
    Ok(())
}

#[test]
fn a_muted_watch_is_left_off_an_existing_tag_that_has_targets() -> TestResult {
    let backup = serde_json::json!({
        "watching": {
            ID_LOW: {"url": "https://muted.example/", "title": "muted", "tags": ["t1", "t2"],
                     "notification_muted": true},
        },
        "settings": {"application": {"tags": {
            "t1": {"title": "shops"},
            "t2": {"title": "Quiet"},
        }}}
    });
    let tag = |name: &str| vigil_store::Tag {
        id: uuid::Uuid::now_v7(),
        name: name.to_owned(),
        color: "gray".to_owned(),
        default_interval_secs: None,
        created_at: 0,
    };
    let target = |enabled: bool| vigil_store::NotificationTarget {
        id: uuid::Uuid::now_v7(),
        name: "phone".to_owned(),
        url: SHOPS_TARGET.to_owned(),
        enabled,
        default_template: None,
        created_at: 0,
    };
    let (shops, quiet) = (tag("Shops"), tag("Quiet"));
    // "Shops" notifies a target that works; "Quiet" only one that is switched off.
    let (live, off) = (target(true), target(false));
    let linked =
        std::collections::HashMap::from([(shops.id, vec![live.id]), (quiet.id, vec![off.id])]);
    let existing = Existing::new(&[shops, quiet], &linked, &[live, off], &[]);
    let plan = plan(
        backup.to_string().as_bytes(),
        None,
        &Options::default(),
        &existing,
    )?;

    // "shops" is the existing "Shops" apart from its case, so it is left off. "Quiet" does not
    // notify anything and stays.
    assert_eq!(tags_of(&plan, "https://muted.example/")?, ["Quiet"]);
    let notes = &item(&plan, "muted")?.notes;
    assert!(notes.iter().any(|n| n.message.contains("\"shops\"")));
    assert!(!notes.iter().any(|n| n.message.contains("\"Quiet\"")));
    Ok(())
}

#[test]
fn the_targets_of_a_tag_that_exists_are_not_made() -> TestResult {
    let backup = serde_json::json!({
        "watching": {ID_LOW: {"url": "https://news.example/", "title": "news", "tags": ["t1"]}},
        "settings": {"application": {"tags": {
            "t1": {"title": "News", "notification_urls": ["ntfy://ntfy.example/news"]},
        }}}
    });
    let news = vigil_store::Tag {
        id: uuid::Uuid::now_v7(),
        name: "News".to_owned(),
        color: "gray".to_owned(),
        default_interval_secs: None,
        created_at: 0,
    };
    let existing = Existing::new(&[news], &Default::default(), &[], &[]);
    let plan = plan(
        backup.to_string().as_bytes(),
        None,
        &Options::default(),
        &existing,
    )?;
    // The tag is used, and keeps the targets it has; the source's would be left unused.
    assert_eq!(tags_of(&plan, "https://news.example/")?, ["News"]);
    assert!(plan.batch.targets.is_empty());
    assert_eq!(plan.report.totals.targets_created, 0);
    assert_eq!(plan.report.totals.tags_created, 0);

    // A tag that does not exist yet brings its targets.
    let fresh = read(backup.to_string().as_bytes())?;
    assert_eq!(fresh.batch.targets.len(), 1);
    assert_eq!(fresh.report.totals.targets_created, 1);
    Ok(())
}

#[test]
fn a_backup_watch_with_a_body_that_is_too_large_is_not_made() -> TestResult {
    let big = "x".repeat(vigil_core::FetchSettings::MAX_BODY_BYTES + 1);
    let backup = serde_json::json!({
        "watching": {
            ID_LOW: {"url": "https://big.example/", "title": "big", "method": "POST", "body": big},
            ID_HIGH: {"url": "https://ok.example/", "title": "ok"},
        },
        "settings": {"application": {}}
    });
    let plan = read(backup.to_string().as_bytes())?;
    assert_eq!(plan.batch.watches.len(), 1);
    assert_eq!(plan.batch.watches[0].settings.url, "https://ok.example/");
    let skipped = item(&plan, "big")?;
    assert_eq!(skipped.status, vigil_import::report::Status::Skipped);
    assert!(
        skipped
            .notes
            .iter()
            .any(|n| n.message.contains("longer than 1 MiB"))
    );
    Ok(())
}
