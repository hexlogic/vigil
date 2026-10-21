//! A JSON export of Distill: `{"data": [{"uri", "name", "tags", "config"}]}`, where `config`
//! is itself a JSON string that holds the selections.
//!
//! This reads the parts of the format that matter for a page watch: the address, the name, the
//! tags and the includes and excludes of the selections. The format was described to the
//! author but no export was at hand, so anything else in it is reported rather than guessed
//! at, and `--dry-run` shows what was understood.

use serde_json::Value;
use vigil_core::{Processor, Schedule, SelectMode, Step};
use vigil_fetch::check_url;
use vigil_store::{ImportTag, ImportWatch, NewWatch};

use crate::builder::{Builder, Existing};
use crate::report::Note;
use crate::steps::validated;
use crate::text::short_url;
use crate::urls::TAG_NAME_LEN;
use crate::{Format, ImportError, ImportPlan, Options};

fn invalid(reason: &str) -> ImportError {
    ImportError::Invalid {
        format: Format::Distill,
        reason: reason.to_owned(),
    }
}

/// The `{type, expr}` entries under `key` of every frame of every selection.
fn entries<'a>(config: &'a Value, key: &str) -> Vec<&'a Value> {
    config
        .get("selections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|selection| {
            selection
                .get("frames")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .flat_map(|frame| {
            frame
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .collect()
}

fn step(entry: &Value, mode: SelectMode, notes: &mut Vec<Note>) -> Option<Step> {
    let expr = entry
        .get("expr")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|e| !e.is_empty())?;
    match entry.get("type").and_then(Value::as_str).unwrap_or("css") {
        "xpath" => Some(Step::XPath {
            expr: expr.to_owned(),
            mode,
            required: false,
        }),
        "css" => Some(Step::Css {
            selector: expr.to_owned(),
            mode,
            required: false,
        }),
        other => {
            notes.push(Note::skipped(
                "selections",
                format!("a selection of type {other:?} has no equivalent and was left out"),
            ));
            None
        }
    }
}

pub(crate) fn plan(
    bytes: &[u8],
    options: &Options,
    existing: &Existing,
) -> Result<ImportPlan, ImportError> {
    let root: Value = serde_json::from_slice(bytes).map_err(|_| invalid("the file is not JSON"))?;
    let items = root
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("there is no data list"))?;
    let mut builder = Builder::new(Format::Distill, options, existing);
    builder.note(Note::info(
        "schedule",
        format!(
            "the schedule of each monitor was not read, so every watch is checked every {}",
            humantime::format_duration(options.every)
        ),
    ));
    let every = options.every.as_secs();
    for (index, item) in items.iter().enumerate() {
        builder.room()?;
        let url = item
            .get("uri")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|n| !n.is_empty());
        let source = name.map_or_else(
            || {
                if url.is_empty() {
                    format!("monitor {}", index + 1)
                } else {
                    short_url(url)
                }
            },
            str::to_owned,
        );
        if url.is_empty() || check_url(url).is_err() {
            builder.skip(
                source,
                vec![Note::skipped(
                    "uri",
                    "only http and https addresses can be watched",
                )],
            );
            continue;
        }
        let mut notes = Vec::new();
        let config: Value = match item.get("config") {
            Some(Value::String(text)) => serde_json::from_str(text).unwrap_or_else(|_| {
                notes.push(Note::skipped(
                    "config",
                    "the selections could not be read, so the whole page is watched",
                ));
                Value::Null
            }),
            Some(other) => other.clone(),
            None => Value::Null,
        };
        let mut steps = Vec::new();
        let (mut css, mut xpath) = (Vec::new(), Vec::new());
        for entry in entries(&config, "includes") {
            match step(entry, SelectMode::Include, &mut notes) {
                Some(Step::Css { selector, .. }) => css.push(selector),
                Some(Step::XPath { expr, .. }) => xpath.push(expr),
                _ => {}
            }
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
        if !css.is_empty() && !xpath.is_empty() {
            notes.push(Note::skipped(
                "selections",
                "XPath selections were left out because the monitor also has CSS selections",
            ));
        }
        for entry in entries(&config, "excludes") {
            if let Some(remove) = step(entry, SelectMode::Remove, &mut notes) {
                steps.push(remove);
            }
        }
        let pipeline = validated(Processor::Text, steps, &mut notes);

        let mut tags = Vec::new();
        for tag in item
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            if tag.chars().count() > TAG_NAME_LEN {
                notes.push(Note::skipped(
                    "tags",
                    format!("a tag name over {TAG_NAME_LEN} characters was left out"),
                ));
                continue;
            }
            builder.define_tag(ImportTag {
                name: tag.to_owned(),
                default_interval_secs: None,
                target_urls: Vec::new(),
            });
            if !tags.iter().any(|t| t == tag) {
                tags.push(tag.to_owned());
            }
        }
        let mut settings = NewWatch::new(url, Schedule::Interval(every));
        settings.name = source.clone();
        settings.pipeline = pipeline;
        builder.add(
            source,
            ImportWatch {
                settings,
                paused: false,
                tags,
                target_urls: Vec::new(),
                template: None,
            },
            notes,
        );
    }
    Ok(builder.finish())
}
