//! Snapshot tests of the text steps.

use vigil_core::normalize::normalize;
use vigil_core::process::process_text;
use vigil_core::{Pipeline, Processor, Step};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const NEWS: &str = include_str!("fixtures/news.html");
const PRODUCT: &str = include_str!("fixtures/product.html");
const STATUS: &str = include_str!("fixtures/status.txt");
const TENDERS: &str = include_str!("fixtures/tenders.json");

/// Build a pipeline from stored JSON steps and render `body` with it.
fn render(
    processor: Processor,
    steps: &str,
    body: &str,
    content_type: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let steps: Vec<Step> = serde_json::from_str(steps)?;
    Ok(Pipeline::new(processor, &steps)?.render(body.as_bytes(), Some(content_type))?)
}

fn html(steps: &str, body: &str) -> Result<String, Box<dyn std::error::Error>> {
    render(Processor::Text, steps, body, "text/html; charset=utf-8")
}

fn status(steps: &str) -> Result<String, Box<dyn std::error::Error>> {
    render(Processor::Text, steps, STATUS, "text/plain")
}

fn error_of(steps: &str, body: &str) -> String {
    let result = serde_json::from_str::<Vec<Step>>(steps)
        .map_err(|e| e.to_string())
        .and_then(|steps| Pipeline::new(Processor::Text, &steps).map_err(|e| e.to_string()))
        .and_then(|pipeline| {
            pipeline
                .render(body.as_bytes(), Some("text/plain"))
                .map_err(|e| e.to_string())
        });
    result.err().unwrap_or_default()
}

#[test]
fn an_empty_pipeline_matches_the_plain_text_processor() -> TestResult {
    for (body, content_type) in [
        (NEWS, "text/html; charset=utf-8"),
        (PRODUCT, "text/html"),
        (STATUS, "text/plain"),
    ] {
        let expected = normalize(&process_text(body, Some(content_type))?);
        assert_eq!(render(Processor::Text, "[]", body, content_type)?, expected);
    }
    Ok(())
}

#[test]
fn regex_extract_keeps_every_match_on_its_own_line() -> TestResult {
    let prices = html(
        r#"[{"type": "regex_extract", "pattern": "€\\d+\\.\\d{2}"}]"#,
        PRODUCT,
    )?;
    insta::assert_snapshot!(prices);
    Ok(())
}

#[test]
fn regex_extract_can_keep_a_capture_group() -> TestResult {
    let sku = html(
        r#"[{"type": "regex_extract", "pattern": "SKU: (\\S+)", "group": 1}]"#,
        PRODUCT,
    )?;
    assert_eq!(sku, "TB-30-GRN\n");
    Ok(())
}

#[test]
fn regex_extract_with_a_missing_group_fails_when_built() {
    let steps = r#"[{"type": "regex_extract", "pattern": "a(b)", "group": 2}]"#;
    assert_eq!(
        error_of(steps, "ab"),
        "step 1 (regex_extract): capture group 2 does not exist, the pattern has 1"
    );
}

#[test]
fn regex_extract_that_finds_nothing_leaves_empty_text() -> TestResult {
    let steps = r#"[{"type": "regex_extract", "pattern": "\\d{9}"}]"#;
    assert_eq!(
        render(Processor::Text, steps, "no digits", "text/plain")?,
        ""
    );
    Ok(())
}

#[test]
fn regex_extract_skips_empty_matches() -> TestResult {
    let steps = r#"[{"type": "regex_extract", "pattern": "x*"}]"#;
    assert_eq!(render(Processor::Text, steps, "abc", "text/plain")?, "");
    Ok(())
}

#[test]
fn regex_extract_required_turns_nothing_found_into_an_error() {
    for (pattern, text) in [("\\d{9}", "no digits"), ("x*", "abc")] {
        let steps =
            serde_json::json!([{"type": "regex_extract", "pattern": pattern, "required": true}]);
        assert_eq!(
            error_of(&steps.to_string(), text),
            "step 1 (regex_extract): matched nothing",
            "{pattern}"
        );
    }
}

