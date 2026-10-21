//! Compiling [`Conditions`] and judging a change with them.

use super::{
    CompiledConditions, Conditions, ConditionsError, DiffShape, Scope, Verdict, VerdictReason,
};
use crate::diff::{ChangeKind, LineDiff};
use crate::pattern::Pattern;

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
            scope: self.scope,
        })
    }
}

impl CompiledConditions {
    /// Decide whether the change described by `diff` should notify.
    ///
    /// In order: a kind of change that is switched off does not notify; a line that matches a
    /// block pattern does not notify; with trigger patterns set, at least one line must match
    /// one. Which lines count depends on the scope: with [`Scope::Added`] only the lines the
    /// change added, so a change that only removes lines never satisfies a trigger; with
    /// [`Scope::Text`] every line of the new text.
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
        let candidates: Vec<&str> = match self.scope {
            Scope::Added => diff.added().collect(),
            Scope::Text => diff
                .lines()
                .iter()
                .filter(|line| line.kind != ChangeKind::Removed)
                .map(|line| line.text.as_str())
                .collect(),
        };
        let text = self.scope == Scope::Text;
        for line in &candidates {
            if let Some(pattern) = self.block.iter().find(|p| p.is_match(line)) {
                let pattern = pattern.as_str().to_owned();
                return no(if text {
                    VerdictReason::BlockedInText(pattern)
                } else {
                    VerdictReason::Blocked(pattern)
                });
            }
        }
        let triggered_text: Vec<String> = if self.trigger.is_empty() {
            Vec::new()
        } else {
            candidates
                .iter()
                .filter(|line| self.trigger.iter().any(|p| p.is_match(line)))
                .map(|line| (*line).to_owned())
                .collect()
        };
        if !self.trigger.is_empty() && triggered_text.is_empty() {
            return no(if text {
                VerdictReason::NoTriggerMatchedInText
            } else {
                VerdictReason::NoTriggerMatched
            });
        }
        Verdict {
            notify: true,
            reason: VerdictReason::Matched,
            triggered_text,
        }
    }
}
