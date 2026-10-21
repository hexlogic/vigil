//! One watch of a backup, turned into a Vigil watch and a list of what did not carry over.

use std::collections::{BTreeSet, HashMap};

use serde_json::Value;
use vigil_core::{Conditions, FetchSettings, Method, NotifyOn, Schedule, Scope};
use vigil_fetch::check_url;
use vigil_store::{ImportTag, ImportTarget, ImportWatch, NewWatch};

use super::filters::{Filters, map_filters, pattern};
use super::settings::{Globals, flag, is_known, seconds, strings, text};
use super::template::translate;
use crate::builder::{Builder, check_target, target_name};
use crate::report::Note;
use crate::text::short_url;
use crate::urls::TAG_NAME_LEN;

/// The shortest and longest interval a watch can have, in seconds.
const MIN_SECS: u64 = 1;
const MAX_SECS: u64 = 365 * 24 * 3600;

/// How the importer treats the timeout of a source whose default is not Vigil's.
const VIGIL_TIMEOUT_SECS: u64 = 30;

/// What became of one watch.
pub(super) enum Mapped {
    Made {
        source: String,
        watch: Box<ImportWatch>,
        notes: Vec<Note>,
    },
    Skipped {
        source: String,
        notes: Vec<Note>,
    },
}

/// The lists a watch has, with those of its tags and of the whole backup added.
fn merged(watch: &Value, tags: &[&Value], key: &str, extra: &[String]) -> Vec<String> {
    let mut out = strings(watch.get(key));
    for tag in tags {
        out.extend(strings(tag.get(key)));
    }
    out.extend(extra.iter().cloned());
    out
}

fn uses_browser(backend: &str) -> bool {
    backend.starts_with("html_webdriver") || backend.starts_with("extra_browser")
}

/// Which kinds of change notify. The source asks for lines that were added, removed or
/// replaced; Vigil classifies a change as only added, only removed, or both.
fn notify_on(watch: &Value, notes: &mut Vec<Note>) -> NotifyOn {
    let added = flag(watch.get("filter_text_added"), true);
    let removed = flag(watch.get("filter_text_removed"), true);
    let replaced = flag(watch.get("filter_text_replaced"), true);
    if added && removed && replaced {
        return NotifyOn::default();
    }
    notes.push(Note::approximated(
        "filter_text_added",
        "the kinds of change to notify about were mapped to the nearest: a change that both adds and removes lines notifies if any wanted kind is on",
    ));
    NotifyOn {
        added,
        removed,
        changed: added || removed || replaced,
    }
}

