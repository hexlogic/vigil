//! The facts about a change that a notification is built from.

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
}

impl Event {
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
        }
    }
}
