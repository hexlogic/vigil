//! Snapshot tests of the text pipeline on realistic pages.

use vigil_core::Processor;
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
