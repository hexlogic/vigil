//! The filters of the form.

use vigil_core::{SelectMode, Step};

use super::super::draft::StepDraft;

/// The lines of a text area, without blanks.
pub(super) fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A filter from its fields; the key of the problem if it lacks one.
pub(super) fn step_of(step: &StepDraft) -> Result<Step, &'static str> {
    let need = |text: &str| -> Result<String, &'static str> {
        if text.trim().is_empty() {
            Err("form.err.step_empty")
        } else {
            Ok(text.trim().to_owned())
        }
    };
    let mode = if step.mode == "remove" {
        SelectMode::Remove
    } else {
        SelectMode::Include
    };
    Ok(match step.kind.as_str() {
        "css" => Step::Css {
            selector: need(&step.selector)?,
            mode,
            required: step.required,
        },
        "xpath" => Step::XPath {
            expr: need(&step.expr)?,
            mode,
            required: step.required,
        },
        "jsonpath" => Step::JsonPath {
            expr: need(&step.expr)?,
            required: step.required,
        },
        "jq" => Step::Jq {
            program: need(&step.program)?,
            raw: step.raw,
        },
        "regex_extract" => Step::RegexExtract {
            pattern: need(&step.pattern)?,
            group: match step.group.trim() {
                "" => 0,
                text => text.parse().map_err(|_| "form.err.step_group")?,
            },
            required: step.required,
        },
        "regex_replace" => Step::RegexReplace {
            pattern: need(&step.pattern)?,
            // An empty replacement deletes the matches, which is a fine thing to want.
            replacement: step.replacement.clone(),
        },
        "ignore_lines" => {
            let patterns = lines(&step.patterns);
            if patterns.is_empty() {
                return Err("form.err.step_empty");
            }
            Step::IgnoreLines { patterns }
        }
        "sort" => Step::Sort {},
        "unique" => Step::Unique {},
        "trim" => Step::Trim {},
        "collapse_whitespace" => Step::CollapseWhitespace {},
        "lowercase" => Step::Lowercase {},
        _ => return Err("form.err.step_kind"),
    })
}
