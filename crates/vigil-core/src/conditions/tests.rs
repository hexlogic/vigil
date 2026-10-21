use serde_json::json;

use super::{Conditions, DiffShape, NotifyOn, Scope, Verdict, VerdictReason};
use crate::diff::LineDiff;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn conditions(value: serde_json::Value) -> Result<Conditions, serde_json::Error> {
    serde_json::from_value(value)
}

fn verdict(c: &Conditions, old: &str, new: &str) -> Result<Verdict, Box<dyn std::error::Error>> {
    Ok(c.compile()?.evaluate(&LineDiff::between(old, new)))
}

#[test]
fn empty_conditions_notify_about_everything() -> TestResult {
    let c = conditions(json!({}))?;
    assert_eq!(c, Conditions::default());
    assert_eq!(
        c.notify_on,
        NotifyOn {
            added: true,
            removed: true,
            changed: true
        }
    );
    for (old, new) in [("a\n", "a\nb\n"), ("a\nb\n", "a\n"), ("a\n", "b\n")] {
        assert!(verdict(&c, old, new)?.notify);
    }
    Ok(())
}

#[test]
fn partial_notify_on_keeps_the_other_flags_on() -> TestResult {
    let c = conditions(json!({"notify_on": {"removed": false}}))?;
    assert_eq!(
        c.notify_on,
        NotifyOn {
            added: true,
            removed: false,
            changed: true
        }
    );
    Ok(())
}

#[test]
fn the_stored_shape_is_pinned() -> TestResult {
    let c = Conditions {
        trigger_text: vec!["sale".into()],
        block_text: vec!["/sold out/i".into()],
        notify_on: NotifyOn {
            added: true,
            removed: false,
            changed: true,
        },
        scope: Scope::Added,
    };
    let shape = json!({
        "trigger_text": ["sale"],
        "block_text": ["/sold out/i"],
        "notify_on": {"added": true, "removed": false, "changed": true}
    });
    assert_eq!(serde_json::to_value(&c)?, shape);
    assert_eq!(conditions(shape)?, c);
    Ok(())
}

#[test]
fn unknown_fields_are_rejected() {
    for bad in [
        json!({"trigger": ["x"]}),
        json!({"notify_on": {"add": true}}),
        json!({"trigger_text": "x"}),
    ] {
        assert!(conditions(bad.clone()).is_err(), "{bad}");
    }
}

#[test]
fn shapes_follow_the_added_and_removed_lines() {
    let shape = |old, new| DiffShape::of(&LineDiff::between(old, new));
    assert_eq!(shape("a\n", "a\nb\n"), Some(DiffShape::Added));
    assert_eq!(shape("a\nb\n", "a\n"), Some(DiffShape::Removed));
    assert_eq!(shape("a\n", "b\n"), Some(DiffShape::Changed));
    assert_eq!(shape("a\n", "a\n"), None);
}

#[test]
fn a_disabled_shape_does_not_notify() -> TestResult {
    let c = conditions(json!({"notify_on": {"removed": false}}))?;
    let v = verdict(&c, "a\nb\n", "a\n")?;
    assert!(!v.notify);
    assert_eq!(v.reason, VerdictReason::ShapeDisabled(DiffShape::Removed));
    assert!(verdict(&c, "a\n", "a\nb\n")?.notify);
    Ok(())
}

#[test]
fn a_trigger_must_match_an_added_line() -> TestResult {
    let c = conditions(json!({"trigger_text": ["SALE", "/^price: \\d+$/"]}))?;
    let v = verdict(&c, "x\n", "x\nbig sale today\nprice: 10\nother\n")?;
    assert!(v.notify);
    assert_eq!(v.triggered_text, ["big sale today", "price: 10"]);
    assert_eq!(v.reason, VerdictReason::Matched);
    Ok(())
}

#[test]
fn a_trigger_ignores_removed_lines() -> TestResult {
    let c = conditions(json!({"trigger_text": ["sale"]}))?;
    let v = verdict(&c, "sale on\nother\n", "other\n")?;
    assert!(!v.notify);
    assert_eq!(v.reason, VerdictReason::NoTriggerMatched);
    Ok(())
}

#[test]
fn no_trigger_match_does_not_notify() -> TestResult {
    let c = conditions(json!({"trigger_text": ["sale"]}))?;
    let v = verdict(&c, "a\n", "a\nb\n")?;
    assert_eq!(
        (v.notify, v.reason),
        (false, VerdictReason::NoTriggerMatched)
    );
    assert!(v.triggered_text.is_empty());
    Ok(())
}

#[test]
fn block_wins_over_trigger() -> TestResult {
    let c = conditions(json!({"trigger_text": ["sale"], "block_text": ["/sold out/i"]}))?;
    let v = verdict(&c, "x\n", "x\nSale: SOLD OUT\n")?;
    assert!(!v.notify);
    assert_eq!(v.reason, VerdictReason::Blocked("/sold out/i".into()));
    Ok(())
}

