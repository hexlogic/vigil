//! urlwatch's `filter` key: a list of filter names, each with an optional argument, turned into
//! a processor and steps.

use regex::RegexBuilder;
use serde_json::Value;
use vigil_core::pattern::Pattern;
use vigil_core::{Processor, SelectMode, Step};

use crate::report::Note;
use crate::steps::{reason, validated};

/// One filter of a job: its name and its argument, a string or a mapping.
struct Call {
    name: String,
    arg: Option<Value>,
}

/// The filters of a job. A list holds names, `name: argument` pairs and mappings; the older
/// form is one string, `css:div,html2text`, whose filters are separated by commas.
fn calls(filter: &Value) -> Vec<Call> {
    let split = |text: &str| -> Call {
        match text.split_once(':') {
            Some((name, arg)) => Call {
                name: name.trim().to_owned(),
                arg: Some(Value::String(arg.trim().to_owned())),
            },
            None => Call {
                name: text.trim().to_owned(),
                arg: None,
            },
        }
    };
    match filter {
        Value::String(text) => text
            .split(',')
            .map(split)
            .filter(|c| !c.name.is_empty())
            .collect(),
        Value::Array(items) => items
            .iter()
            .flat_map(|item| match item {
                Value::String(text) => vec![split(text)],
                Value::Object(map) => map
                    .iter()
                    .map(|(name, arg)| Call {
                        name: name.clone(),
                        arg: Some(arg.clone()),
                    })
                    .collect(),
                _ => Vec::new(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// An argument that is a string, or the value of `key` in a mapping.
fn arg_text<'a>(arg: Option<&'a Value>, keys: &[&str]) -> Option<&'a str> {
    match arg? {
        Value::String(text) => Some(text.as_str()),
        Value::Object(map) => keys.iter().find_map(|key| map.get(*key)?.as_str()),
        _ => None,
    }
}

/// The options of a mapping argument that this importer does not carry over, by name.
fn extra_options(arg: Option<&Value>, used: &[&str]) -> Vec<String> {
    match arg {
        Some(Value::Object(map)) => map
            .keys()
            .filter(|key| !used.contains(&key.as_str()))
            .cloned()
            .collect(),
        _ => Vec::new(),
    }
}

/// A replacement as urlwatch (Python) writes it, as the `regex` crate reads it: `\1` and
/// `\g<name>` become `${1}` and `${name}`, and a `$` is kept as one.
fn replacement(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '$' => out.push_str("$$"),
            '\\' => match chars.peek().copied() {
                Some(d) if d.is_ascii_digit() => {
                    chars.next();
                    out.push_str(&format!("${{{d}}}"));
                }
                Some('g') => {
                    chars.next();
                    let mut name = String::new();
                    if chars.peek() == Some(&'<') {
                        chars.next();
                        for n in chars.by_ref() {
                            if n == '>' {
                                break;
                            }
                            name.push(n);
                        }
                    }
                    out.push_str(&format!("${{{name}}}"));
                }
                Some('n') => {
                    chars.next();
                    out.push('\n');
                }
                Some('\\') => {
                    chars.next();
                    out.push('\\');
                }
                _ => out.push('\\'),
            },
            other => out.push(other),
        }
    }
    out
}

fn groups(regex: &str) -> Result<usize, String> {
    RegexBuilder::new(regex)
        .size_limit(1 << 20)
        .build()
        .map(|re| re.captures_len() - 1)
        .map_err(|error| reason(&error.to_string()))
}

/// What the filters of a job became.
pub(super) struct Mapped {
    pub(super) processor: Processor,
    pub(super) pipeline: Vec<Step>,
    pub(super) notes: Vec<Note>,
}

pub(super) fn map_filters(filter: Option<&Value>) -> Mapped {
    let mut notes = Vec::new();
    let mut steps = Vec::new();
    let mut processor = Processor::Text;
    for call in filter.map(calls).unwrap_or_default() {
        let arg = call.arg.as_ref();
        let name = call.name.as_str();
        let mut note_extra = |used: &[&str]| {
            let extra = extra_options(arg, used);
            if !extra.is_empty() {
                notes.push(Note::skipped(
                    name,
                    format!(
                        "the option(s) {} of the {name} filter have no equivalent",
                        extra.join(", ")
                    ),
                ));
            }
        };
        match name {
            "css" | "xpath" => {
                let keys: &[&str] = if name == "css" {
                    &["selector"]
                } else {
                    &["path"]
                };
                let Some(expr) = arg_text(arg, keys) else {
                    notes.push(Note::skipped(
                        name,
                        format!("a {name} filter without a selector was left out"),
                    ));
                    continue;
                };
                note_extra(&["selector", "path", "exclude"]);
                let select = |expr: String, mode| {
                    if name == "css" {
                        Step::Css {
                            selector: expr,
                            mode,
                            required: false,
                        }
                    } else {
                        Step::XPath {
                            expr,
                            mode,
                            required: false,
                        }
                    }
                };
                steps.push(select(expr.to_owned(), SelectMode::Include));
                if let Some(exclude) = arg_text(arg.and_then(|a| a.get("exclude")), &[]) {
                    steps.push(select(exclude.to_owned(), SelectMode::Remove));
                }
            }
            "element-by-id" | "element-by-class" | "element-by-tag" => {
                let Some(value) = arg_text(arg, &["value"]) else {
                    notes.push(Note::skipped(
                        name,
                        format!("a {name} filter without a value was left out"),
                    ));
                    continue;
                };
                let selector = match name {
                    "element-by-id" => format!("#{value}"),
                    "element-by-class" => format!(
                        ".{}",
                        value.split_whitespace().collect::<Vec<_>>().join(".")
                    ),
                    _ => value.to_owned(),
                };
                steps.push(Step::Css {
                    selector,
                    mode: SelectMode::Include,
                    required: false,
                });
            }
            "html2text" | "beautify" => {}
            "strip" | "striplines" => steps.push(Step::Trim {}),
            "remove-duplicate-lines" => steps.push(Step::Unique {}),
            "sort" => {
                note_extra(&[]);
                steps.push(Step::Sort {});
            }
            "grep" | "grepi" => {
                let Some(re) = arg_text(arg, &["re", "pattern"]) else {
                    notes.push(Note::skipped(
                        name,
                        format!("a {name} filter without a pattern was left out"),
                    ));
                    continue;
                };
                // urlwatch's `grep` keeps the lines that match; `grepi` removes them.
                if name == "grep" {
                    match groups(&format!("(?m)^.*(?:{re}).*$")) {
                        Ok(_) => steps.push(Step::RegexExtract {
                            pattern: format!("(?m)^.*(?:{re}).*$"),
                            group: 0,
                            required: false,
                        }),
                        Err(why) => notes.push(Note::skipped(
                            name,
                            format!("the pattern does not work here: {why}"),
                        )),
                    }
                } else {
                    let text = format!("/{re}/");
                    match Pattern::parse(&text) {
                        Ok(_) => steps.push(Step::IgnoreLines {
                            patterns: vec![text],
                        }),
                        Err(error) => notes.push(Note::skipped(
                            name,
                            format!(
                                "the pattern does not work here: {}",
                                reason(&error.to_string())
                            ),
                        )),
                    }
                }
            }
            "re.sub" => {
                let Some(pattern) = arg_text(arg, &["pattern"]) else {
                    notes.push(Note::skipped(
                        name,
                        "a re.sub filter without a pattern was left out",
                    ));
                    continue;
                };
                let repl = arg
                    .and_then(|a| a.get("repl"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                steps.push(Step::RegexReplace {
                    pattern: pattern.to_owned(),
                    replacement: replacement(repl),
                });
            }
            "re.findall" => {
                let Some(pattern) = arg_text(arg, &["pattern"]) else {
                    notes.push(Note::skipped(
                        name,
                        "a re.findall filter without a pattern was left out",
                    ));
                    continue;
                };
                if arg.and_then(|a| a.get("repl")).is_some() {
                    notes.push(Note::skipped(
                        name,
                        "the replacement of re.findall has no equivalent",
                    ));
                }
                match groups(pattern) {
                    Ok(count) => steps.push(Step::RegexExtract {
                        pattern: pattern.to_owned(),
                        group: usize::from(count > 0),
                        required: false,
                    }),
                    Err(why) => notes.push(Note::skipped(
                        name,
                        format!("the pattern does not work here: {why}"),
                    )),
                }
            }
            "jq" => {
                processor = Processor::Json;
                let program = arg_text(arg, &["query"]).unwrap_or(".");
                steps.push(Step::Jq {
                    program: program.to_owned(),
                    raw: false,
                });
                notes.push(Note::approximated(name, "the result is a JSON list here, where urlwatch printed one JSON value per line"));
            }
            "format-json" => processor = Processor::Json,
            other => notes.push(Note::skipped(
                other,
                format!("the {other} filter has no equivalent and was left out"),
            )),
        }
    }
    let pipeline = validated(processor, steps, &mut notes);
    Mapped {
        processor,
        pipeline,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vigil_core::{Processor, SelectMode, Step};

    use super::{map_filters, replacement};

    #[test]
    fn replacements_are_written_for_the_regex_crate() {
        assert_eq!(
            replacement(r"\1-\g<name> costs $5\n"),
            "${1}-${name} costs $$5\n"
        );
        assert_eq!(replacement(r"a\\b"), "a\\b");
    }

    #[test]
    fn a_list_and_the_older_string_form_mean_the_same() {
        let list = map_filters(Some(&json!([{"css": "#price"}, "html2text", "strip"])));
        let string = map_filters(Some(&json!("css:#price,html2text,strip")));
        let want = [
            Step::Css {
                selector: "#price".into(),
                mode: SelectMode::Include,
                required: false,
            },
            Step::Trim {},
        ];
        assert_eq!(list.pipeline, want);
        assert_eq!(string.pipeline, want);
        assert!(list.notes.is_empty() && string.notes.is_empty());
    }

    #[test]
    fn each_filter_is_mapped_or_reported() {
        let mapped = map_filters(Some(&json!([
            {"css": {"selector": ".a", "exclude": ".b", "maxitems": 3}},
            {"element-by-id": "main"},
            {"grep": "price"},
            {"grepi": "advert"},
            {"re.sub": {"pattern": "(\\d+)", "repl": "<\\1>"}},
            "remove-duplicate-lines",
            "pdf2text",
            "shellpipe"
        ])));
        assert_eq!(mapped.processor, Processor::Text);
        // `element-by-id` after `css` is still an HTML step, but the text steps before the
        // later ones put them out of order: some are dropped and said.
        assert!(!mapped.pipeline.is_empty());
        let said: Vec<&str> = mapped.notes.iter().map(|n| n.option.as_str()).collect();
        assert!(
            said.contains(&"css") && said.contains(&"pdf2text") && said.contains(&"shellpipe"),
            "{said:?}"
        );
    }

    #[test]
    fn jq_and_format_json_make_the_page_json() {
        let mapped = map_filters(Some(&json!([{"jq": ".items[]"}])));
        assert_eq!(mapped.processor, Processor::Json);
        assert_eq!(
            mapped.pipeline,
            [Step::Jq {
                program: ".items[]".into(),
                raw: false
            }]
        );
        assert_eq!(
            map_filters(Some(&json!(["format-json"]))).processor,
            Processor::Json
        );
    }
}
