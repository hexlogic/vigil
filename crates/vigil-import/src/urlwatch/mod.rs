//! A urlwatch `urls.yaml`: one or more YAML documents, each a job.
//!
//! Only jobs with a `url` can be carried over; jobs that run a command or drive a browser are
//! skipped and reported. urlwatch has no interval per job and keeps notification settings in
//! its own configuration file, so watches get the interval of `--every` and no targets.

mod filters;

use std::collections::BTreeMap;

use serde_json::Value;
use vigil_core::{Method, Schedule};
use vigil_fetch::check_url;
use vigil_store::{ImportTag, ImportWatch, NewWatch};

use crate::builder::{Builder, Existing};
use crate::report::Note;
use crate::text::short_url;
use crate::urls::{TAG_NAME_LEN, header_is_valid};
use crate::{Format, ImportError, ImportPlan, Options};
use filters::map_filters;

/// Options of a job that have no equivalent here, with the words to say so.
const NO_EQUIVALENT: &[(&str, &str)] = &[
    ("http_proxy", "a proxy for this job has no equivalent yet"),
    ("https_proxy", "a proxy for this job has no equivalent yet"),
    (
        "ssl_no_verify",
        "turning off certificate checks has no equivalent",
    ),
    (
        "ignore_cached",
        "ignoring the server's cache validators has no equivalent",
    ),
    (
        "encoding",
        "forcing a character set has no equivalent; it is read from the page",
    ),
    (
        "max_tries",
        "ignoring the first failures has no equivalent; failures back off instead",
    ),
    (
        "ignore_connection_errors",
        "ignoring errors has no equivalent; failures back off instead",
    ),
    (
        "ignore_http_error_codes",
        "ignoring error statuses has no equivalent",
    ),
    (
        "ignore_timeout_errors",
        "ignoring errors has no equivalent; failures back off instead",
    ),
    (
        "ignore_too_many_redirects",
        "ignoring errors has no equivalent; failures back off instead",
    ),
    (
        "ignore_incomplete_reads",
        "ignoring errors has no equivalent; failures back off instead",
    ),
    (
        "treat_new_as_changed",
        "reporting a new job as changed has no equivalent; the first check is a baseline",
    ),
    (
        "compared_versions",
        "comparing several versions has no equivalent",
    ),
    ("diff_tool", "a custom diff tool has no equivalent"),
    (
        "diff_filter",
        "filters on the difference have no equivalent",
    ),
    (
        "user_visible_url",
        "a different address to show has no equivalent",
    ),
];

fn invalid(reason: impl Into<String>) -> ImportError {
    ImportError::Invalid {
        format: Format::Urlwatch,
        reason: reason.into(),
    }
}

/// A scalar as the text a header or form field holds.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn is_set(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::Bool(false)) && value != &Value::String(String::new())
}