#[test]
fn block_only_looks_at_added_lines() -> TestResult {
    let c = conditions(json!({"block_text": ["maintenance"]}))?;
    assert!(verdict(&c, "maintenance\nx\n", "x\ny\n")?.notify);
    Ok(())
}

#[test]
fn an_unchanged_diff_has_no_verdict_to_give() -> TestResult {
    let v = verdict(&Conditions::default(), "a\n", "a\n")?;
    assert_eq!((v.notify, v.reason), (false, VerdictReason::NoChange));
    Ok(())
}

#[test]
fn invalid_patterns_are_reported_with_their_place() -> TestResult {
    let c = conditions(json!({"trigger_text": ["ok", "/(/"], "block_text": [""]}))?;
    let message = c.compile().err().map(|e| e.to_string()).unwrap_or_default();
    assert!(
        message.starts_with("trigger_text[1]: invalid regex \"/(/\": "),
        "{message}"
    );
    let c = conditions(json!({"block_text": ["ok", ""]}))?;
    let message = c.compile().err().map(|e| e.to_string()).unwrap_or_default();
    assert_eq!(message, "block_text[1]: the pattern is empty");
    Ok(())
}

#[test]
fn reasons_read_as_sentences() {
    assert_eq!(VerdictReason::Matched.to_string(), "matched");
    assert_eq!(
        VerdictReason::ShapeDisabled(DiffShape::Added).to_string(),
        "notify_on.added is off"
    );
    assert_eq!(
        VerdictReason::Blocked("x".into()).to_string(),
        "an added line matches block_text \"x\""
    );
}

#[test]
fn the_default_scope_is_left_out_of_the_stored_json() -> TestResult {
    assert_eq!(
        serde_json::to_value(Conditions::default())?.get("scope"),
        None
    );
    let c = conditions(json!({"scope": "text"}))?;
    assert_eq!(c.scope, Scope::Text);
    assert_eq!(
        serde_json::to_value(&c)?,
        json!({
            "trigger_text": [],
            "block_text": [],
            "notify_on": {"added": true, "removed": true, "changed": true},
            "scope": "text"
        })
    );
    assert!(conditions(json!({"scope": "removed"})).is_err());
    Ok(())
}

#[test]
fn text_scope_triggers_on_a_line_that_did_not_change() -> TestResult {
    let old = "Out of stock\nPrice: 10\n";
    let new = "Out of stock\nPrice: 11\n";
    let added = conditions(json!({"trigger_text": ["out of stock"]}))?;
    let v = verdict(&added, old, new)?;
    assert_eq!(
        (v.notify, v.reason),
        (false, VerdictReason::NoTriggerMatched)
    );

    let text = conditions(json!({"trigger_text": ["out of stock"], "scope": "text"}))?;
    let v = verdict(&text, old, new)?;
    assert!(v.notify);
    assert_eq!(v.triggered_text, ["Out of stock"]);
    Ok(())
}

#[test]
fn text_scope_ignores_removed_lines() -> TestResult {
    let c = conditions(json!({"trigger_text": ["sale"], "scope": "text"}))?;
    let v = verdict(&c, "sale on\nother\n", "other\nmore\n")?;
    assert_eq!(
        (v.notify, v.reason),
        (false, VerdictReason::NoTriggerMatchedInText)
    );
    Ok(())
}

#[test]
fn text_scope_blocks_on_any_line_of_the_new_text() -> TestResult {
    let c = conditions(json!({"block_text": ["maintenance"], "scope": "text"}))?;
    let v = verdict(&c, "maintenance\nx\n", "maintenance\nx\ny\n")?;
    assert!(!v.notify);
    assert_eq!(v.reason, VerdictReason::BlockedInText("maintenance".into()));
    // The line is gone from the new text, so it no longer blocks.
    assert!(verdict(&c, "maintenance\nx\n", "x\ny\n")?.notify);
    Ok(())
}

#[test]
fn text_scope_still_honours_notify_on() -> TestResult {
    let c = conditions(json!({"scope": "text", "notify_on": {"removed": false}}))?;
    let v = verdict(&c, "a\nb\n", "a\n")?;
    assert_eq!(v.reason, VerdictReason::ShapeDisabled(DiffShape::Removed));
    Ok(())
}

#[test]
fn text_scope_reasons_read_as_sentences() {
    assert_eq!(
        VerdictReason::NoTriggerMatchedInText.to_string(),
        "no line of the text matches trigger_text"
    );
    assert_eq!(
        VerdictReason::BlockedInText("x".into()).to_string(),
        "a line of the text matches block_text \"x\""
    );
}
