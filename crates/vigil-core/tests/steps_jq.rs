//! Snapshot tests of the jq step.

use vigil_core::{Pipeline, Processor, Step};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TENDERS: &str = include_str!("fixtures/tenders.json");
const API: &str = include_str!("fixtures/api.json");

fn render_with(
    processor: Processor,
    steps: &str,
    body: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let steps: Vec<Step> = serde_json::from_str(steps)?;
    let pipeline = Pipeline::new(processor, &steps)?;
    Ok(pipeline.render(body.as_bytes(), Some("application/json"))?)
}

fn jq(program: &str, body: &str) -> Result<String, Box<dyn std::error::Error>> {
    let steps = serde_json::json!([{"type": "jq", "program": program}]);
    render_with(Processor::Json, &steps.to_string(), body)
}

fn error_of(program: &str, body: &str) -> String {
    jq(program, body)
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default()
}

#[test]
fn jq_outputs_are_collected_into_an_array() -> TestResult {
    insta::assert_snapshot!(jq(".data[].title", TENDERS)?);
    Ok(())
}

#[test]
fn jq_projects_and_filters() -> TestResult {
    let program = ".data[] | select(.value.amount > 1000000) | {id, title, amount: .value.amount}";
    insta::assert_snapshot!(jq(program, TENDERS)?);
    Ok(())
}

#[test]
fn jq_with_a_single_output_is_still_an_array() -> TestResult {
    assert_eq!(jq(".meta.total", API)?, "[\n  3\n]\n");
    assert_eq!(
        jq("[.items[] | .name]", API)?,
        "[\n  [\n    \"Desk lamp\",\n    \"Notebook A5\",\n    \"Кавова чашка\"\n  ]\n]\n"
    );
    Ok(())
}

#[test]
fn jq_without_output_gives_an_empty_array() -> TestResult {
    assert_eq!(
        jq(".data[] | select(.status == \"none\")", TENDERS)?,
        "[]\n"
    );
    assert_eq!(jq("empty", TENDERS)?, "[]\n");
    Ok(())
}

#[test]
fn jq_key_order_follows_the_canonical_printer() -> TestResult {
    let text = jq("{z: 1, a: 2}", "{}")?;
    assert_eq!(text, "[\n  {\n    \"a\": 2,\n    \"z\": 1\n  }\n]\n");
    Ok(())
}

#[test]
fn jq_dates_are_available_but_the_clock_is_not() -> TestResult {
    assert_eq!(
        jq("1760256000 | todate", "{}")?,
        "[\n  \"2025-10-12T08:00:00Z\"\n]\n"
    );
    assert_eq!(
        jq("\"2025-10-12T08:00:00Z\" | fromdateiso8601", "{}")?,
        "[\n  1760256000\n]\n"
    );
    Ok(())
}

#[test]
fn jq_filters_that_read_the_world_are_not_available() {
    for (program, reason) in [
        ("env", "filter env/0 is not defined"),
        ("now", "filter now/0 is not defined"),
        ("input", "filter input/0 is not defined"),
        ("[inputs]", "filter inputs/0 is not defined"),
        ("halt", "filter halt/0 is not defined"),
        ("halt(3)", "filter halt/1 is not defined"),
        ("halt_error", "filter halt_error/0 is not defined"),
        ("debug", "filter debug/0 is not defined"),
        ("stderr", "filter stderr/0 is not defined"),
        ("input_filename", "filter input_filename/0 is not defined"),
        ("localtime", "filter localtime/0 is not defined"),
        (
            "1 | strflocaltime(\"%Y\")",
            "filter strflocaltime/1 is not defined",
        ),
        ("$ENV", "variable $ENV is not defined"),
    ] {
        assert_eq!(
            error_of(program, "{}"),
            format!("step 1 (jq): invalid jq program: {reason}"),
            "{program}"
        );
    }
}

#[test]
fn jq_syntax_errors_name_the_step_and_position() {
    for program in [".[", ".a |", "{", "foo(", ") (", "\"unterminated"] {
        let message = error_of(program, "{}");
        assert!(
            message.starts_with("step 1 (jq): invalid jq program: "),
            "{program}: {message}"
        );
        assert!(!message.contains('\n'), "{program}: {message}");
    }
    assert_eq!(
        error_of(".a | | .b", "{}"),
        "step 1 (jq): invalid jq program: syntax error at byte 5"
    );
}

#[test]
fn jq_undefined_filters_are_reported() {
    assert_eq!(
        error_of("frobnicate(1)", "{}"),
        "step 1 (jq): invalid jq program: filter frobnicate/1 is not defined"
    );
}

#[test]
fn jq_runtime_errors_are_reported() {
    assert_eq!(
        error_of("error(\"boom\")", "{}"),
        "step 1 (jq): jq failed: \"boom\""
    );
    let message = error_of(".a.b", "1");
    assert!(message.starts_with("step 1 (jq): jq failed: "), "{message}");
    assert!(message.contains("cannot index"), "{message}");
}