pub(super) fn map_watch(
    watch: &Value,
    tags: &HashMap<String, Value>,
    globals: &Globals,
    builder: &mut Builder<'_>,
    unknown: &mut BTreeSet<String>,
) -> Mapped {
    let url = text(watch.get("url")).unwrap_or_default().to_owned();
    let source = text(watch.get("title"))
        .or_else(|| text(watch.get("page_title")))
        .map_or_else(|| short_url(&url), str::to_owned);
    let mut notes = Vec::new();
    let skip = |source: String, mut notes: Vec<Note>, option: &str, why: &str| {
        notes.push(Note::skipped(option, why));
        Mapped::Skipped { source, notes }
    };
    if url.is_empty() {
        return skip(source, notes, "url", "the watch has no address");
    }
    if check_url(&url).is_err() {
        return skip(
            source,
            notes,
            "url",
            "only http and https addresses can be watched",
        );
    }
    let kind = text(watch.get("processor")).unwrap_or("text_json_diff");
    let restock = kind == "restock_diff";
    match kind {
        "text_json_diff" | "restock_diff" => {}
        "image_ssim_diff" => {
            return skip(
                source,
                notes,
                "processor",
                "watching a page as an image has no equivalent",
            );
        }
        _ => {
            return skip(
                source,
                notes,
                "processor",
                "this kind of watch has no equivalent",
            );
        }
    }
    for key in watch.as_object().into_iter().flat_map(|o| o.keys()) {
        if !is_known(key) {
            unknown.insert(key.clone());
        }
    }

    // Tags, and the filters they add to their watches.
    let mut tag_values = Vec::new();
    let mut tag_names = Vec::new();
    for id in strings(watch.get("tags")) {
        let Some(tag) = tags.get(&id.to_ascii_lowercase()).or_else(|| tags.get(&id)) else {
            continue;
        };
        let Some(name) = text(tag.get("title")) else {
            continue;
        };
        if name.chars().count() > TAG_NAME_LEN {
            notes.push(Note::skipped(
                "tags",
                format!("a tag name over {TAG_NAME_LEN} characters was left out"),
            ));
            continue;
        }
        let mut target_urls = Vec::new();
        for url in strings(tag.get("notification_urls")) {
            match check_target(&url) {
                Ok(()) => {
                    builder.define_target(ImportTarget {
                        name: target_name(&url),
                        url: url.clone(),
                    });
                    target_urls.push(url);
                }
                Err(note) => notes.push(note),
            }
        }
        builder.define_tag(ImportTag {
            name: name.to_owned(),
            default_interval_secs: None,
            target_urls,
        });
        tag_names.push(name.to_owned());
        tag_values.push(tag);
    }

    // The text.
    let backend = text(watch.get("fetch_backend")).unwrap_or("system");
    let filters = Filters {
        include: merged(watch, &tag_values, "include_filters", &[]),
        subtractive: merged(
            watch,
            &tag_values,
            "subtractive_selectors",
            &globals.subtractive,
        ),
        ignore: merged(watch, &tag_values, "ignore_text", &globals.ignore_text),
        extract: merged(watch, &tag_values, "extract_text", &[]),
        extract_lines: merged(watch, &tag_values, "extract_lines_containing", &[]),
        trim: flag(watch.get("trim_text_whitespace"), false),
        dedupe: flag(watch.get("remove_duplicate_lines"), false),
        sort: flag(watch.get("sort_text_alphabetically"), false),
        json_hint: text(watch.get("content-type")).is_some_and(|t| t.contains("json")),
    };
    let mapped = map_filters(&filters);
    notes.extend(mapped.notes);

    // When the notifications fire.
    let mut conditions = Conditions::default();
    for (key, option, list) in [
        ("trigger_text", "trigger_text", &mut conditions.trigger_text),
        (
            "text_should_not_be_present",
            "text_should_not_be_present",
            &mut conditions.block_text,
        ),
    ] {
        for entry in merged(watch, &tag_values, key, &[]) {
            if let Some(p) = pattern(&entry, option, &mut notes)
                && !list.contains(&p)
            {
                list.push(p);
            }
        }
    }
    if !conditions.trigger_text.is_empty() || !conditions.block_text.is_empty() {
        // The source looks for these in the whole text, not only in what was added.
        conditions.scope = Scope::Text;
    }
    conditions.notify_on = notify_on(watch, &mut notes);
    if let Some(rules) = watch.get("conditions").and_then(Value::as_array)
        && rules
            .iter()
            .any(|rule| text(rule.get("field")).is_some_and(|field| field != "None"))
    {
        notes.push(Note::skipped(
            "conditions",
            "rules on the page's contents have no equivalent",
        ));
    }

    // The schedule.
    let own = if flag(watch.get("time_between_check_use_default"), true) {
        None
    } else {
        Some(seconds(watch.get("time_between_check"))).filter(|s| *s > 0)
    };
    let mut every = own
        .or(globals.interval)
        .unwrap_or_else(|| builder.options().every.as_secs());
    if !(MIN_SECS..=MAX_SECS).contains(&every) {
        every = every.clamp(MIN_SECS, MAX_SECS);
        notes.push(Note::approximated(
            "time_between_check",
            "the interval was outside what Vigil accepts and was brought into range",
        ));
    }
    if watch
        .pointer("/time_schedule_limit/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        notes.push(Note::skipped(
            "time_schedule_limit",
            "checking only on some days or hours has no equivalent",
        ));
    }

    // The request.
    let mut fetch = FetchSettings::default();
    let method = text(watch.get("method")).unwrap_or("GET");
    match method.parse::<Method>() {
        Ok(method) => fetch.method = method,
        Err(_) => notes.push(Note::skipped(
            "method",
            "this request method is not supported; GET is used",
        )),
    }
    if let Some(body) = text(watch.get("body")) {
        if fetch.method == Method::Get {
            notes.push(Note::skipped(
                "body",
                "a request body was left out because the method is GET",
            ));
        } else {
            fetch.body = Some(body.to_owned());
        }
    }
    let mut headers = globals.headers.clone();
    if let Some(own) = watch.get("headers").and_then(Value::as_object) {
        for (name, value) in own {
            if let Some(value) = value.as_str() {
                headers.insert(name.clone(), value.to_owned());
            }
        }
    }
    for (name, value) in headers {
        if crate::backup::header_is_valid(&name, &value) {
            fetch.headers.insert(name, value);
        } else {
            // Only the name: values often hold tokens.
            notes.push(Note::skipped(
                "headers",
                format!("the header {name:?} cannot be sent and was left out"),
            ));
        }
    }
    fetch.user_agent = globals.user_agent.clone().filter(|_| {
        !fetch
            .headers
            .keys()
            .any(|k| k.eq_ignore_ascii_case("user-agent"))
    });
    fetch.timeout_secs = globals
        .timeout
        .filter(|t| *t != VIGIL_TIMEOUT_SECS && (1..=3600).contains(t));
    if text(watch.get("proxy")).is_some() {
        notes.push(Note::skipped(
            "proxy",
            "a proxy for this watch has no equivalent yet",
        ));
    }
    if uses_browser(backend) {
        notes.push(Note::approximated("fetch_backend", "the watch used a browser; it is fetched over plain HTTP, so pages that need scripts will look different"));
    }
    if text(watch.get("webdriver_js_execute_code")).is_some() {
        notes.push(Note::skipped(
            "webdriver_js_execute_code",
            "running scripts in the page has no equivalent",
        ));
    }
    if flag(watch.get("ignore_status_codes"), false) {
        notes.push(Note::skipped(
            "ignore_status_codes",
            "pages that answer with an error status are still reported as errors",
        ));
    }
    let mut paused = flag(watch.get("paused"), false) || globals.all_paused;
    let steps = watch
        .get("browser_steps")
        .and_then(Value::as_array)
        .is_some_and(|steps| {
            steps.iter().any(|step| {
                text(step.get("operation")).is_some_and(|op| op != "Choose one" && op != "Nothing")
            })
        });
    if steps {
        paused = true;
        notes.push(Note::skipped(
            "browser_steps",
            "steps in a browser (clicking, typing) have no equivalent; the watch was made paused",
        ));
    }
    if restock {
        paused = true;
        notes.push(Note::approximated("processor", "price and stock tracking has no equivalent; the page is watched as text, and the watch was made paused"));
    }
    if flag(watch.get("check_unique_lines"), false) {
        notes.push(Note::skipped(
            "check_unique_lines",
            "reporting only lines never seen before has no equivalent",
        ));
    }
    if text(watch.get("link_to_open")).is_some() {
        notes.push(Note::skipped(
            "link_to_open",
            "a different address to open from the list has no equivalent",
        ));
    }
    if text(watch.get("llm_intent")).is_some() || text(watch.get("llm_change_summary")).is_some() {
        notes.push(Note::skipped(
            "llm_intent",
            "AI summaries and checks have no equivalent",
        ));
    }
    if flag(watch.get("notification_screenshot"), false) {
        notes.push(Note::skipped(
            "notification_screenshot",
            "screenshots in notifications have no equivalent",
        ));
    }

    // Where notifications go.
    let muted = flag(watch.get("notification_muted"), false) || globals.all_muted;
    let own_urls = strings(watch.get("notification_urls"));
    let urls = if own_urls.is_empty() {
        &globals.notification_urls
    } else {
        &own_urls
    };
    let mut target_urls = Vec::new();
    if muted {
        if !urls.is_empty() {
            notes.push(Note::approximated("notification_muted", "notifications were muted in the source, so the watch is not linked to any notification target"));
        }
    } else {
        for url in urls {
            match check_target(url) {
                Ok(()) => {
                    builder.define_target(ImportTarget {
                        name: target_name(url),
                        url: url.clone(),
                    });
                    if !target_urls.contains(url) {
                        target_urls.push(url.clone());
                    }
                }
                Err(note) => notes.push(note),
            }
        }
    }
    let template = translate(
        text(watch.get("notification_title")),
        text(watch.get("notification_body")),
        &mut notes,
    );

    let mut settings = NewWatch::new(&url, Schedule::Interval(every));
    settings.name = source.clone();
    settings.jitter_secs = globals.jitter;
    settings.fetch = fetch;
    settings.processor = mapped.processor;
    settings.pipeline = mapped.pipeline;
    settings.conditions = conditions;
    settings.snapshot_retention = watch
        .get("history_snapshot_max_length")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
        .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
        .or(globals.retention)
        .unwrap_or(settings.snapshot_retention);
    Mapped::Made {
        source,
        watch: Box::new(ImportWatch {
            settings,
            paused,
            tags: tag_names,
            target_urls,
            template,
        }),
        notes,
    }
}
