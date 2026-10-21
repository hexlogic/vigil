//! A watch's filters: what to select, what to remove and how to tidy the text, as the source
//! spells them, turned into a processor and a pipeline.
//!
//! The source keeps one list of "include" filters in which the first characters say what each
//! is (`xpath:`, `json:`, `jq:`, a leading `/`, anything else is CSS). Vigil has one step per
//! kind. Steps that Vigil cannot build (a Python-only regular expression, an XPath 3
//! expression) are dropped one by one with a note, not the whole watch.

use regex::RegexBuilder;
use vigil_core::pattern::Pattern;
use vigil_core::{Pipeline, Processor, SelectMode, Step};

use crate::report::Note;

/// The most characters of a library message that go into a note.
const REASON_CHARS: usize = 120;

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
struct Written<'a> {
    /// The regular expression, or the text.
    body: &'a str,
    /// The flags after the closing slash; `None` for plain text.
    flags: Option<&'a str>,
}

fn written(entry: &str) -> Written<'_> {
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
/// source that mean nothing here (`g`) are dropped quietly; the others are noted.
fn flags_of(flags: &str, notes: &mut Vec<Note>, option: &str) -> String {
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

/// An entry of `extract_text` as a raw regular expression with its flags written inline.
fn extract_regex(entry: &str, option: &str, notes: &mut Vec<Note>) -> String {
    let w = written(entry);
    match w.flags {
        Some(flags) => format!("(?{}){}", flags_of(flags, notes, option), w.body),
        None => format!("(?i){}", regex::escape(w.body)),
    }
}

fn reason(message: &str) -> String {
    let line = message.lines().next().unwrap_or_default();
    line.chars().take(REASON_CHARS).collect()
}

/// Number of capture groups of a regular expression, or the reason it does not build.
fn groups(regex: &str) -> Result<usize, String> {
    RegexBuilder::new(regex)
        .size_limit(1 << 20)
        .build()
        .map(|re| re.captures_len() - 1)
        .map_err(|error| reason(&error.to_string()))
}

fn dedupe(list: &[String]) -> Vec<String> {
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

/// `extract_text`: each match, or its capture group, on a line of its own.
fn extract_step(entries: &[String], notes: &mut Vec<Note>) -> Option<Step> {
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
fn lines_step(entries: &[String], notes: &mut Vec<Note>) -> Option<Step> {
    let mut alternatives = Vec::new();
    for entry in dedupe(entries) {
        let w = written(&entry);
        let regex = match w.flags {
            Some(flags) => format!(
                "(?{}){}",
                flags_of(flags, notes, "extract_lines_containing"),
                w.body
            ),
            None => format!("(?i){}", regex::escape(w.body)),
        };
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

/// Keep the steps Vigil can build, dropping each that cannot, with a note.
fn validated(processor: Processor, steps: Vec<Step>, notes: &mut Vec<Note>) -> Vec<Step> {
    let mut kept: Vec<Step> = Vec::new();
    for step in steps {
        kept.push(step);
        if let Err(error) = Pipeline::new(processor, &kept)
            && let Some(step) = kept.pop()
        {
            notes.push(Note::skipped(
                step.name(),
                format!(
                    "the {} filter was left out because it does not work here: {}",
                    step.name(),
                    reason(&error.to_string())
                ),
            ));
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::{Filters, map_filters, pattern};
    use vigil_core::{Processor, SelectMode, Step};

    fn pat(entry: &str) -> (Option<String>, usize) {
        let mut notes = Vec::new();
        (pattern(entry, "ignore_text", &mut notes), notes.len())
    }

    #[test]
    fn a_regular_expression_without_flags_is_case_insensitive_as_in_the_source() {
        assert_eq!(pat("/price: \\d+/").0.as_deref(), Some("/price: \\d+/i"));
        assert_eq!(pat("/Sold Out/s").0.as_deref(), Some("/Sold Out/s"));
        assert_eq!(pat("/x/gi").0.as_deref(), Some("/x/i"));
        assert_eq!(pat("plain text").0.as_deref(), Some("plain text"));
        // A flag that Vigil does not have is dropped, and said.
        assert_eq!(pat("/x/a"), (Some("/x/".to_owned()), 1));
        // A Python-only construct does not build here: the pattern is dropped, and said.
        assert_eq!(pat("/(?<=a)b/"), (None, 1));
    }

    #[test]
    fn includes_are_told_apart_by_their_first_characters() {
        let filters = Filters {
            include: vec![".price".into(), "div.x".into()],
            subtractive: vec![".ad".into(), "xpath://footer".into()],
            ..Filters::default()
        };
        let mapped = map_filters(&filters);
        assert_eq!(mapped.processor, Processor::Text);
        assert_eq!(
            mapped.pipeline,
            [
                Step::Css {
                    selector: ".price, div.x".into(),
                    mode: SelectMode::Include,
                    required: false
                },
                Step::Css {
                    selector: ".ad".into(),
                    mode: SelectMode::Remove,
                    required: false
                },
                Step::XPath {
                    expr: "//footer".into(),
                    mode: SelectMode::Remove,
                    required: false
                },
            ]
        );
        assert_eq!(mapped.notes.len(), 1, "{:?}", mapped.notes);

        let json = map_filters(&Filters {
            include: vec!["jqraw:.data[].title".into()],
            ..Filters::default()
        });
        assert_eq!(json.processor, Processor::Json);
        assert_eq!(
            json.pipeline,
            [Step::Jq {
                program: ".data[].title".into(),
                raw: true
            }]
        );
        assert!(json.notes.is_empty());
    }

    #[test]
    fn a_filter_that_does_not_build_is_dropped_alone() {
        let mapped = map_filters(&Filters {
            include: vec!["a[".into()],
            trim: true,
            ..Filters::default()
        });
        assert_eq!(mapped.pipeline, [Step::Trim {}]);
        assert_eq!(mapped.notes.len(), 1);
        assert!(
            mapped.notes[0].message.contains("left out"),
            "{:?}",
            mapped.notes
        );
    }

    #[test]
    fn extracts_are_one_pattern_with_the_groups_the_source_would_return() {
        let one = map_filters(&Filters {
            extract: vec!["/Price: (\\d+)/".into()],
            ..Filters::default()
        });
        assert_eq!(
            one.pipeline,
            [Step::RegexExtract {
                pattern: "(?i)Price: (\\d+)".into(),
                group: 1,
                required: false
            }]
        );
        let plain = map_filters(&Filters {
            extract: vec!["a.b".into(), "/c+/".into()],
            ..Filters::default()
        });
        assert_eq!(
            plain.pipeline,
            [Step::RegexExtract {
                pattern: "(?:(?i)a\\.b)|(?:(?i)c+)".into(),
                group: 0,
                required: false
            }]
        );
        let mixed = map_filters(&Filters {
            extract: vec!["/(a)/".into(), "/b/".into()],
            ..Filters::default()
        });
        assert_eq!(mixed.pipeline.len(), 1);
        assert_eq!(mixed.notes.len(), 1);
    }
}