#[test]
fn jq_programs_that_are_too_long_or_too_nested_are_refused() {
    // Long chains of operators recurse in the parser and overflow the stack, so the length
    // is limited as well as the brackets.
    for long in [
        format!("{}1", " ".repeat(2049)),
        format!("{}1", "-".repeat(16_000)),
        format!("{}.", ".|".repeat(8_100)),
    ] {
        assert_eq!(
            error_of(&long, "{}"),
            "step 1 (jq): invalid jq program: the program is longer than 2048 bytes"
        );
    }
    let nested = format!("{}1{}", "(".repeat(100), ")".repeat(100));
    assert_eq!(
        error_of(&nested, "{}"),
        "step 1 (jq): invalid jq program: brackets are nested deeper than 32 levels"
    );
    let fine = format!("{}1{}", "(".repeat(30), ")".repeat(30));
    assert!(jq(&fine, "{}").is_ok());
}

#[test]
fn jq_stops_at_the_output_count_limit() -> TestResult {
    assert!(jq("range(10000)", "{}")?.starts_with("[\n  0,\n  1,"));
    assert_eq!(
        error_of("range(10001)", "{}"),
        "step 1 (jq): jq produced more than 10000 outputs"
    );
    Ok(())
}

#[test]
fn jq_stops_at_the_output_size_limit() {
    assert_eq!(
        error_of("\"x\" * 20000000", "{}"),
        "step 1 (jq): the output is larger than 16 MiB"
    );
}

#[test]
fn jq_follows_jsonpath() -> TestResult {
    let steps = r#"[{"type": "jsonpath", "expr": "$.data[*]"},
                    {"type": "jq", "program": ".[] | .procuringEntity.region"}]"#;
    let text = render_with(Processor::Json, steps, TENDERS)?;
    assert_eq!(
        text,
        "[\n  \"Львівська область\",\n  \"Київ\",\n  \"Харківська область\"\n]\n"
    );
    Ok(())
}

#[test]
fn jq_needs_the_json_processor() {
    let steps = r#"[{"type": "jq", "program": "."}]"#;
    for processor in [Processor::Text, Processor::Feed] {
        let message = render_with(processor, steps, "{}")
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert_eq!(
            message,
            format!(
                "step 1 (jq): json steps need the json processor, but the watch uses {processor}"
            )
        );
    }
}

#[test]
fn jq_maps_what_json_cannot_hold_the_way_jq_prints_it() -> TestResult {
    assert_eq!(jq("nan", "{}")?, "[\n  null\n]\n");
    assert_eq!(jq("[nan, 1]", "{}")?, "[\n  [\n    null,\n    1\n  ]\n]\n");
    assert_eq!(jq("infinite", "{}")?, "[\n  1.7976931348623157e+308\n]\n");
    assert_eq!(jq("-infinite", "{}")?, "[\n  -1.7976931348623157e+308\n]\n");
    // The minimum of an empty list, a common way to meet infinity.
    let program = "reduce (.[]) as $x (infinite; if $x < . then $x else . end)";
    assert_eq!(jq(program, "[]")?, "[\n  1.7976931348623157e+308\n]\n");
    assert_eq!(jq(program, "[3, 1, 2]")?, "[\n  1\n]\n");
    Ok(())
}

#[test]
fn jq_keeps_integers_exact_and_big_numbers_as_floats() -> TestResult {
    assert_eq!(jq(".", "9007199254740993")?, "[\n  9007199254740993\n]\n");
    // Beyond 64 bits the nearest float is all JSON can hold.
    assert_eq!(
        jq(". + 1", "18446744073709551615")?,
        "[\n  1.8446744073709552e+19\n]\n"
    );
    assert_eq!(jq(".[0] * 2", "[1.5]")?, "[\n  3.0\n]\n");
    Ok(())
}

#[test]
fn jq_values_that_are_not_json_are_reported_in_words() {
    assert_eq!(
        error_of("{(1): 2}", "{}"),
        "step 1 (jq): jq failed: an object key is not text, which JSON needs"
    );
    assert_eq!(
        error_of("reduce range(0; 200) as $i (null; [.])", "{}"),
        "step 1 (jq): jq failed: the program built a value nested more than 128 levels deep"
    );
}

#[test]
fn jq_values_that_share_structure_cannot_blow_up_the_copy() {
    // 2^25 leaves described by a few dozen nodes; copying them out is stopped by the limit.
    assert_eq!(
        error_of("reduce range(0; 25) as $i (1; [., .])", "{}"),
        "step 1 (jq): the output is larger than 16 MiB"
    );
}

#[test]
fn jq_text_with_bytes_that_are_not_utf8_is_cleaned() -> TestResult {
    assert_eq!(
        jq("\"abc\" | explode | implode", "{}")?,
        "[\n  \"abc\"\n]\n"
    );
    Ok(())
}