pub(crate) fn plan(
    bytes: &[u8],
    options: &Options,
    existing: &Existing,
) -> Result<ImportPlan, ImportError> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("the file is not UTF-8 text"))?;
    let documents: Vec<Value> = serde_saphyr::from_multiple(text).map_err(|error| {
        invalid(format!(
            "the YAML cannot be read: {}",
            crate::steps::reason(&error.to_string())
        ))
    })?;
    let mut builder = Builder::new(Format::Urlwatch, options, existing);
    builder.note(Note::info(
        "interval",
        format!(
            "urlwatch has no interval per job, so every watch is checked every {}",
            humantime::format_duration(options.every)
        ),
    ));
    builder.note(Note::info(
        "notifications",
        "urlwatch keeps its notification settings outside urls.yaml, so no notification targets were imported",
    ));
    let every = options.every.as_secs();
    for (index, job) in documents.iter().enumerate() {
        // An empty document, as `---` followed by `---`, is nothing.
        let Some(job) = job.as_object() else {
            if !job.is_null() {
                builder.room()?;
                builder.skip(
                    format!("job {}", index + 1),
                    vec![Note::skipped("job", "this entry is not a job")],
                );
            }
            continue;
        };
        builder.room()?;
        let url = job
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        let name = job
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|n| !n.is_empty());
        let source = name.map_or_else(
            || {
                if url.is_empty() {
                    format!("job {}", index + 1)
                } else {
                    short_url(url)
                }
            },
            str::to_owned,
        );
        if url.is_empty() {
            let why = if job.contains_key("navigate") {
                "a job that drives a browser has no equivalent"
            } else if job.contains_key("command") {
                "a job that runs a command has no equivalent"
            } else {
                "the job has no url"
            };
            builder.skip(source, vec![Note::skipped("kind", why)]);
            continue;
        }
        if check_url(url).is_err() {
            builder.skip(
                source,
                vec![Note::skipped(
                    "url",
                    "only http and https addresses can be watched",
                )],
            );
            continue;
        }
        let mut notes = Vec::new();
        let mapped = map_filters(job.get("filter"));
        notes.extend(mapped.notes);

        let mut settings = NewWatch::new(url, Schedule::Interval(every));
        settings.name = source.clone();
        settings.processor = mapped.processor;
        settings.pipeline = mapped.pipeline;

        // The request.
        let mut headers: BTreeMap<String, String> = job
            .get("headers")
            .and_then(Value::as_object)
            .map(|h| {
                h.iter()
                    .filter_map(|(k, v)| Some((k.clone(), scalar(v)?)))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(cookies) = job.get("cookies").and_then(Value::as_object) {
            let joined: Vec<String> = cookies
                .iter()
                .filter_map(|(k, v)| Some(format!("{k}={}", scalar(v)?)))
                .collect();
            if !joined.is_empty() {
                headers.insert("Cookie".to_owned(), joined.join("; "));
            }
        }
        let method = job.get("method").and_then(Value::as_str).unwrap_or("GET");
        match method.parse::<Method>() {
            Ok(method) => settings.fetch.method = method,
            Err(_) => notes.push(Note::skipped(
                "method",
                "this request method is not supported; GET is used",
            )),
        }
        if let Some(data) = job.get("data").filter(|d| is_set(d)) {
            let json = job.get("json").and_then(Value::as_bool).unwrap_or(false);
            let body = match data {
                Value::String(text) => Some(text.clone()),
                Value::Object(map) if json => serde_json::to_string(map).ok(),
                Value::Object(map) => {
                    // urlwatch (requests) sends a mapping as a form.
                    let mut form = url_form(map);
                    if !headers
                        .keys()
                        .any(|k| k.eq_ignore_ascii_case("content-type"))
                    {
                        headers.insert(
                            "Content-Type".to_owned(),
                            "application/x-www-form-urlencoded".to_owned(),
                        );
                    }
                    Some(std::mem::take(&mut form))
                }
                _ => None,
            };
            if json
                && !headers
                    .keys()
                    .any(|k| k.eq_ignore_ascii_case("content-type"))
            {
                headers.insert("Content-Type".to_owned(), "application/json".to_owned());
            }
            if settings.fetch.method == Method::Get {
                // urlwatch sends data with POST when there is no method.
                settings.fetch.method = Method::Post;
                notes.push(Note::info("data", "the job has data and no method, so it is requested with POST, as urlwatch does"));
            }
            settings.fetch.body = body;
        }
        for (name, value) in headers {
            if header_is_valid(&name, &value) {
                settings.fetch.headers.insert(name, value);
            } else {
                notes.push(Note::skipped(
                    "headers",
                    format!("the header {name:?} cannot be sent and was left out"),
                ));
            }
        }
        if let Some(timeout) = job.get("timeout").and_then(Value::as_u64)
            && (1..=3600).contains(&timeout)
        {
            settings.fetch.timeout_secs = Some(timeout);
        }
        for (key, why) in NO_EQUIVALENT {
            if job.get(*key).is_some_and(is_set) {
                notes.push(Note::skipped(key, *why));
            }
        }

        // Tags, and whether the job is on.
        let mut tags = Vec::new();
        let tag_values: Vec<&str> = match job.get("tags") {
            Some(Value::String(text)) => vec![text.as_str()],
            Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        for tag in tag_values
            .into_iter()
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
        let paused = job.get("enabled").and_then(Value::as_bool) == Some(false);
        builder.add(
            source,
            ImportWatch {
                settings,
                paused,
                tags,
                target_urls: Vec::new(),
                template: None,
            },
            notes,
        );
    }
    Ok(builder.finish())
}

/// A mapping as an `application/x-www-form-urlencoded` body.
fn url_form(map: &serde_json::Map<String, Value>) -> String {
    let mut out = String::new();
    for (key, value) in map {
        let Some(value) = scalar(value) else { continue };
        if !out.is_empty() {
            out.push('&');
        }
        out.push_str(&encode(key));
        out.push('=');
        out.push_str(&encode(&value));
    }
    out
}

fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte))
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
