//! A watch's filters: what to select, what to remove and how to tidy the text, as the source
//! spells them, turned into a processor and a pipeline.
//!
//! The source keeps one list of "include" filters in which the first characters say what each
//! is (`xpath:`, `json:`, `jq:`, a leading `/`, anything else is CSS). Vigil has one step per
//! kind. Steps that Vigil cannot build (a Python-only regular expression, an XPath 3
//! expression) are dropped one by one with a note, not the whole watch.

use regex::RegexBuilder;
use vigil_core::pattern::Pattern;
use vigil_core::{Processor, SelectMode, Step};

use super::extract::{extract_step, lines_step};
use crate::report::Note;
use crate::steps::{reason, validated};

/// What a watch says about its text, as lists of strings and switches.
#[derive(Default)]
pub(super) struct Filters {
    pub(super) include: Vec<String>,
    pub(super) subtractive: Vec<String>,
    pub(super) ignore: Vec<String>,
    pub(super) extract: Vec<String>,
    pub(super) extract_lines: Vec<String>,
    pub(super) trim: bool,
    pub(super) dedupe: bool,
    pub(super) sort: bool,
    /// The page was JSON when the source last saw it, though no filter says so.
    pub(super) json_hint: bool,
}

/// The processor and steps, and what was left out.
pub(super) struct Mapped {
    pub(super) processor: Processor,
    pub(super) pipeline: Vec<Step>,
    pub(super) notes: Vec<Note>,
}

enum Include {
    Css(String),
    XPath(String),
    JsonPath(String),
    Jq { program: String, raw: bool },
}

fn classify(entry: &str) -> Include {
    let entry = entry.trim();
    for prefix in ["xpath1:", "xpath:"] {
        if let Some(rest) = entry.strip_prefix(prefix) {
            return Include::XPath(rest.trim().to_owned());
        }
    }
    if let Some(rest) = entry.strip_prefix("json:") {
        return Include::JsonPath(rest.trim().to_owned());
    }
    if let Some(rest) = entry.strip_prefix("jqraw:") {
        return Include::Jq {
            program: rest.trim().to_owned(),
            raw: true,
        };
    }
    if let Some(rest) = entry.strip_prefix("jq:") {
        return Include::Jq {
            program: rest.trim().to_owned(),
            raw: false,
        };
    }
    if entry.starts_with('/') || entry.starts_with("(/") {
        return Include::XPath(entry.to_owned());
    }
    Include::Css(entry.to_owned())
}

/// A pattern as the source writes it: `/regex/flags`, or plain text.
pub(super) struct Written<'a> {
    /// The regular expression, or the text.
    pub(super) body: &'a str,
    /// The flags after the closing slash; `None` for plain text.
    pub(super) flags: Option<&'a str>,
}

pub(super) fn written(entry: &str) -> Written<'_> {
    let entry = entry.trim();
    if let Some(close) = entry.rfind('/')
        && entry.starts_with('/')
        && close > 1
        && entry[close + 1..].chars().all(|c| c.is_ascii_alphabetic())
    {
        return Written {
            body: &entry[1..close],
            flags: Some(&entry[close + 1..]),
        };
    }
    Written {
        body: entry,
        flags: None,
    }
}

/// The flags Vigil's patterns and regexes have: `i`, `m`, `s` and `x`. The source reads a
/// regular expression without flags case-insensitively, so then `i` is added. Flags of the
/// source that mean nothing here (`g`) are dropped quietly; the others are noted. Written
/// flags replace that default even when all of them are dropped, so the result can be empty.
pub(super) fn flags_of(flags: &str, notes: &mut Vec<Note>, option: &str) -> String {
    if flags.is_empty() {
        return "i".to_owned();
    }
    let mut kept = String::new();
    for flag in flags.chars() {
        match flag {
            'i' | 'm' | 's' | 'x' if !kept.contains(flag) => kept.push(flag),
            'g' | 'i' | 'm' | 's' | 'x' => {}
            other => notes.push(Note::approximated(
                option,
                format!("the regular expression flag {other:?} has no equivalent and was dropped"),
            )),
        }
    }
    kept
}

/// An entry of `ignore_text`, `trigger_text` or `block_text` as a pattern in Vigil's syntax.
/// A pattern that does not build is dropped with a note.
pub(super) fn pattern(entry: &str, option: &str, notes: &mut Vec<Note>) -> Option<String> {
    let w = written(entry);
    let text = match w.flags {
        Some(flags) => format!("/{}/{}", w.body, flags_of(flags, notes, option)),
        None => w.body.to_owned(),
    };
    match Pattern::parse(&text) {
        Ok(_) => Some(text),
        Err(error) => {
            notes.push(Note::skipped(
                option,
                format!(
                    "a pattern was left out because it does not work here: {}",
                    reason(&error.to_string())
                ),
            ));
            None
        }
    }
}

/// An entry of `extract_text` or `extract_lines_containing` as a raw regular expression with
/// its flags written inline. Flags that were written replace the default `i` even when all of
/// them are dropped, so then the expression has no flags group: `(?)` does not build.
pub(super) fn extract_regex(entry: &str, option: &str, notes: &mut Vec<Note>) -> String {
    let w = written(entry);
    match w.flags {
        Some(flags) => match flags_of(flags, notes, option) {
            kept if kept.is_empty() => w.body.to_owned(),
            kept => format!("(?{kept}){}", w.body),
        },
        None => format!("(?i){}", regex::escape(w.body)),
    }
}

