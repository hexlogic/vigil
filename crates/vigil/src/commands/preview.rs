//! `vigil preview <id>`.

use std::fmt::Write as _;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use vigil_fetch::Fetcher;
use vigil_scheduler::{Checker, Preview};

use super::settings::overridden;
use super::{open_store, resolve};
use crate::cli::ContentArgs;
use crate::config::Config;
use crate::format::{colorize, timestamp, wants_color};

/// Lines of unchanged context around each change.
const CONTEXT: usize = 3;

/// Fetch a watch, show what a check would store and what it would notify, and save nothing.
/// Exits with failure when the page or the settings do not work.
pub async fn run(config: &Config, id: &str, content: &ContentArgs) -> anyhow::Result<ExitCode> {
    let store = open_store(config).await?;
    let id = resolve(&store, id).await?;
    let watch = overridden(&store.get_watch(id).await?, content)?;
    let fetcher = Fetcher::new(config.fetch_config()).context("building the http client")?;
    let report = Checker::new(store, fetcher).preview(&watch).await?;

    match report.result {
        Ok(preview) => {
            print!(
                "{}",
                render(&watch.url, &preview, report.duration, wants_color())
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(message) => {
            eprintln!(
                "{}: preview failed ({}ms): {message}",
                watch.url,
                report.duration.as_millis()
            );
            Ok(ExitCode::FAILURE)
        }
    }
}

fn render(url: &str, preview: &Preview, took: Duration, color: bool) -> String {
    let mut out = format!(
        "{url}: {} {}, {} bytes ({}ms)\n",
        preview.http_status,
        preview.content_type.as_deref().unwrap_or("no content type"),
        preview.raw_size,
        took.as_millis()
    );
    out.push_str("--- output ---\n");
    if preview.text.is_empty() {
        out.push_str("(empty)\n");
    } else {
        out.push_str(&preview.text);
    }
    match (&preview.latest, &preview.diff) {
        (Some(latest), Some(diff)) => {
            let _ = writeln!(
                out,
                "--- compared with the snapshot of {} ---",
                timestamp(latest.taken_at)
            );
            if diff.is_empty() {
                out.push_str("no difference\n");
            } else {
                let unified = diff.unified(CONTEXT);
                out.push_str(&if color { colorize(&unified) } else { unified });
            }
        }
        _ => out.push_str("--- no snapshot to compare with yet ---\n"),
    }
    if let Some(verdict) = &preview.verdict {
        if verdict.notify {
            let _ = writeln!(out, "would notify ({})", verdict.reason);
            for line in &verdict.triggered_text {
                let _ = writeln!(out, "  matched: {}", line.trim());
            }
        } else {
            let _ = writeln!(out, "would not notify ({})", verdict.reason);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use vigil_core::{LineDiff, Verdict, VerdictReason};
    use vigil_store::SnapshotMeta;

    use super::*;

    fn preview(text: &str, latest: Option<&str>, verdict: Option<Verdict>) -> Preview {
        Preview {
            text: text.to_owned(),
            http_status: 200,
            content_type: Some("text/html".into()),
            raw_size: 42,
            latest: latest.map(|_| SnapshotMeta {
                id: uuid_nil(),
                watch_id: "00000000-0000-0000-0000-000000000001"
                    .parse()
                    .expect("valid id"),
                taken_at: 1_000_000_000_000,
                hash: vigil_core::ContentHash::of("x"),
                raw_size: 1,
                http_status: Some(200),
                content_type: None,
            }),
            diff: latest.map(|old| LineDiff::between(old, text)),
            verdict,
        }
    }

    fn uuid_nil() -> vigil_store::SnapshotId {
        vigil_store::SnapshotId::nil()
    }

    fn lines(text: &str) -> Vec<&str> {
        text.lines().collect()
    }

    #[test]
    fn a_first_preview_has_nothing_to_compare_with() {
        let out = render(
            "https://x.example",
            &preview("hello\n", None, None),
            Duration::from_millis(7),
            false,
        );
        assert_eq!(
            lines(&out),
            [
                "https://x.example: 200 text/html, 42 bytes (7ms)",
                "--- output ---",
                "hello",
                "--- no snapshot to compare with yet ---"
            ]
        );
    }

    #[test]
    fn a_change_shows_the_diff_and_the_verdict() {
        let verdict = Verdict {
            notify: true,
            reason: VerdictReason::Matched,
            triggered_text: vec!["  new price: 12".into()],
        };
        let out = render(
            "https://x.example",
            &preview(
                "a\nnew price: 12\n",
                Some("a\nold price: 10\n"),
                Some(verdict),
            ),
            Duration::from_millis(7),
            false,
        );
        assert!(
            out.contains("--- compared with the snapshot of 2001-09-09T01:46:40Z ---"),
            "{out}"
        );
        assert!(out.contains("-old price: 10\n+new price: 12\n"), "{out}");
        assert!(
            out.ends_with("would notify (matched)\n  matched: new price: 12\n"),
            "{out}"
        );
    }

    #[test]
    fn a_suppressed_change_says_why() {
        let verdict = Verdict {
            notify: false,
            reason: VerdictReason::NoTriggerMatched,
            triggered_text: Vec::new(),
        };
        let out = render(
            "https://x.example",
            &preview("b\n", Some("a\n"), Some(verdict)),
            Duration::from_millis(1),
            false,
        );
        assert!(
            out.ends_with("would not notify (no added line matches trigger_text)\n"),
            "{out}"
        );
    }

    #[test]
    fn the_same_text_and_empty_text_are_stated() {
        let out = render(
            "u",
            &preview("a\n", Some("a\n"), None),
            Duration::ZERO,
            false,
        );
        assert!(out.contains("no difference\n"), "{out}");
        assert!(!out.contains("would"), "{out}");
        let out = render("u", &preview("", None, None), Duration::ZERO, false);
        assert!(out.contains("--- output ---\n(empty)\n"), "{out}");
    }
}
