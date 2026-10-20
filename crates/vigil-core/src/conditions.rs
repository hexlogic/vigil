//! Conditions that decide whether a detected change is worth a notification.
//!
//! Conditions never decide whether a change is stored: every change becomes a snapshot. They
//! only produce a [`Verdict`] that the notification step reads.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::diff::LineDiff;
use crate::pattern::{Pattern, PatternError};

/// Which kinds of change should notify. All are on by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotifyOn {
    /// Notify when lines were only added.
    pub added: bool,
    /// Notify when lines were only removed.
    pub removed: bool,
    /// Notify when lines were both added and removed.
    pub changed: bool,
}

impl Default for NotifyOn {
    fn default() -> Self {
        Self {
            added: true,
            removed: true,
            changed: true,
        }
    }
}

/// A watch's notification conditions, as stored. `{}` means "notify about every change".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Conditions {
    /// Notify only if an added line matches one of these patterns (substring or
    /// `/regex/flags`). Empty means no restriction.
    pub trigger_text: Vec<String>,
    /// Never notify if an added line matches one of these patterns. Wins over `trigger_text`.
    pub block_text: Vec<String>,
    /// Which kinds of change notify.
    pub notify_on: NotifyOn,
}

/// A pattern in [`Conditions`] could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{list}[{index}]: {source}")]
pub struct ConditionsError {
    list: &'static str,
    index: usize,
    #[source]
    source: PatternError,
}

/// What a change did to the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffShape {
    /// Lines were added and none removed.
    Added,
    /// Lines were removed and none added.
    Removed,
    /// Lines were both added and removed.
    Changed,
}

impl DiffShape {
    /// The shape of `diff`, or `None` when it has no added or removed lines.
    pub fn of(diff: &LineDiff) -> Option<Self> {
        match (diff.added_count() > 0, diff.removed_count() > 0) {
            (true, false) => Some(Self::Added),
            (false, true) => Some(Self::Removed),
            (true, true) => Some(Self::Changed),
            (false, false) => None,
        }
    }
}

impl fmt::Display for DiffShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Changed => "changed",
        })
    }
}

/// Why a [`Verdict`] came out as it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerdictReason {
    /// The change passed every condition.
    Matched,
    /// The diff has no added or removed lines.
    NoChange,
    /// This kind of change is switched off in `notify_on`.
    ShapeDisabled(DiffShape),
    /// `trigger_text` is set and no added line matches it.
    NoTriggerMatched,
    /// An added line matched this `block_text` pattern.
    Blocked(String),
}

impl fmt::Display for VerdictReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Matched => f.write_str("matched"),
            Self::NoChange => f.write_str("no lines changed"),
            Self::ShapeDisabled(shape) => write!(f, "notify_on.{shape} is off"),
            Self::NoTriggerMatched => f.write_str("no added line matches trigger_text"),
            Self::Blocked(pattern) => write!(f, "an added line matches block_text {pattern:?}"),
        }
    }
}

/// Whether to notify about a change, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// True when a notification should be sent.
    pub notify: bool,
    /// Why.
    pub reason: VerdictReason,
    /// The added lines that matched `trigger_text`; empty when no trigger is set.
    pub triggered_text: Vec<String>,
}

/// [`Conditions`] with their patterns compiled.
#[derive(Debug, Clone)]
pub struct CompiledConditions {
    trigger: Vec<Pattern>,
    block: Vec<Pattern>,
    notify_on: NotifyOn,
}

impl Conditions {
    /// Compile the patterns, naming the one that is invalid.
    pub fn compile(&self) -> Result<CompiledConditions, ConditionsError> {
        let compile = |list: &'static str, patterns: &[String]| {
            patterns
                .iter()
                .enumerate()
                .map(|(index, source)| {
                    Pattern::parse(source).map_err(|source| ConditionsError {
                        list,
                        index,
                        source,
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        };
        Ok(CompiledConditions {
            trigger: compile("trigger_text", &self.trigger_text)?,
            block: compile("block_text", &self.block_text)?,
            notify_on: self.notify_on,
        })
    }
}

impl CompiledConditions {
    /// Decide whether the change described by `diff` should notify.
    ///
    /// In order: a kind of change that is switched off does not notify; an added line that
    /// matches a block pattern does not notify; with trigger patterns set, at least one added
    /// line must match one. Patterns only look at added lines, so a change that only removes
    /// lines never satisfies a trigger.
    pub fn evaluate(&self, diff: &LineDiff) -> Verdict {
        let no = |reason| Verdict {
            notify: false,
            reason,
            triggered_text: Vec::new(),
        };
        let Some(shape) = DiffShape::of(diff) else {
            return no(VerdictReason::NoChange);
        };
        let on = match shape {
            DiffShape::Added => self.notify_on.added,
            DiffShape::Removed => self.notify_on.removed,
            DiffShape::Changed => self.notify_on.changed,
        };
        if !on {
            return no(VerdictReason::ShapeDisabled(shape));
        }
        for line in diff.added() {
            if let Some(pattern) = self.block.iter().find(|p| p.is_match(line)) {
                return no(VerdictReason::Blocked(pattern.as_str().to_owned()));
            }
        }
        let triggered_text: Vec<String> = if self.trigger.is_empty() {
            Vec::new()
        } else {
            diff.added()
                .filter(|line| self.trigger.iter().any(|p| p.is_match(line)))
                .map(str::to_owned)
                .collect()
        };
        if !self.trigger.is_empty() && triggered_text.is_empty() {
            return no(VerdictReason::NoTriggerMatched);
        }
        Verdict {
            notify: true,
            reason: VerdictReason::Matched,
            triggered_text,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Conditions, DiffShape, NotifyOn, Verdict, VerdictReason};
    use crate::diff::LineDiff;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn conditions(value: serde_json::Value) -> Result<Conditions, serde_json::Error> {
        serde_json::from_value(value)
    }

    fn verdict(
        c: &Conditions,
        old: &str,
        new: &str,
    ) -> Result<Verdict, Box<dyn std::error::Error>> {
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
}
