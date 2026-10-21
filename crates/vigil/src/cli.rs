//! Command-line interface definition.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use vigil_core::{Method, Processor};
use vigil_import::Format;
use vigil_scheduler::parse_interval;

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
    Serve {
        /// Let anyone who can reach the server in, without a password. For a trusted network
        /// only; also set by `no_auth = true` or `VIGIL_NO_AUTH=true`.
        #[arg(long)]
        no_auth: bool,
    },
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
    /// Import watches from a file, and say what could not be carried over.
    ///
    /// The file is a list of addresses (one per line, optionally followed by tags), an OPML
    /// outline of feeds, a backup of another self-hosted change monitor (a ZIP or its older
    /// JSON file), a urlwatch `urls.yaml`, or a Distill JSON export. The format is guessed
    /// from the content unless `--format` says. Use `--dry-run` to see the report first.
    Import {
        /// The file to read, or `-` for standard input.
        file: String,
        /// What the file is: url-list, opml, backup, urlwatch or distill.
        #[arg(long, value_name = "FORMAT")]
        format: Option<Format>,
        /// Show what would be imported and change nothing.
        #[arg(long)]
        dry_run: bool,
        /// Time between checks for sources that do not say, e.g. 30m, 1h, 1d.
        #[arg(long, default_value = "1h", value_parser = parse_interval, value_name = "DURATION")]
        every: Duration,
        /// Make a watch even when one with its address exists.
        #[arg(long)]
        allow_duplicates: bool,
    },
    /// Make, list or revoke API tokens.
    Token {
        /// What to do with API tokens.
        #[command(subcommand)]
        command: TokenCommand,
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
        matches!(self, Self::Serve { .. })
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
    /// The request method: GET (the default), POST, PUT, PATCH or DELETE.
    #[arg(long, value_name = "METHOD")]
    pub method: Option<Method>,
    /// The request body, as text or @FILE; empty removes it. Needs a method other than GET.
    /// Send a Content-Type header too if the server wants one.
    #[arg(long, value_name = "TEXT|@FILE")]
    pub body: Option<String>,
}

impl ContentArgs {
    /// True when none of the options was given.
    pub fn is_empty(&self) -> bool {
        self.processor.is_none()
            && self.pipeline.is_none()
            && self.conditions.is_none()
            && self.method.is_none()
            && self.body.is_none()
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

/// `vigil token` subcommands.
#[derive(Debug, Subcommand)]
pub enum TokenCommand {
    /// Make an API token and print it. It is shown only this once.
    ///
    /// Send it to the REST API in the x-api-key header. Capture it in a script with
    /// `TOKEN=$(vigil token add ci)`.
    Add {
        /// A name for the token, such as "backup script".
        name: String,
    },
    /// List API tokens (never the tokens themselves).
    List,
    /// Revoke a token; programs that use it stop working.
    Remove {
        /// Token name, or its id or the last 6 or more characters of it.
        token: String,
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

#[cfg(test)]
mod tests;
