//! Snapshot tests of the text pipeline on realistic pages.

use vigil_core::normalize::normalize;
use vigil_core::process::{ProcessError, process_text};

#[test]
fn news_page_renders_as_readable_text() -> Result<(), ProcessError> {
    let html = include_str!("fixtures/news.html");
    let text = normalize(&process_text(html, Some("text/html; charset=utf-8"))?);
    insta::assert_snapshot!(text);
    Ok(())
}
