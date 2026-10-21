//! `extract_text` and `extract_lines_containing`: keep only some of the text.

use vigil_core::Step;

use super::filters::{dedupe, extract_regex, groups};
use crate::report::Note;

/// `extract_text`: each match, or its capture group, on a line of its own.
pub(super) fn extract_step(entries: &[String], notes: &mut Vec<Note>) -> Option<Step> {
    let mut built: Vec<(String, usize)> = Vec::new();
    for entry in dedupe(entries) {
        let regex = extract_regex(&entry, "extract_text", notes);
        match groups(&regex) {
            Ok(count) => built.push((regex, count)),
            Err(why) => notes.push(Note::skipped(
                "extract_text",
                format!("a pattern was left out because it does not work here: {why}"),
            )),
        }
    }
    let (first, rest) = built.split_first()?;
    if rest.is_empty() {
        if first.1 > 1 {
            notes.push(Note::approximated(
                "extract_text",
                "the pattern has several capture groups and only the first is kept",
            ));
        }
        return Some(Step::RegexExtract {
            pattern: first.0.clone(),
            group: usize::from(first.1 > 0),
            required: false,
        });
    }
    if built.iter().all(|(_, count)| *count == 0) {
        let joined: Vec<String> = built.iter().map(|(r, _)| format!("(?:{r})")).collect();
        return Some(Step::RegexExtract {
            pattern: joined.join("|"),
            group: 0,
            required: false,
        });
    }
    notes.push(Note::skipped(
        "extract_text",
        format!(
            "{} pattern(s) were left out: several patterns can only be combined when none has a capture group",
            rest.len()
        ),
    ));
    Some(Step::RegexExtract {
        pattern: first.0.clone(),
        group: usize::from(first.1 > 0),
        required: false,
    })
}

/// `extract_lines_containing`: keep the lines that contain any of the entries.
pub(super) fn lines_step(entries: &[String], notes: &mut Vec<Note>) -> Option<Step> {
    let mut alternatives = Vec::new();
    for entry in dedupe(entries) {
        let regex = extract_regex(&entry, "extract_lines_containing", notes);
        match groups(&regex) {
            Ok(_) => alternatives.push(format!("(?:{regex})")),
            Err(why) => notes.push(Note::skipped(
                "extract_lines_containing",
                format!("a pattern was left out because it does not work here: {why}"),
            )),
        }
    }
    if alternatives.is_empty() {
        return None;
    }
    Some(Step::RegexExtract {
        pattern: format!("(?m)^.*(?:{}).*$", alternatives.join("|")),
        group: 0,
        required: false,
    })
}
