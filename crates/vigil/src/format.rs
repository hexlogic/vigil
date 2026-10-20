//! Plain-text rendering helpers for CLI output.

use std::io::IsTerminal;
use std::time::{Duration, UNIX_EPOCH};

/// Color only when writing to a terminal and `NO_COLOR` is not set.
pub fn wants_color() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// "5m ago", "3h ago", or "never" when there is no time.
pub fn ago(at_ms: Option<i64>, now_ms: i64) -> String {
    let Some(at_ms) = at_ms else {
        return "never".into();
    };
    let secs = (now_ms.saturating_sub(at_ms) / 1000).max(0);
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// A unix-millisecond time as RFC 3339 in UTC, e.g. `2026-10-20T17:21:51Z`.
pub fn timestamp(ms: i64) -> String {
    let time = UNIX_EPOCH + Duration::from_millis(u64::try_from(ms).unwrap_or(0));
    humantime::format_rfc3339_seconds(time).to_string()
}

/// Left-align `rows` into columns separated by two spaces. Trailing spaces are trimmed.
pub fn render_table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for row in rows {
        let line: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(c, cell)| format!("{cell:<width$}", width = widths[c]))
            .collect();
        out.push_str(line.join("  ").trim_end());
        out.push('\n');
    }
    out
}

const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const CYAN: &str = "\x1b[36m";
const RESET: &str = "\x1b[0m";

/// Color a unified diff: additions green, removals red, hunk headers cyan.
pub fn colorize(diff: &str) -> String {
    let mut out = String::with_capacity(diff.len() + 64);
    for line in diff.lines() {
        let color = if line.starts_with("+++") || line.starts_with("---") {
            None
        } else if line.starts_with('+') {
            Some(GREEN)
        } else if line.starts_with('-') {
            Some(RED)
        } else if line.starts_with("@@") {
            Some(CYAN)
        } else {
            None
        };
        match color {
            Some(color) => {
                out.push_str(color);
                out.push_str(line);
                out.push_str(RESET);
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_picks_the_largest_unit() {
        let now = 10_000_000_000;
        assert_eq!(ago(None, now), "never");
        assert_eq!(ago(Some(now - 5_000), now), "5s ago");
        assert_eq!(ago(Some(now - 59_000), now), "59s ago");
        assert_eq!(ago(Some(now - 60_000), now), "1m ago");
        assert_eq!(ago(Some(now - 3_599_000), now), "59m ago");
        assert_eq!(ago(Some(now - 3_600_000), now), "1h ago");
        assert_eq!(ago(Some(now - 86_400_000), now), "1d ago");
    }

    #[test]
    fn ago_never_goes_negative() {
        assert_eq!(ago(Some(2_000), 1_000), "0s ago");
    }

    #[test]
    fn timestamp_is_rfc3339_utc() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp(1_000_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(timestamp(-5), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn table_aligns_columns_and_trims_trailing_space() {
        let rows = vec![
            vec!["ID".to_owned(), "STATUS".to_owned()],
            vec!["abcdef".to_owned(), "ok".to_owned()],
        ];
        assert_eq!(render_table(&rows), "ID      STATUS\nabcdef  ok\n");
    }

    #[test]
    fn table_handles_no_rows() {
        assert_eq!(render_table(&[]), "");
    }

    #[test]
    fn colorize_marks_changes_but_not_file_headers() {
        let diff = "--- a\n+++ b\n@@ -1,1 +1,1 @@\n-old\n+new\n same\n";
        let colored = colorize(diff);
        assert!(colored.starts_with("--- a\n+++ b\n"));
        assert!(colored.contains("\x1b[36m@@ -1,1 +1,1 @@\x1b[0m\n"));
        assert!(colored.contains("\x1b[31m-old\x1b[0m\n"));
        assert!(colored.contains("\x1b[32m+new\x1b[0m\n"));
        assert!(colored.ends_with(" same\n"));
    }
}
