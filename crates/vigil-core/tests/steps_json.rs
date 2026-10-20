//! Snapshot tests of the JSON steps.

use vigil_core::{Pipeline, Processor, Step};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TENDERS: &str = include_str!("fixtures/tenders.json");

fn render_with(
    processor: Processor,
    steps: &str,
    body: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let steps: Vec<Step> = serde_json::from_str(steps)?;
    let pipeline = Pipeline::new(processor, &steps)?;
    Ok(pipeline.render(body.as_bytes(), Some("application/json"))?)
}

fn render(steps: &str, body: &str) -> Result<String, Box<dyn std::error::Error>> {
    render_with(Processor::Json, steps, body)
}

fn jsonpath(expr: &str, body: &str) -> Result<String, Box<dyn std::error::Error>> {
    render(
        &serde_json::json!([{"type": "jsonpath", "expr": expr}]).to_string(),
        body,
    )
}

fn error_of(processor: Processor, steps: &str, body: &str) -> String {
    render_with(processor, steps, body)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default()
}

#[test]
fn jsonpath_collects_matches_into_an_array() -> TestResult {
    insta::assert_snapshot!(jsonpath("$.data[*].title", TENDERS)?);
    Ok(())
}

#[test]
fn jsonpath_with_one_match_is_still_an_array() -> TestResult {
    assert_eq!(
        jsonpath("$.data[0].id", TENDERS)?,
        "[\n  \"UA-2026-10-12-000123-a\"\n]\n"
    );
    Ok(())
}

#[test]
fn jsonpath_supports_filters() -> TestResult {
    insta::assert_snapshot!(jsonpath(
        "$.data[?@.value.amount > 1000000].title",
        TENDERS
    )?);
    assert_eq!(
        jsonpath("$.data[?@.status == 'complete'].id", TENDERS)?,
        "[\n  \"UA-2026-10-09-000789-c\"\n]\n"
    );
    Ok(())
}

#[test]
fn jsonpath_supports_functions_and_slices() -> TestResult {
    assert_eq!(
        jsonpath("$.data[?length(@.title) > 40].id", TENDERS)?,
        "[\n  \"UA-2026-10-11-000456-b\"\n]\n"
    );
    assert_eq!(
        jsonpath("$.data[0:2].status", TENDERS)?,
        "[\n  \"active.tendering\",\n  \"active.tendering\"\n]\n"
    );
    Ok(())
}

#[test]
fn jsonpath_can_return_whole_objects() -> TestResult {
    insta::assert_snapshot!(jsonpath("$.data[?@.status == 'complete']", TENDERS)?);
    Ok(())
}

#[test]
fn jsonpath_steps_chain() -> TestResult {
    let text = render(
        r#"[{"type": "jsonpath", "expr": "$.data[*]"},
            {"type": "jsonpath", "expr": "$[?@.status == 'complete'].procuringEntity"}]"#,
        TENDERS,
    )?;
    insta::assert_snapshot!(text);
    Ok(())
}

#[test]
fn jsonpath_output_feeds_text_steps() -> TestResult {
    let text = render(
        r#"[{"type": "jsonpath", "expr": "$.data[*].title"},
            {"type": "ignore_lines", "patterns": ["Канцелярське", "/^[\\[\\]]$/"]},
            {"type": "trim"}]"#,
        TENDERS,
    )?;
    assert_eq!(
        text,
        "\"Закупівля паливно-мастильних матеріалів\",\n\"Послуги з поточного ремонту дорожнього покриття\",\n"
    );
    Ok(())
}

#[test]
fn jsonpath_that_finds_nothing_is_an_error() {
    assert_eq!(
        error_of(
            Processor::Json,
            r#"[{"type": "jsonpath", "expr": "$.data[*].missing"}]"#,
            TENDERS
        ),
        "step 1 (jsonpath): matched nothing"
    );
}

#[test]
fn an_invalid_query_names_the_step() {
    for expr in ["$.data[", "data", "$..[?@.a ==]", "$.a.b.["] {
        let steps = serde_json::json!([{"type": "jsonpath", "expr": expr}]).to_string();
        let message = error_of(Processor::Json, &steps, "{}");
        assert!(
            message.starts_with(&format!("step 1 (jsonpath): invalid jsonpath {expr:?}: ")),
            "{expr}: {message}"
        );
        assert!(!message.contains('\n'), "{expr}: {message}");
    }
}

#[test]
fn a_body_that_is_not_json_fails_without_naming_a_step() {
    let message = error_of(
        Processor::Json,
        r#"[{"type": "jsonpath", "expr": "$"}]"#,
        "<html></html>",
    );
    assert!(
        message.starts_with("the response is not valid JSON: "),
        "{message}"
    );
}

#[test]
fn json_steps_need_the_json_processor() {
    for processor in [Processor::Text, Processor::Feed] {
        assert_eq!(
            error_of(processor, r#"[{"type": "jsonpath", "expr": "$"}]"#, "{}"),
            format!(
                "step 1 (jsonpath): json steps need the json processor, but the watch uses {processor}"
            )
        );
    }
}

#[test]
fn json_steps_must_come_before_text_steps() {
    assert_eq!(
        error_of(
            Processor::Json,
            r#"[{"type": "lowercase"}, {"type": "jsonpath", "expr": "$"}]"#,
            "{}"
        ),
        "step 2 (jsonpath): json steps must come before text steps"
    );
}

#[test]
fn html_steps_cannot_follow_json_steps() {
    assert_eq!(
        error_of(
            Processor::Json,
            r#"[{"type": "jsonpath", "expr": "$"}, {"type": "css", "selector": "p"}]"#,
            "{}"
        ),
        "step 2 (css): html steps must come before json steps"
    );
}

#[test]
fn overlapping_matches_stop_at_the_output_limit() {
    let big = "x".repeat(100_000);
    let body = format!("[{}]", vec![format!("\"{big}\""); 200].join(","));
    assert_eq!(
        error_of(
            Processor::Json,
            r#"[{"type": "jsonpath", "expr": "$..*"}]"#,
            &body
        ),
        "step 1 (jsonpath): the output is larger than 16 MiB"
    );
}

#[test]
fn jsonpath_handles_scalars_and_deep_documents() -> TestResult {
    assert_eq!(jsonpath("$", "42")?, "[\n  42\n]\n");
    assert_eq!(jsonpath("$", "null")?, "[\n  null\n]\n");
    let deep = format!("{}1{}", "[".repeat(100), "]".repeat(100));
    jsonpath("$..*", &deep)?;
    Ok(())
}
