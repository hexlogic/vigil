//! Reading a list of addresses and an OPML outline.

use vigil_import::{Existing, Format, ImportError, Options, plan};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const LIST: &[u8] = include_bytes!("fixtures/sites.txt");
const OPML: &[u8] = include_bytes!("fixtures/feeds.opml");

fn names(plan: &vigil_import::ImportPlan) -> Vec<(String, String, bool)> {
    plan.batch
        .watches
        .iter()
        .map(|w| (w.settings.name.clone(), w.settings.url.clone(), w.paused))
        .collect()
}

#[test]
fn a_list_becomes_watches_with_tags_and_a_report() -> TestResult {
    let plan = plan(LIST, None, &Options::default(), &Existing::default())?;
    assert_eq!(plan.report.format, Format::UrlList);
    insta::assert_json_snapshot!("list_report", plan.report);
    insta::assert_debug_snapshot!("list_batch", plan.batch);
    Ok(())
}

#[test]
fn the_interval_and_duplicates_follow_the_options() -> TestResult {
    let options = Options {
        every: std::time::Duration::from_secs(900),
        ..Options::default()
    };
    let plan = plan(
        b"https://a.example/\nhttps://a.example/\n",
        None,
        &options,
        &Existing::default(),
    )?;
    // The same address twice in one file is two watches: they may differ.
    assert_eq!(plan.batch.watches.len(), 2);
    assert!(
        plan.batch
            .watches
            .iter()
            .all(|w| w.settings.schedule == vigil_core::Schedule::Interval(900))
    );
    Ok(())
}

#[test]
fn an_address_that_is_watched_already_is_skipped_unless_duplicates_are_allowed() -> TestResult {
    let store_has = vec![vigil_core::Watch {
        id: vigil_core::WatchId::from_uuid(uuid_nil()),
        name: "x".into(),
        url: "https://example.com/pricing".into(),
        paused: false,
        fetch: Default::default(),
        schedule: vigil_core::Schedule::Interval(60),
        jitter_secs: 0,
        processor: vigil_core::Processor::Text,
        pipeline: vec![],
        conditions: Default::default(),
        snapshot_retention: 100,
        notification_template: None,
        created_at: 0,
        updated_at: 0,
        state: Default::default(),
    }];
    let existing = Existing::new(&[], &[], &store_has);
    let skipped = plan(LIST, None, &Options::default(), &existing)?;
    assert_eq!(skipped.report.totals.skipped, 4);
    assert!(
        skipped.report.items[0]
            .notes
            .iter()
            .any(|n| n.message.contains("exists already"))
    );
    let allowed = plan(
        LIST,
        None,
        &Options {
            allow_duplicates: true,
            ..Options::default()
        },
        &existing,
    )?;
    assert_eq!(allowed.report.totals.skipped, 2);
    assert!(allowed.batch.allow_duplicates);
    Ok(())
}

fn uuid_nil() -> uuid::Uuid {
    uuid::Uuid::nil()
}

#[test]
fn an_outline_becomes_feed_watches_tagged_by_their_folder() -> TestResult {
    let plan = plan(OPML, None, &Options::default(), &Existing::default())?;
    assert_eq!(plan.report.format, Format::Opml);
    insta::assert_json_snapshot!("opml_report", plan.report);
    insta::assert_debug_snapshot!("opml_batch", plan.batch);
    let made = names(&plan);
    assert!(made.contains(&(
        "Example Blog".into(),
        "https://blog.example/feed.xml".into(),
        false
    )));
    Ok(())
}

#[test]
fn files_that_cannot_be_read_are_errors_not_empty_plans() {
    let options = Options::default();
    let existing = Existing::default();
    assert!(matches!(
        plan(b"just some words\n", None, &options, &existing),
        Err(ImportError::UnknownFormat)
    ));
    assert!(matches!(
        plan(
            b"<opml><body><outline text=\"x\"></body></opml>",
            Some(Format::Opml),
            &options,
            &existing
        ),
        Err(ImportError::Invalid {
            format: Format::Opml,
            ..
        })
    ));
    assert!(matches!(
        plan(b"<rss></rss>", Some(Format::Opml), &options, &existing),
        Err(ImportError::Invalid {
            format: Format::Opml,
            ..
        })
    ));
    assert!(matches!(
        plan(&[0xff, 0xfe, 0x00], Some(Format::Opml), &options, &existing),
        Err(ImportError::Invalid { .. })
    ));
}

#[test]
fn nothing_in_a_report_repeats_a_query_string_or_a_credential() -> TestResult {
    for (bytes, format) in [(LIST, Format::UrlList), (OPML, Format::Opml)] {
        let plan = plan(
            bytes,
            Some(format),
            &Options::default(),
            &Existing::default(),
        )?;
        let shown = serde_json::to_string(&plan.report)?;
        assert!(
            !shown.contains("SECRET") && !shown.contains("utm="),
            "{shown}"
        );
    }
    Ok(())
}

const URLWATCH: &[u8] = include_bytes!("fixtures/urls.yaml");

#[test]
fn a_urlwatch_job_file_becomes_watches_and_a_report() -> TestResult {
    let plan = plan(URLWATCH, None, &Options::default(), &Existing::default())?;
    assert_eq!(plan.report.format, Format::Urlwatch);
    insta::assert_json_snapshot!("urlwatch_report", plan.report);
    insta::assert_debug_snapshot!("urlwatch_batch", plan.batch);
    let shown = serde_json::to_string(&plan.report)?;
    for secret in ["SECRET", "proxy.example"] {
        assert!(!shown.contains(secret), "the report repeats {secret}");
    }
    Ok(())
}

#[test]
fn yaml_that_is_not_yaml_is_an_error() {
    assert!(matches!(
        plan(
            b"url: [unclosed",
            Some(Format::Urlwatch),
            &Options::default(),
            &Existing::default()
        ),
        Err(ImportError::Invalid {
            format: Format::Urlwatch,
            ..
        })
    ));
}

const DISTILL: &[u8] = include_bytes!("fixtures/export.json");

#[test]
fn a_distill_export_becomes_watches_and_a_report() -> TestResult {
    let plan = plan(DISTILL, None, &Options::default(), &Existing::default())?;
    assert_eq!(plan.report.format, Format::Distill);
    insta::assert_json_snapshot!("distill_report", plan.report);
    insta::assert_debug_snapshot!("distill_batch", plan.batch);
    assert!(!serde_json::to_string(&plan.report)?.contains("SECRET"));
    Ok(())
}
