//! Conditions evaluated on a realistic change: a procurement listing gets a new tender.

use vigil_core::{Conditions, LineDiff, Pipeline, Processor, Verdict, VerdictReason};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const BEFORE: &str = include_str!("fixtures/tenders.json");
const AFTER: &str = include_str!("fixtures/tenders-v2.json");

fn verdict(conditions: &str) -> Result<Verdict, Box<dyn std::error::Error>> {
    let pipeline = Pipeline::new(Processor::Json, &[])?;
    let render = |body: &str| pipeline.render(body.as_bytes(), Some("application/json"));
    let diff = LineDiff::between(&render(BEFORE)?, &render(AFTER)?);
    let conditions: Conditions = serde_json::from_str(conditions)?;
    Ok(conditions.compile()?.evaluate(&diff))
}

#[test]
fn the_listing_change_notifies_without_conditions() -> TestResult {
    let v = verdict("{}")?;
    assert!(v.notify);
    insta::assert_debug_snapshot!(v);
    Ok(())
}

#[test]
fn a_cyrillic_trigger_finds_the_new_tender() -> TestResult {
    let v = verdict(r#"{"trigger_text": ["ЕЛЕКТРОЕНЕРГІЇ"]}"#)?;
    assert!(v.notify);
    assert_eq!(
        v.triggered_text,
        ["      \"title\": \"Постачання електроенергії для комунальних підприємств\","]
    );
    insta::assert_debug_snapshot!(v);
    Ok(())
}

#[test]
fn a_regex_trigger_can_look_at_amounts() -> TestResult {
    let v = verdict(r#"{"trigger_text": ["/\"amount\": \\d{7}\\./"]}"#)?;
    assert!(v.notify);
    assert_eq!(v.triggered_text, ["        \"amount\": 4200000.0,"]);
    Ok(())
}

#[test]
fn a_trigger_that_is_not_in_the_new_lines_does_not_notify() -> TestResult {
    let v = verdict(r#"{"trigger_text": ["Харківська"]}"#)?;
    assert_eq!(v.reason, VerdictReason::NoTriggerMatched);
    assert!(!v.notify);
    Ok(())
}

#[test]
fn a_block_pattern_on_an_added_line_suppresses() -> TestResult {
    let v = verdict(r#"{"trigger_text": ["тендер"], "block_text": ["Одеська"]}"#)?;
    assert!(!v.notify);
    assert_eq!(v.reason, VerdictReason::Blocked("Одеська".into()));
    Ok(())
}

#[test]
fn a_change_with_added_and_removed_lines_is_the_changed_shape() -> TestResult {
    let v = verdict(r#"{"notify_on": {"changed": false}}"#)?;
    assert!(!v.notify);
    insta::assert_debug_snapshot!(v);
    Ok(())
}
