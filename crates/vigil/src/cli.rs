//! Command-line interface definition.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use vigil_core::Processor;
use vigil_scheduler::backoff::MAX_INTERVAL;

/// Self-hosted website change detection.
#[derive(Debug, Parser)]
#[command(name = "vigil", version, about)]
pub struct Cli {
    /// Path to the TOML config file (default: ./vigil.toml if present).
    #[arg(long, env = "VIGIL_CONFIG", global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Available subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the HTTP server and the check scheduler.
    Serve,
    /// Add, list or remove watches.
    Watch {
        /// What to do with watches.
        #[command(subcommand)]
        command: WatchCommand,
    },
    /// Add, list, remove or test notification targets.
    Notify {
        /// What to do with notification targets.
        #[command(subcommand)]
        command: NotifyCommand,
    },
    /// Check a watch now and report what changed.
    ///
    /// A change that the watch's conditions want announced is sent to its notification
    /// targets, and the command waits until that is done.
    Check {
        /// Watch id, or the last 6 or more characters of it.
        id: String,
        /// Do not send notifications for this check.
        #[arg(long)]
        no_notify: bool,
    },
    /// Show the latest change of a watch as a diff.
    Diff {
        /// Watch id, or the last 6 or more characters of it.
        id: String,
    },
    /// Fetch a watch and show what a check would store, without saving anything.
    ///
    /// The options replace the watch's settings for this run only, to try a pipeline before
    /// saving it with `vigil watch edit`.
    Preview {
        /// Watch id, or the last 6 or more characters of it.
        id: String,
        /// Settings to try instead of the watch's own.
        #[command(flatten)]
        content: ContentArgs,
    },
}

impl Command {
    /// Long-running commands log at info level to stdout; the others keep stdout for results.
    pub fn is_server(&self) -> bool {
        matches!(self, Self::Serve)
    }
}

/// What a watch compares and when it notifies. Each option replaces the watch's current
/// setting as a whole.
#[derive(Debug, Clone, Default, Args)]
pub struct ContentArgs {
    /// How the response becomes text: text (the default), json or feed.
    #[arg(long, value_name = "NAME")]
    pub processor: Option<Processor>,
    /// Steps applied after the processor, as a JSON array or @FILE holding one.
    /// Example: '[{"type": "css", "selector": "#price"}]'
    #[arg(long, value_name = "JSON|@FILE")]
    pub pipeline: Option<String>,
    /// Which changes notify, as a JSON object or @FILE holding one.
    /// Example: '{"trigger_text": ["in stock"]}'
    #[arg(long, value_name = "JSON|@FILE")]
    pub conditions: Option<String>,
}

impl ContentArgs {
    /// True when none of the options was given.
    pub fn is_empty(&self) -> bool {
        self.processor.is_none() && self.pipeline.is_none() && self.conditions.is_none()
    }
}

/// Where a watch's notifications go.
#[derive(Debug, Clone, Default, Args)]
pub struct NotifyArgs {
    /// Send this watch's notifications to a target, by name or id (repeat for several).
    /// Targets are managed with `vigil notify`.
    #[arg(long = "notify", value_name = "TARGET")]
    pub notify: Vec<String>,
    /// Message template for this watch's notifications, as text or @FILE; empty removes it.
    #[arg(long, value_name = "TEXT|@FILE")]
    pub notify_template: Option<String>,
}

/// `vigil watch` subcommands.
#[derive(Debug, Subcommand)]
pub enum WatchCommand {
    /// Start watching a URL.
    Add {
        /// The page to watch (http or https).
        url: String,
        /// Time between checks, e.g. 30s, 5m, 1h, 1d.
        #[arg(long, default_value = "1h", value_parser = parse_interval, value_name = "DURATION")]
        every: Duration,
        /// Display name (default: the URL).
        #[arg(long)]
        name: Option<String>,
        /// What to compare and when to notify.
        #[command(flatten)]
        content: ContentArgs,
        /// Where to send notifications.
        #[command(flatten)]
        notify: NotifyArgs,
    },
    /// Change a watch. Only the options given are changed.
    Edit {
        /// Watch id, or the last 6 or more characters of it.
        id: String,
        /// New time between checks, e.g. 30s, 5m, 1h, 1d.
        #[arg(long, value_parser = parse_interval, value_name = "DURATION")]
        every: Option<Duration>,
        /// New display name.
        #[arg(long)]
        name: Option<String>,
        /// What to compare and when to notify.
        #[command(flatten)]
        content: ContentArgs,
        /// Where to send notifications. Only the targets given are added.
        #[command(flatten)]
        notify: NotifyArgs,
        /// Stop sending this watch's notifications to a target (repeat for several).
        #[arg(long = "no-notify", value_name = "TARGET")]
        no_notify: Vec<String>,
    },
    /// Print a watch's settings as JSON.
    Show {
        /// Watch id, or the last 6 or more characters of it.
        id: String,
    },
    /// List all watches.
    List,
    /// Stop watching and delete a watch with its history.
    Remove {
        /// Watch id, or the last 6 or more characters of it.
        id: String,
    },
}

/// `vigil notify` subcommands.
#[derive(Debug, Subcommand)]
pub enum NotifyCommand {
    /// Add a place to send notifications to.
    ///
    /// The URL says where: ntfys://ntfy.sh/TOPIC, tgram://BOT_TOKEN/CHAT_ID,
    /// discord://WEBHOOK_ID/WEBHOOK_TOKEN, slack://T000/B000/XXXX,
    /// mailto://USER:PASSWORD@smtp.example.com?to=you@example.com, json://host/path,
    /// form://host/path or gotify://host/TOKEN. Add an s for HTTPS (ntfys://, jsons://, ...);
    /// mailtos:// uses TLS from the start. Link a target to a watch with `vigil watch edit
    /// --notify NAME`.
    Add {
        /// A name for the target, such as "phone".
        name: String,
        /// The notification URL, which usually holds credentials.
        url: String,
        /// Message template for this target, as text or @FILE.
        #[arg(long, value_name = "TEXT|@FILE")]
        template: Option<String>,
    },
    /// List notification targets, with their credentials hidden.
    List,
    /// Delete a target; watches stop notifying it.
    Remove {
        /// Target name, or its id or the last 6 or more characters of it.
        target: String,
    },
    /// Send a test notification to a target.
    Test {
        /// Target name, or its id or the last 6 or more characters of it.
        target: String,
    },
}

/// Parse an interval such as `5m`, rejecting anything under one second.
fn parse_interval(text: &str) -> Result<Duration, String> {
    let interval = humantime::parse_duration(text).map_err(|e| format!("{e} (try 30s, 5m, 1h)"))?;
    if interval < Duration::from_secs(1) {
        return Err("the interval must be at least 1s".into());
    }
    if interval > MAX_INTERVAL {
        return Err("the interval must be at most 365 days".into());
    }
    Ok(interval)
}

#[cfg(test)]
mod tests;
