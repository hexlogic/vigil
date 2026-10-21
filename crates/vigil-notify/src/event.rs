//! The facts about a change that a notification is built from.

use std::time::{Duration, UNIX_EPOCH};

/// What happened to a watch, as the variables a message template can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// The watch id.
    pub watch_id: String,
    /// The watch's display name.
    pub watch_name: String,
    /// The watched URL.
    pub watch_url: String,
    /// Lines that were added.
    pub added: Vec<String>,
    /// Lines that were removed.
    pub removed: Vec<String>,
    /// The change as a unified diff.
    pub diff: String,
    /// The page text after the change.
    pub snapshot: String,
    /// The added lines that matched the watch's trigger patterns.
    pub triggered: Vec<String>,
    /// When the check ran, in unix milliseconds.
    pub checked_at_ms: i64,
    /// The address Vigil is served at, or empty if not configured.
    pub base_url: String,
    /// Link to the diff page, or empty if there is no base URL.
    pub diff_url: String,
    /// Whether this is a test notification rather than a change.
    pub is_test: bool,
}

/// The title of a message whose template sets none.
pub(crate) fn default_title(watch_name: &str, is_test: bool) -> String {
    if is_test {
        watch_name.to_owned()
    } else {
        format!("{watch_name} changed")
    }
}

impl Event {
    /// When the check ran, as an RFC 3339 UTC timestamp.
    pub fn checked_at_rfc3339(&self) -> String {
        let millis = u64::try_from(self.checked_at_ms).unwrap_or(0);
        humantime::format_rfc3339_seconds(UNIX_EPOCH + Duration::from_millis(millis)).to_string()
    }

    /// A made-up event, for checking templates and for test notifications.
    pub fn sample() -> Self {
        Self {
            watch_id: "0192a3b4-c5d6-7e8f-9012-3456789abcde".into(),
            watch_name: "Example product page".into(),
            watch_url: "https://example.com/products/widget".into(),
            added: vec!["Widget: $12".into()],
            removed: vec!["Widget: $10".into()],
            diff: "@@ -1,2 +1,2 @@\n Example product page\n-Widget: $10\n+Widget: $12\n".into(),
            snapshot: "Example product page\nWidget: $12\n".into(),
            triggered: vec!["Widget: $12".into()],
            checked_at_ms: 1_767_225_600_000,
            base_url: "https://vigil.example.com".into(),
            diff_url: "https://vigil.example.com/watch/0192a3b4-c5d6-7e8f-9012-3456789abcde/diff?from=a&to=b"
                .into(),
            is_test: false,
        }
    }

    /// The event behind a test notification. It says plainly that it is a test, and links to
    /// Vigil when its address is known.
    pub fn test(base_url: Option<&str>) -> Self {
        let line = "This is a test notification from Vigil. If you can read it, this target works.";
        let checked_at_ms = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .unwrap_or(0);
        Self {
            watch_id: "00000000-0000-0000-0000-000000000000".into(),
            watch_name: "Vigil test notification".into(),
            watch_url: base_url.unwrap_or_default().into(),
            added: vec![line.into()],
            removed: Vec::new(),
            diff: format!("+{line}\n"),
            snapshot: format!("{line}\n"),
            triggered: Vec::new(),
            checked_at_ms,
            base_url: base_url.unwrap_or_default().into(),
            diff_url: String::new(),
            is_test: true,
        }
    }
}
