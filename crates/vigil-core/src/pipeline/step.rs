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
    /// Select parts of an HTML page with a CSS selector, before the page is turned into text.
    /// Only works with the `text` processor, and the page is parsed as HTML whatever its
    /// `Content-Type` says.
    Css {
        /// A CSS selector.
        selector: String,
        /// Keep only the selected elements, or remove them.
        #[serde(default)]
        mode: SelectMode,
        /// Fail the check when nothing is selected, instead of treating that as an empty page.
        #[serde(default, skip_serializing_if = "is_false")]
        required: bool,
    },
    /// Select parts of an HTML page with an XPath 1.0 expression, before the page is turned
    /// into text. Works like `css`, and an `include` expression may also select text nodes,
    /// attribute values (`//a/@href`) or compute a value (`count(//li)`), which is kept as
    /// text; `remove` can only delete elements and text nodes.
    #[serde(rename = "xpath")]
    XPath {
        /// An XPath 1.0 expression.
        expr: String,
        /// Keep only the selected nodes, or remove them.
        #[serde(default)]
        mode: SelectMode,
        /// Fail the check when nothing is selected, instead of treating that as an empty page.
        #[serde(default, skip_serializing_if = "is_false")]
        required: bool,
    },
    /// Select values from JSON with a JSONPath query (RFC 9535). The result is always an array
    /// of the matches, so its shape does not depend on how many there are, and finding
    /// nothing gives `[]` unless `required` is set. Only works with the `json` processor.
    #[serde(rename = "jsonpath")]
    JsonPath {
        /// A JSONPath query such as `$.items[*].price`.
        expr: String,
        /// Fail the check when nothing matches, instead of storing an empty array.
        #[serde(default, skip_serializing_if = "is_false")]
        required: bool,
    },
    /// Transform the JSON with a jq program (the jaq implementation). The outputs are
    /// collected into an array, so a program that yields one value and one that yields
    /// several give the same shape; no output gives `[]`. Only works with the `json`
    /// processor. `env`, `now`, `input`, `halt` and other filters that read the world or
    /// stop the program are not available, so the same input always gives the same output.
    ///
    /// With `raw`, the outputs are printed as lines of text instead of as a JSON array, the
    /// way `jq -r` does: a string is written without its quotes and anything else as compact
    /// JSON. The result is text and not JSON any more, so a raw step must be the last of the
    /// JSON steps.
    Jq {
        /// A jq program such as `.items | map(.price)`.
        program: String,
        /// Print the outputs as lines of text.
        #[serde(default, skip_serializing_if = "is_false")]
        raw: bool,
    },
    /// Keep only what a regex matches, one match per line. With `group` set, keep that
    /// capture group of each match instead of the whole match. Finding nothing leaves empty
    /// text unless `required` is set.
    RegexExtract {
        /// Regular expression syntax of the `regex` crate; use `(?i)` for flags.
        pattern: String,
        /// Capture group to keep; 0 is the whole match.
        #[serde(default)]
        group: usize,
        /// Fail the check when nothing matches, instead of producing empty text.
        #[serde(default, skip_serializing_if = "is_false")]
        required: bool,
    },
    /// Replace every match of a regex. `$1` and `${name}` in the replacement refer to
    /// capture groups and `$$` is a literal dollar sign. A name runs as far as letters,
    /// digits and underscores go, so text right after a group needs braces: `${1}USD`, not
    /// `$1USD`. A name the pattern does not have is refused.
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

/// What a selection step does with the elements it finds.
///
/// `include` keeps only the selected elements; `remove` deletes them and keeps the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelectMode {
    /// Keep only the selected elements.
    #[default]
    Include,
    /// Delete the selected elements.
    Remove,
}

/// What a step works on. Steps must be listed in this order: HTML, then JSON, then text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Stage {
    Html,
    Json,
    Text,
}

impl Stage {
    /// Word used in error messages.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Json => "json",
            Self::Text => "text",
        }
    }
}

fn is_false(value: &bool) -> bool {
    !value
}

