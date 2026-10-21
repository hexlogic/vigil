//! The settings every watch of a backup shares, and the helpers that read its loose JSON.

use std::collections::BTreeMap;

use serde_json::Value;

/// The message body every watch of the source has until someone changes it.
const DEFAULT_BODY: &str = "{{watch_url}} had a change.\n---\n{{diff}}\n---";

/// Seconds in the intervals a person can write in the source.
const UNITS: [(&str, u64); 5] = [
    ("weeks", 7 * 24 * 3600),
    ("days", 24 * 3600),
    ("hours", 3600),
    ("minutes", 60),
    ("seconds", 1),
];

/// The strings of a JSON list, trimmed, without empty ones.
pub(super) fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// A non-empty string.
pub(super) fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// A switch, `default` when it is missing or not one.
pub(super) fn flag(value: Option<&Value>, default: bool) -> bool {
    value.and_then(Value::as_bool).unwrap_or(default)
}

/// The seconds in `{"weeks": null, "days": 1, "hours": 2, ...}`; 0 if nothing is set.
pub(super) fn seconds(value: Option<&Value>) -> u64 {
    let Some(object) = value.and_then(Value::as_object) else {
        return 0;
    };
    UNITS
        .iter()
        .map(|(unit, each)| {
            object
                .get(*unit)
                .and_then(|v| {
                    v.as_u64()
                        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                })
                .unwrap_or(0)
                .saturating_mul(*each)
        })
        .fold(0, u64::saturating_add)
}

/// What the whole backup says that applies to every watch.
#[derive(Debug, Default)]
pub(super) struct Globals {
    /// How often to check a watch that has no interval of its own.
    pub(super) interval: Option<u64>,
    pub(super) jitter: u32,
    pub(super) headers: BTreeMap<String, String>,
    pub(super) timeout: Option<u64>,
    pub(super) user_agent: Option<String>,
    pub(super) retention: Option<u32>,
    pub(super) notification_urls: Vec<String>,
    pub(super) ignore_text: Vec<String>,
    pub(super) subtractive: Vec<String>,
    pub(super) all_paused: bool,
    pub(super) all_muted: bool,
    pub(super) has_notification_template: bool,
}

impl Globals {
    /// The settings that are copied into every watch, by name, for one note about them.
    pub(super) fn copied_settings(&self) -> Vec<&'static str> {
        let mut copied = Vec::new();
        for (set, name) in [
            (self.interval.is_some(), "the default interval"),
            (self.jitter > 0, "the random delay"),
            (self.timeout.is_some_and(|t| t != 30), "the timeout"),
            (!self.headers.is_empty(), "headers"),
            (self.retention.is_some(), "the number of versions kept"),
            (!self.ignore_text.is_empty(), "text to ignore"),
            (!self.subtractive.is_empty(), "elements to remove"),
            (
                !self.notification_urls.is_empty(),
                "notification addresses (for watches with none of their own)",
            ),
        ] {
            if set {
                copied.push(name);
            }
        }
        copied
    }

    pub(super) fn read(settings: &Value) -> Self {
        let app = settings.get("application");
        let requests = settings.get("requests");
        let interval =
            Some(seconds(requests.and_then(|r| r.get("time_between_check")))).filter(|s| *s > 0);
        let headers = settings
            .get("headers")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        let user_agent = requests
            .and_then(|r| r.pointer("/default_ua/html_requests"))
            .and_then(|v| text(Some(v)))
            .map(str::to_owned);
        // The message every watch starts with is the same in every backup; only one that
        // someone wrote is worth a word.
        let has_template = app
            .and_then(|a| a.get("notification_body"))
            .and_then(|v| text(Some(v)))
            .is_some_and(|body| body != DEFAULT_BODY);
        Self {
            interval,
            jitter: requests
                .and_then(|r| r.get("jitter_seconds"))
                .and_then(Value::as_u64)
                .map_or(0, |v| u32::try_from(v).unwrap_or(u32::MAX)),
            headers,
            timeout: requests
                .and_then(|r| r.get("timeout"))
                .and_then(Value::as_u64),
            user_agent,
            retention: app
                .and_then(|a| a.get("history_snapshot_max_length"))
                .and_then(Value::as_u64)
                .filter(|n| *n > 0)
                .map(|n| u32::try_from(n).unwrap_or(u32::MAX)),
            notification_urls: strings(app.and_then(|a| a.get("notification_urls"))),
            ignore_text: strings(app.and_then(|a| a.get("global_ignore_text"))),
            subtractive: strings(app.and_then(|a| a.get("global_subtractive_selectors"))),
            all_paused: flag(app.and_then(|a| a.get("all_paused")), false),
            all_muted: flag(app.and_then(|a| a.get("all_muted")), false),
            has_notification_template: has_template,
        }
    }
}

