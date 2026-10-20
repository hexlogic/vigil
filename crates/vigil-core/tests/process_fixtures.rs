//! Snapshot tests of the text pipeline on realistic pages.

use vigil_core::Processor;
use vigil_core::decode::decode;
use vigil_core::normalize::normalize;
use vigil_core::process::{ProcessError, process, process_text};

#[test]
fn news_page_renders_as_readable_text() -> Result<(), ProcessError> {
    let html = include_str!("fixtures/news.html");
    let text = normalize(&process_text(html, Some("text/html; charset=utf-8"))?);
    insta::assert_snapshot!(text);
    Ok(())
}

fn json_fixture(body: &str) -> Result<String, ProcessError> {
    Ok(normalize(&process(
        Processor::Json,
        body,
        Some("application/json"),
    )?))
}

#[test]
fn json_api_is_canonical() -> Result<(), ProcessError> {
    insta::assert_snapshot!(json_fixture(include_str!("fixtures/api.json"))?);
    Ok(())
}

#[test]
fn tenders_json_is_canonical() -> Result<(), ProcessError> {
    insta::assert_snapshot!(json_fixture(include_str!("fixtures/tenders.json"))?);
    Ok(())
}

#[test]
fn json_output_does_not_depend_on_key_order_or_layout() -> Result<(), ProcessError> {
    let compact = r#"{"b":[1,{"y":1,"x":2}],"a":null}"#;
    let spread = "{\n\"a\": null,\n  \"b\": [ 1, { \"x\": 2, \"y\": 1 } ]\n}";
    assert_eq!(json_fixture(compact)?, json_fixture(spread)?);
    Ok(())
}

fn feed_fixture(bytes: &[u8], content_type: &str) -> Result<String, ProcessError> {
    Ok(normalize(&process(
        Processor::Feed,
        &decode(bytes, Some(content_type)),
        Some(content_type),
    )?))
}

#[test]
fn rss_feed_lists_entries_newest_first_with_undated_ones_last() -> Result<(), ProcessError> {
    let text = feed_fixture(include_bytes!("fixtures/feed.xml"), "application/rss+xml")?;
    insta::assert_snapshot!(text);
    Ok(())
}

#[test]
fn atom_feed_uses_updated_when_there_is_no_published_date() -> Result<(), ProcessError> {
    let text = feed_fixture(include_bytes!("fixtures/atom.xml"), "application/atom+xml")?;
    insta::assert_snapshot!(text);
    Ok(())
}

#[test]
fn feed_with_a_windows_1251_declaration_is_readable() -> Result<(), ProcessError> {
    let text = feed_fixture(
        include_bytes!("fixtures/feed-cp1251.xml"),
        "application/rss+xml",
    )?;
    insta::assert_snapshot!(text);
    Ok(())
}

#[test]
fn feed_metadata_changes_do_not_change_the_text() -> Result<(), ProcessError> {
    let original = include_str!("fixtures/feed.xml");
    let touched = original
        .replace(
            "Mon, 12 Oct 2026 10:00:00 GMT",
            "Tue, 13 Oct 2026 10:00:00 GMT",
        )
        .replace("Harbor Weekly", "Harbor Weekly (renamed)");
    assert_ne!(original, touched);
    assert_eq!(
        normalize(&process(Processor::Feed, original, None)?),
        normalize(&process(Processor::Feed, &touched, None)?)
    );
    Ok(())
}