#[test]
fn an_invalid_regex_names_the_step() {
    let steps =
        r#"[{"type": "sort"}, {"type": "regex_replace", "pattern": "(", "replacement": ""}]"#;
    let message = error_of(steps, "x");
    assert!(
        message.starts_with("step 2 (regex_replace): invalid regex \"(\": "),
        "{message}"
    );
    assert!(!message.contains('\n'), "{message}");
}

#[test]
fn regex_replace_rewrites_matches_with_group_references() -> TestResult {
    let text = html(
        r#"[{"type": "regex_replace", "pattern": "(\\d+) minutes ago", "replacement": "${1}m ago"}]"#,
        NEWS,
    )?;
    insta::assert_snapshot!(text);
    Ok(())
}

#[test]
fn regex_replace_without_a_match_changes_nothing() -> TestResult {
    let steps = r#"[{"type": "regex_replace", "pattern": "zzz", "replacement": "y"}]"#;
    assert_eq!(html(steps, NEWS)?, html("[]", NEWS)?);
    Ok(())
}

#[test]
fn regex_replace_stops_at_the_output_limit() {
    let big = "x".repeat(400_000);
    let steps =
        format!(r#"[{{"type": "regex_replace", "pattern": "(?m)^", "replacement": "{big}"}}]"#);
    let body = "line\n".repeat(100);
    assert_eq!(
        error_of(&steps, &body),
        "step 1 (regex_replace): the output is larger than 16 MiB"
    );
}

#[test]
fn ignore_lines_drops_substring_and_regex_matches() -> TestResult {
    let text = html(
        r#"[{"type": "ignore_lines", "patterns": ["MINUTES AGO", "/^\\[home\\]/i"]}]"#,
        NEWS,
    )?;
    insta::assert_snapshot!(text);
    Ok(())
}

#[test]
fn ignore_lines_rejects_invalid_patterns_when_built() {
    let steps = r#"[{"type": "ignore_lines", "patterns": ["ok", "/(/"]}]"#;
    assert!(error_of(steps, "x").starts_with("step 1 (ignore_lines): invalid regex "));
    let steps = r#"[{"type": "ignore_lines", "patterns": [""]}]"#;
    assert_eq!(
        error_of(steps, "x"),
        "step 1 (ignore_lines): the pattern is empty"
    );
}

#[test]
fn sort_orders_lines() -> TestResult {
    insta::assert_snapshot!(status(r#"[{"type": "sort"}]"#)?);
    Ok(())
}

#[test]
fn unique_keeps_the_first_of_each_line() -> TestResult {
    insta::assert_snapshot!(status(r#"[{"type": "unique"}]"#)?);
    Ok(())
}

#[test]
fn trim_removes_leading_whitespace() -> TestResult {
    insta::assert_snapshot!(status(r#"[{"type": "trim"}]"#)?);
    Ok(())
}

#[test]
fn collapse_whitespace_squeezes_runs_inside_lines() -> TestResult {
    insta::assert_snapshot!(status(r#"[{"type": "collapse_whitespace"}]"#)?);
    Ok(())
}

#[test]
fn lowercase_handles_cyrillic() -> TestResult {
    let text = render(
        Processor::Json,
        r#"[{"type": "lowercase"}]"#,
        TENDERS,
        "application/json",
    )?;
    insta::assert_snapshot!(text);
    Ok(())
}

#[test]
fn steps_run_in_order_on_the_output_of_the_previous_one() -> TestResult {
    let text = status(
        r#"[{"type": "collapse_whitespace"}, {"type": "trim"}, {"type": "unique"},
            {"type": "sort"}, {"type": "ignore_lines", "patterns": ["cache"]}]"#,
    )?;
    insta::assert_snapshot!(text);
    Ok(())
}

#[test]
fn blank_lines_left_by_a_step_do_not_survive() -> TestResult {
    let text = status(r#"[{"type": "ignore_lines", "patterns": ["service"]}]"#)?;
    assert_eq!(text, "  disk   usage:   71%\n  disk   usage:   71%\n");
    Ok(())
}