/// Number of capture groups of a regular expression, or the reason it does not build.
pub(super) fn groups(regex: &str) -> Result<usize, String> {
    RegexBuilder::new(regex)
        .size_limit(1 << 20)
        .build()
        .map(|re| re.captures_len() - 1)
        .map_err(|error| reason(&error.to_string()))
}

pub(super) fn dedupe(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in list.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if !out.iter().any(|seen| seen == item) {
            out.push(item.to_owned());
        }
    }
    out
}

/// Turn a watch's filters into a processor and a pipeline.
pub(super) fn map_filters(filters: &Filters) -> Mapped {
    let mut notes = Vec::new();
    let include: Vec<Include> = dedupe(&filters.include)
        .iter()
        .map(|e| classify(e))
        .collect();
    let json = include
        .iter()
        .any(|i| matches!(i, Include::JsonPath(_) | Include::Jq { .. }));
    let processor = if json || filters.json_hint {
        Processor::Json
    } else {
        Processor::Text
    };
    if filters.json_hint && !json {
        notes.push(Note::info(
            "include_filters",
            "the page was JSON when it was last checked, so it is read as JSON",
        ));
    }
    let mut steps = Vec::new();

    if processor == Processor::Json {
        let mut html = 0;
        let mut chained = 0;
        for item in &include {
            match item {
                Include::JsonPath(expr) => {
                    steps.push(Step::JsonPath {
                        expr: expr.clone(),
                        required: false,
                    });
                    chained += 1;
                }
                Include::Jq { program, raw } => {
                    steps.push(Step::Jq {
                        program: program.clone(),
                        raw: *raw,
                    });
                    chained += 1;
                }
                Include::Css(_) | Include::XPath(_) => html += 1,
            }
        }
        if html > 0 {
            notes.push(Note::skipped(
                "include_filters",
                format!("{html} CSS or XPath filter(s) were left out: the page is JSON"),
            ));
        }
        if chained > 1 {
            notes.push(Note::approximated(
                "include_filters",
                "several JSON filters run one after another here, each on the result of the one before; the source ran each on the page and joined the results",
            ));
        }
    } else {
        let css: Vec<&str> = include
            .iter()
            .filter_map(|i| {
                if let Include::Css(s) = i {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .collect();
        let xpath: Vec<&str> = include
            .iter()
            .filter_map(|i| {
                if let Include::XPath(s) = i {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .collect();
        if !css.is_empty() && !xpath.is_empty() {
            notes.push(Note::skipped(
                "include_filters",
                "XPath filters were left out because the watch also has CSS filters, and the two cannot be combined",
            ));
        }
        if !css.is_empty() {
            steps.push(Step::Css {
                selector: css.join(", "),
                mode: SelectMode::Include,
                required: false,
            });
        } else if !xpath.is_empty() {
            steps.push(Step::XPath {
                expr: xpath.join(" | "),
                mode: SelectMode::Include,
                required: false,
            });
        }
        if css.len().max(if css.is_empty() { xpath.len() } else { 0 }) > 1 {
            notes.push(Note::approximated(
                "include_filters",
                "several selectors are joined into one, so the matches come in page order, not in the order of the filters",
            ));
        }
        let (rcss, rxpath): (Vec<&String>, Vec<&String>) = dedupe_refs(&filters.subtractive)
            .into_iter()
            .partition(|s| !matches!(classify(s), Include::XPath(_)));
        if !rcss.is_empty() {
            steps.push(Step::Css {
                selector: rcss
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                mode: SelectMode::Remove,
                required: false,
            });
        }
        if !rxpath.is_empty() {
            let exprs: Vec<String> = rxpath
                .iter()
                .map(|s| match classify(s) {
                    Include::XPath(e) => e,
                    _ => (*s).clone(),
                })
                .collect();
            steps.push(Step::XPath {
                expr: exprs.join(" | "),
                mode: SelectMode::Remove,
                required: false,
            });
        }
    }

    let ignore: Vec<String> = dedupe(&filters.ignore)
        .iter()
        .filter_map(|e| pattern(e, "ignore_text", &mut notes))
        .collect();
    if !ignore.is_empty() {
        steps.push(Step::IgnoreLines { patterns: ignore });
    }
    if let Some(step) = extract_step(&filters.extract, &mut notes) {
        steps.push(step);
    }
    if let Some(step) = lines_step(&filters.extract_lines, &mut notes) {
        steps.push(step);
    }
    if filters.trim {
        steps.push(Step::Trim {});
    }
    if filters.dedupe {
        steps.push(Step::Unique {});
    }
    if filters.sort {
        steps.push(Step::Sort {});
    }

    let pipeline = validated(processor, steps, &mut notes);
    Mapped {
        processor,
        pipeline,
        notes,
    }
}

fn dedupe_refs(list: &[String]) -> Vec<&String> {
    let mut out: Vec<&String> = Vec::new();
    for item in list.iter().filter(|s| !s.trim().is_empty()) {
        if !out.iter().any(|seen| seen.trim() == item.trim()) {
            out.push(item);
        }
    }
    out
}

#[cfg(test)]
mod tests;
