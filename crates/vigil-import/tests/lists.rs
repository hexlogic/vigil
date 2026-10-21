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
    let existing = Existing::new(&[], &Default::default(), &[], &store_has);
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

fn urlwatch_watch(yaml: &str) -> Result<(vigil_store::ImportWatch, Vec<String>), ImportError> {
    let plan = plan(
        yaml.as_bytes(),
        Some(Format::Urlwatch),
        &Options::default(),
        &Existing::default(),
    )?;
    let watch = plan.batch.watches.into_iter().next();
    let notes = plan.report.items.into_iter().flat_map(|i| i.notes);
    let messages = notes
        .map(|n| format!("{}: {}", n.option, n.message))
        .collect();
    watch.map(|w| (w, messages)).ok_or(ImportError::Invalid {
        format: Format::Urlwatch,
        reason: "no watch".to_owned(),
    })
}

#[test]
fn a_urlwatch_job_with_data_and_no_method_is_posted() -> TestResult {
    let (made, notes) = urlwatch_watch("url: https://a.example/\ndata:\n  q: x y\n")?;
    let fetch = &made.settings.fetch;
    assert_eq!(fetch.method, vigil_core::Method::Post);
    assert_eq!(fetch.body.as_deref(), Some("q=x+y"));
    assert_eq!(
        fetch.headers.get("Content-Type").map(String::as_str),
        Some("application/x-www-form-urlencoded")
    );
    assert!(notes.iter().any(|n| n.contains("no method")), "{notes:?}");
    Ok(())
}

#[test]
fn a_urlwatch_job_that_names_get_keeps_get_and_loses_its_data() -> TestResult {
    let (made, notes) =
        urlwatch_watch("url: https://a.example/\nmethod: GET\ndata: q=1\njson: true\n")?;
    let fetch = &made.settings.fetch;
    assert_eq!(fetch.method, vigil_core::Method::Get);
    assert_eq!(fetch.body, None);
    // A body that is not sent has no content type to go with it.
    assert!(fetch.headers.is_empty(), "{:?}", fetch.headers);
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("data:") && n.contains("method is GET")),
        "{notes:?}"
    );
    // It is not told that the job had no method: it had one.
    assert!(!notes.iter().any(|n| n.contains("no method")), "{notes:?}");
    Ok(())
}

#[test]
fn a_urlwatch_method_that_is_not_supported_falls_back_to_get_without_the_data() -> TestResult {
    let (made, notes) = urlwatch_watch("url: https://a.example/\nmethod: OPTIONS\ndata: q=1\n")?;
    assert_eq!(made.settings.fetch.method, vigil_core::Method::Get);
    assert_eq!(made.settings.fetch.body, None);
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("method:") && n.contains("GET is used")),
        "{notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("data:") && n.contains("method is GET")),
        "{notes:?}"
    );
    assert!(!notes.iter().any(|n| n.contains("no method")), "{notes:?}");
    Ok(())
}

#[test]
fn a_urlwatch_job_with_data_that_is_not_text_says_so() -> TestResult {
    let (made, notes) = urlwatch_watch("url: https://a.example/\nmethod: POST\ndata: [1, 2]\n")?;
    assert_eq!(made.settings.fetch.body, None);
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("data:") && n.contains("neither text nor a mapping")),
        "{notes:?}"
    );
    Ok(())
}

#[test]
fn a_urlwatch_job_with_a_body_that_is_too_large_is_not_made_and_says_why() -> TestResult {
    let big = "x".repeat(vigil_core::FetchSettings::MAX_BODY_BYTES + 1);
    let yaml = format!(
        "---\nurl: https://big.example/\nname: Big\nmethod: POST\ndata: {big}\n---\nurl: https://ok.example/\nname: Ok\n"
    );
    let plan = plan(
        yaml.as_bytes(),
        Some(Format::Urlwatch),
        &Options::default(),
        &Existing::default(),
    )?;
    // The other job is made; this one is reported, with the reason and not the body.
    assert_eq!(plan.batch.watches.len(), 1);
    assert_eq!(plan.batch.watches[0].settings.url, "https://ok.example/");
    let big = plan
        .report
        .items
        .iter()
        .find(|i| i.source == "Big")
        .ok_or("no item")?;
    assert_eq!(big.status, vigil_import::report::Status::Skipped);
    assert!(
        big.notes
            .iter()
            .any(|n| n.message.contains("longer than 1 MiB")),
        "{:?}",
        big.notes
    );
    assert_eq!(plan.report.totals.skipped, 1);
    Ok(())
}