/// Keys of a watch that are about what the page last looked like or how the last check went.
/// They mean nothing for a new watch and are ignored without a word.
pub(super) const RUNTIME_KEYS: &[&str] = &[
    "uuid",
    "date_created",
    "last_checked",
    "last_viewed",
    "previous_md5",
    "check_count",
    "fetch_time",
    "last_error",
    "consecutive_filter_failures",
    "content-type",
    "remote_server_reply",
    "notification_alert_count",
    "use_page_title_in_list",
    "save_button",
    "last_filter_config_hash",
    "last_check_status",
    "last_notification_error",
    "browser_steps_last_error_step",
    "scheduler_timezone_default",
    "history_snapshot_max_length",
    "has_ldjson_price_data",
    "track_ldjson_price_data",
    "follow_price_changes",
    "in_stock_only",
    "price_change_threshold_percent",
    "restock",
    "page_title",
    "title",
    "tag",
    "tags",
    "conditions_match_logic",
    "filter_failure_notification_send",
    "ignore_whitespace",
    "strip_ignored_lines",
    "history_snapshot_max_length",
];

/// Keys of a watch that the importer reads.
pub(super) const READ_KEYS: &[&str] = &[
    "url",
    "paused",
    "notification_muted",
    "notification_urls",
    "notification_title",
    "notification_body",
    "notification_format",
    "notification_screenshot",
    "method",
    "body",
    "headers",
    "fetch_backend",
    "processor",
    "proxy",
    "ignore_status_codes",
    "webdriver_delay",
    "webdriver_js_execute_code",
    "browser_steps",
    "include_filters",
    "subtractive_selectors",
    "ignore_text",
    "trigger_text",
    "text_should_not_be_present",
    "extract_text",
    "extract_lines_containing",
    "sort_text_alphabetically",
    "remove_duplicate_lines",
    "trim_text_whitespace",
    "check_unique_lines",
    "filter_text_added",
    "filter_text_removed",
    "filter_text_replaced",
    "time_between_check",
    "time_between_check_use_default",
    "time_schedule_limit",
    "conditions",
    "link_to_open",
];

/// Whether a key is one the importer knows about, reads, or deliberately ignores.
pub(super) fn is_known(key: &str) -> bool {
    key.starts_with("llm_")
        || key.starts_with('_')
        || key.starts_with("processor_config_")
        || READ_KEYS.contains(&key)
        || RUNTIME_KEYS.contains(&key)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Globals, is_known, seconds};

    #[test]
    fn an_interval_is_the_sum_of_its_parts() {
        let tbc = json!({"weeks": null, "days": 1, "hours": 2, "minutes": "30", "seconds": null});
        assert_eq!(seconds(Some(&tbc)), 86_400 + 7_200 + 1_800);
        assert_eq!(seconds(Some(&json!({}))), 0);
        assert_eq!(seconds(None), 0);
    }

    #[test]
    fn the_shared_settings_are_read_from_their_places() {
        let settings = json!({
            "application": {"notification_urls": ["ntfy://h/t", " "], "all_muted": true,
                            "history_snapshot_max_length": 20, "notification_body": "mine"},
            "requests": {"time_between_check": {"hours": 3}, "jitter_seconds": 4, "timeout": 45,
                         "default_ua": {"html_requests": "UA"}},
            "headers": {"X-A": "1", "X-B": 2}
        });
        let g = Globals::read(&settings);
        assert_eq!(g.interval, Some(10_800));
        assert_eq!((g.jitter, g.timeout, g.retention), (4, Some(45), Some(20)));
        assert_eq!(g.user_agent.as_deref(), Some("UA"));
        assert_eq!(g.notification_urls, ["ntfy://h/t"]);
        assert!(g.all_muted && !g.all_paused && g.has_notification_template);
        assert_eq!(g.headers.len(), 1);
    }

    #[test]
    fn keys_the_importer_has_not_heard_of_are_found() {
        assert!(is_known("include_filters") && is_known("previous_md5") && is_known("llm_intent"));
        assert!(is_known("_llm_result") && is_known("processor_config_restock_diff"));
        assert!(!is_known("a_brand_new_option"));
    }
}