impl Step {
    pub(super) fn stage(&self) -> Stage {
        match self {
            Self::Css { .. } | Self::XPath { .. } => Stage::Html,
            Self::JsonPath { .. } | Self::Jq { .. } => Stage::Json,
            _ => Stage::Text,
        }
    }

    /// The step's `type` as written in the stored JSON, used to name it in error messages.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Css { .. } => "css",
            Self::XPath { .. } => "xpath",
            Self::JsonPath { .. } => "jsonpath",
            Self::Jq { .. } => "jq",
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

    use super::{SelectMode, Step};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Every variant with the exact JSON it is stored as.
    fn shapes() -> Vec<(Step, Value)> {
        vec![
            (
                Step::Css {
                    selector: "div.price > span".into(),
                    mode: SelectMode::Remove,
                    required: false,
                },
                json!({"type": "css", "selector": "div.price > span", "mode": "remove"}),
            ),
            (
                Step::Css {
                    selector: "main".into(),
                    mode: SelectMode::Include,
                    required: true,
                },
                json!({"type": "css", "selector": "main", "mode": "include", "required": true}),
            ),
            (
                Step::XPath {
                    expr: "//a/@href".into(),
                    mode: SelectMode::Include,
                    required: false,
                },
                json!({"type": "xpath", "expr": "//a/@href", "mode": "include"}),
            ),
            (
                Step::XPath {
                    expr: "//a".into(),
                    mode: SelectMode::Remove,
                    required: true,
                },
                json!({"type": "xpath", "expr": "//a", "mode": "remove", "required": true}),
            ),
            (
                Step::JsonPath {
                    expr: "$.items[*].price".into(),
                    required: false,
                },
                json!({"type": "jsonpath", "expr": "$.items[*].price"}),
            ),
            (
                Step::JsonPath {
                    expr: "$.items[*]".into(),
                    required: true,
                },
                json!({"type": "jsonpath", "expr": "$.items[*]", "required": true}),
            ),
            (
                Step::Jq {
                    program: ".items | map(.price)".into(),
                    raw: false,
                },
                json!({"type": "jq", "program": ".items | map(.price)"}),
            ),
            (
                Step::Jq {
                    program: ".items[].name".into(),
                    raw: true,
                },
                json!({"type": "jq", "program": ".items[].name", "raw": true}),
            ),
            (
                Step::RegexExtract {
                    pattern: "a(b)".into(),
                    group: 1,
                    required: false,
                },
                json!({"type": "regex_extract", "pattern": "a(b)", "group": 1}),
            ),
            (
                Step::RegexExtract {
                    pattern: "a".into(),
                    group: 0,
                    required: true,
                },
                json!({"type": "regex_extract", "pattern": "a", "group": 0, "required": true}),
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
                group: 0,
                required: false
            }
        );
        Ok(())
    }

    #[test]
    fn the_mode_defaults_to_include() -> TestResult {
        let step: Step = serde_json::from_value(json!({"type": "css", "selector": "p"}))?;
        assert_eq!(
            step,
            Step::Css {
                selector: "p".into(),
                mode: SelectMode::Include,
                required: false
            }
        );
        Ok(())
    }

    #[test]
    fn selection_steps_do_not_require_a_match_unless_asked() -> TestResult {
        for shape in [
            json!({"type": "css", "selector": "p"}),
            json!({"type": "xpath", "expr": "//p"}),
            json!({"type": "jsonpath", "expr": "$.a"}),
            json!({"type": "regex_extract", "pattern": "a"}),
        ] {
            let step: Step = serde_json::from_value(shape.clone())?;
            // Defaults are not written back, so stored configs keep their shape.
            assert_eq!(
                serde_json::to_value(&step)?["required"],
                Value::Null,
                "{shape}"
            );
            let mut asked = shape.clone();
            asked["required"] = json!(true);
            let step: Step = serde_json::from_value(asked.clone())?;
            assert_eq!(
                serde_json::to_value(&step)?["required"],
                json!(true),
                "{asked}"
            );
        }
        Ok(())
    }

    #[test]
    fn unknown_fields_are_rejected() {
        for bad in [
            json!({"type": "css", "selector": "p", "mod": "remove"}),
            json!({"type": "css", "selector": "p", "mode": "exclude"}),
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
