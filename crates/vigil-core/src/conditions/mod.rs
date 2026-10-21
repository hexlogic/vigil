//! Conditions that decide whether a detected change is worth a notification.
//!
//! Conditions never decide whether a change is stored: every change becomes a snapshot. They
//! only produce a [`Verdict`] that the notification step reads.

mod eval;

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::diff::LineDiff;
use crate::pattern::{Pattern, PatternError};

/// Which kinds of change should notify. All are on by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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

/// Which lines `trigger_text` and `block_text` look at.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum Scope {
    /// Only lines the change added. This is the default.
    #[default]
    Added,
    /// Every line of the new text, changed or not.
    Text,
}

impl Scope {
    /// Whether this is the default, so the stored JSON can leave it out.
    pub fn is_default(&self) -> bool {
        *self == Self::Added
    }
}

/// A watch's notification conditions, as stored. `{}` means "notify about every change".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Conditions {
    /// Notify only if a line in `scope` matches one of these patterns (substring or
    /// `/regex/flags`). Empty means no restriction.
    pub trigger_text: Vec<String>,
    /// Never notify if a line in `scope` matches one of these patterns. Wins over
    /// `trigger_text`.
    pub block_text: Vec<String>,
    /// Which kinds of change notify.
    pub notify_on: NotifyOn,
    /// Which lines the two pattern lists look at. Left out of the stored JSON when it is the
    /// default.
    #[serde(skip_serializing_if = "Scope::is_default")]
    pub scope: Scope,
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
    /// `trigger_text` is set, its scope is the whole text, and no line of the new text
    /// matches it.
    NoTriggerMatchedInText,
    /// A line of the new text, changed or not, matched this `block_text` pattern.
    BlockedInText(String),
}

impl fmt::Display for VerdictReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Matched => f.write_str("matched"),
            Self::NoChange => f.write_str("no lines changed"),
            Self::ShapeDisabled(shape) => write!(f, "notify_on.{shape} is off"),
            Self::NoTriggerMatched => f.write_str("no added line matches trigger_text"),
            Self::Blocked(pattern) => write!(f, "an added line matches block_text {pattern:?}"),
            Self::NoTriggerMatchedInText => f.write_str("no line of the text matches trigger_text"),
            Self::BlockedInText(pattern) => {
                write!(f, "a line of the text matches block_text {pattern:?}")
            }
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
    /// The lines that matched `trigger_text`; empty when no trigger is set.
    pub triggered_text: Vec<String>,
}

/// [`Conditions`] with their patterns compiled.
#[derive(Debug, Clone)]
pub struct CompiledConditions {
    trigger: Vec<Pattern>,
    block: Vec<Pattern>,
    notify_on: NotifyOn,
    scope: Scope,
}

#[cfg(test)]
mod tests;
