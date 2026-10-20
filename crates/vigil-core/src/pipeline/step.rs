//! The configuration of one pipeline step, as stored with a watch.

use serde::{Deserialize, Serialize};

/// One step of a watch's pipeline.
///
/// Steps are stored as JSON objects tagged with a `type`, for example
/// `{"type": "regex_replace", "pattern": "\\d{2}:\\d{2}", "replacement": "HH:MM"}`. This shape
/// is a stable contract: the API and importers use it, so variants are only ever added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    /// Keep only what a regex matches, one match per line. With `group` set, keep that
    /// capture group of each match instead of the whole match. Finding nothing is an error.
    RegexExtract {
        /// Regular expression syntax of the `regex` crate; use `(?i)` for flags.
        pattern: String,
        /// Capture group to keep; 0 is the whole match.
        #[serde(default)]
        group: usize,
    },
    /// Replace every match of a regex. `$1` and `${name}` in the replacement refer to
    /// capture groups and `$$` is a literal dollar sign.
    RegexReplace {
        /// Regular expression syntax of the `regex` crate; use `(?i)` for flags.
        pattern: String,
        /// Replacement text.
        replacement: String,
    },
    /// Drop lines that match any of the patterns (substring or `/regex/flags`).
    IgnoreLines {
        /// Patterns in the shared pattern syntax.
        patterns: Vec<String>,
    },
    /// Sort the lines.
    Sort {},
    /// Remove repeated lines, keeping the first of each.
    Unique {},
    /// Remove whitespace from both ends of every line.
    Trim {},
    /// Turn every run of whitespace within a line into one space.
    CollapseWhitespace {},
    /// Lowercase all text.
    Lowercase {},
}

impl Step {
    /// The step's `type` as written in the stored JSON, used to name it in error messages.
    pub fn name(&self) -> &'static str {
        match self {
            Self::RegexExtract { .. } => "regex_extract",
            Self::RegexReplace { .. } => "regex_replace",
            Self::IgnoreLines { .. } => "ignore_lines",
            Self::Sort {} => "sort",
            Self::Unique {} => "unique",
            Self::Trim {} => "trim",
            Self::CollapseWhitespace {} => "collapse_whitespace",
            Self::Lowercase {} => "lowercase",
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::Step;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Every variant with the exact JSON it is stored as.
    fn shapes() -> Vec<(Step, Value)> {
        vec![
            (
                Step::RegexExtract {
                    pattern: "a(b)".into(),
                    group: 1,
                },
                json!({"type": "regex_extract", "pattern": "a(b)", "group": 1}),
            ),
            (
                Step::RegexReplace {
                    pattern: "a".into(),
                    replacement: "b".into(),
                },
                json!({"type": "regex_replace", "pattern": "a", "replacement": "b"}),
            ),
            (
                Step::IgnoreLines {
                    patterns: vec!["x".into(), "/y/i".into()],
                },
                json!({"type": "ignore_lines", "patterns": ["x", "/y/i"]}),
            ),
            (Step::Sort {}, json!({"type": "sort"})),
            (Step::Unique {}, json!({"type": "unique"})),
            (Step::Trim {}, json!({"type": "trim"})),
            (
                Step::CollapseWhitespace {},
                json!({"type": "collapse_whitespace"}),
            ),
            (Step::Lowercase {}, json!({"type": "lowercase"})),
        ]
    }

    #[test]
    fn every_step_matches_its_stored_shape() -> TestResult {
        for (step, shape) in shapes() {
            assert_eq!(serde_json::to_value(&step)?, shape, "{}", step.name());
            assert_eq!(serde_json::from_value::<Step>(shape)?, step);
        }
        Ok(())
    }

    #[test]
    fn names_are_the_stored_type_tags() -> TestResult {
        for (step, shape) in shapes() {
            assert_eq!(shape["type"], step.name());
        }
        Ok(())
    }

    #[test]
    fn the_group_defaults_to_the_whole_match() -> TestResult {
        let step: Step = serde_json::from_value(json!({"type": "regex_extract", "pattern": "a"}))?;
        assert_eq!(
            step,
            Step::RegexExtract {
                pattern: "a".into(),
                group: 0
            }
        );
        Ok(())
    }

    #[test]
    fn unknown_fields_are_rejected() {
        for bad in [
            json!({"type": "regex_extract", "pattern": "a", "grup": 1}),
            json!({"type": "sort", "reverse": true}),
            json!({"type": "ignore_lines", "patterns": [], "pattern": "x"}),
        ] {
            assert!(
                serde_json::from_value::<Step>(bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn unknown_types_and_missing_fields_are_rejected() {
        for bad in [
            json!({"type": "frobnicate"}),
            json!({"pattern": "a"}),
            json!({"type": "regex_replace", "pattern": "a"}),
        ] {
            assert!(
                serde_json::from_value::<Step>(bad.clone()).is_err(),
                "{bad}"
            );
        }
    }
}
