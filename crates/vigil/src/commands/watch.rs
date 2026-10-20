//! `vigil watch add|list|remove`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use vigil_core::{Schedule, Watch};
use vigil_store::NewWatch;

use super::{open_store, resolve};
use crate::config::Config;
use crate::format::{ago, render_table};

/// Start watching `url`, checking every `every`.
pub async fn add(
    config: &Config,
    url: &str,
    every: Duration,
    name: Option<String>,
) -> anyhow::Result<()> {
    vigil_fetch::check_url(url).with_context(|| format!("cannot watch {url}"))?;
    let store = open_store(config).await?;
    let mut new = NewWatch::new(url, Schedule::Interval(every.as_secs()));
    if let Some(name) = name {
        new.name = name;
    }
    let watch = store.create_watch(new).await?;
    println!(
        "added {} {} (every {})",
        watch.id,
        watch.url,
        humantime::format_duration(Duration::from_secs(every.as_secs()))
    );
    Ok(())
}

/// Print all watches as a table.
pub async fn list(config: &Config) -> anyhow::Result<()> {
    let store = open_store(config).await?;
    let watches = store.list_watches().await?;
    if watches.is_empty() {
        println!("no watches yet; add one with `vigil watch add <url>`");
        return Ok(());
    }
    print!("{}", render(&watches, now_ms()));
    Ok(())
}

/// Delete a watch and its history.
pub async fn remove(config: &Config, id: &str) -> anyhow::Result<()> {
    let store = open_store(config).await?;
    let id = resolve(&store, id).await?;
    let watch = store.get_watch(id).await?;
    store.delete_watch(id).await?;
    println!("removed {} {}", watch.id, watch.url);
    Ok(())
}

fn render(watches: &[Watch], now_ms: i64) -> String {
    let mut rows = vec![vec![
        "ID".to_owned(),
        "WATCH".to_owned(),
        "EVERY".to_owned(),
        "CHECKED".to_owned(),
        "CHANGED".to_owned(),
        "STATUS".to_owned(),
    ]];
    for watch in watches {
        rows.push(vec![
            watch.id.to_string(),
            describe(watch),
            humantime::format_duration(watch.schedule.interval()).to_string(),
            ago(watch.state.last_checked_at, now_ms),
            ago(watch.state.last_changed_at, now_ms),
            status(watch),
        ]);
    }
    render_table(&rows)
}

fn describe(watch: &Watch) -> String {
    if watch.name == watch.url {
        watch.url.clone()
    } else {
        format!("{} <{}>", watch.name, watch.url)
    }
}

fn status(watch: &Watch) -> String {
    if watch.paused {
        "paused".into()
    } else if watch.state.consecutive_errors > 0 {
        format!("error x{}", watch.state.consecutive_errors)
    } else if watch.state.last_checked_at.is_some() {
        "ok".into()
    } else {
        "pending".into()
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use vigil_core::{Conditions, FetchSettings, Processor, WatchState};

    use super::*;

    fn watch(name: &str, state: WatchState, paused: bool) -> Watch {
        Watch {
            id: "00000000-0000-0000-0000-000000000001"
                .parse()
                .expect("valid id"),
            name: name.into(),
            url: "https://example.com".into(),
            paused,
            fetch: FetchSettings::default(),
            schedule: Schedule::Interval(300),
            jitter_secs: 0,
            processor: Processor::Text,
            pipeline: Vec::new(),
            conditions: Conditions::default(),
            snapshot_retention: 100,
            created_at: 0,
            updated_at: 0,
            state,
        }
    }

    #[test]
    fn status_reflects_the_watch_state() {
        let pending = watch("w", WatchState::default(), false);
        assert_eq!(status(&pending), "pending");
        assert_eq!(status(&watch("w", WatchState::default(), true)), "paused");

        let checked = WatchState {
            last_checked_at: Some(1),
            ..WatchState::default()
        };
        assert_eq!(status(&watch("w", checked, false)), "ok");

        let failing = WatchState {
            last_checked_at: Some(1),
            consecutive_errors: 3,
            ..WatchState::default()
        };
        assert_eq!(status(&watch("w", failing, false)), "error x3");
    }

    #[test]
    fn custom_names_show_the_url_too() {
        let plain = watch("https://example.com", WatchState::default(), false);
        assert_eq!(describe(&plain), "https://example.com");
        let named = watch("Example", WatchState::default(), false);
        assert_eq!(describe(&named), "Example <https://example.com>");
    }

    #[test]
    fn the_table_lists_every_watch_with_full_ids() {
        let state = WatchState {
            last_checked_at: Some(1_000_000 - 120_000),
            last_changed_at: Some(1_000_000 - 7_200_000),
            ..WatchState::default()
        };
        let table = render(&[watch("https://example.com", state, false)], 1_000_000);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("ID "));
        assert!(lines[1].contains("00000000-0000-0000-0000-000000000001"));
        assert!(lines[1].contains("5m"));
        assert!(lines[1].contains("2m ago"));
        assert!(lines[1].contains("2h ago"));
    }
}
