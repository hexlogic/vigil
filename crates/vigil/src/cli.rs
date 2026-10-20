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
    /// Check a watch now and report what changed.
    Check {
        /// Watch id, or the last 6 or more characters of it.
        id: String,
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
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("vigil").chain(args.iter().copied()))
    }

    #[test]
    fn serve_is_the_server_command() -> Result<(), clap::Error> {
        assert!(parse(&["serve"])?.command.is_server());
        assert!(!parse(&["watch", "list"])?.command.is_server());
        Ok(())
    }

    #[test]
    fn watch_add_parses_url_interval_and_name() -> Result<(), clap::Error> {
        let cli = parse(&[
            "watch",
            "add",
            "https://example.com",
            "--every",
            "5m",
            "--name",
            "Ex",
        ])?;
        let Command::Watch {
            command: WatchCommand::Add {
                url, every, name, ..
            },
        } = cli.command
        else {
            panic!("expected watch add");
        };
        assert_eq!(url, "https://example.com");
        assert_eq!(every, Duration::from_secs(300));
        assert_eq!(name.as_deref(), Some("Ex"));
        Ok(())
    }

    #[test]
    fn watch_add_defaults_to_hourly() -> Result<(), clap::Error> {
        let cli = parse(&["watch", "add", "https://example.com"])?;
        let Command::Watch {
            command: WatchCommand::Add { every, name, .. },
        } = cli.command
        else {
            panic!("expected watch add");
        };
        assert_eq!(every, Duration::from_secs(3600));
        assert_eq!(name, None);
        Ok(())
    }

    #[test]
    fn intervals_accept_compound_durations() {
        assert_eq!(parse_interval("1h 30m"), Ok(Duration::from_secs(5400)));
        assert_eq!(parse_interval("90s"), Ok(Duration::from_secs(90)));
    }

    #[test]
    fn the_longest_interval_is_a_year() {
        assert_eq!(parse_interval("365d"), Ok(MAX_INTERVAL));
        assert!(parse_interval("366d").is_err());
        assert!(parse_interval("500000000000y").is_err());
    }

    #[test]
    fn bad_intervals_are_rejected() {
        assert!(parse(&["watch", "add", "https://example.com", "--every", "0s"]).is_err());
        assert!(parse(&["watch", "add", "https://example.com", "--every", "500ms"]).is_err());
        assert!(parse(&["watch", "add", "https://example.com", "--every", "soon"]).is_err());
    }

    #[test]
    fn check_diff_and_remove_take_an_id() -> Result<(), clap::Error> {
        assert!(
            matches!(parse(&["check", "abc123"])?.command, Command::Check { id } if id == "abc123")
        );
        assert!(
            matches!(parse(&["diff", "abc123"])?.command, Command::Diff { id } if id == "abc123")
        );
        let remove = parse(&["watch", "remove", "abc123"])?.command;
        assert!(matches!(
            remove,
            Command::Watch { command: WatchCommand::Remove { id } } if id == "abc123"
        ));
        Ok(())
    }

    #[test]
    fn watch_add_takes_content_options() -> Result<(), clap::Error> {
        let cli = parse(&[
            "watch",
            "add",
            "https://example.com",
            "--processor",
            "json",
            "--pipeline",
            "[]",
            "--conditions",
            "@rules.json",
        ])?;
        let Command::Watch {
            command: WatchCommand::Add { content, .. },
        } = cli.command
        else {
            panic!("expected watch add");
        };
        assert_eq!(content.processor, Some(Processor::Json));
        assert_eq!(content.pipeline.as_deref(), Some("[]"));
        assert_eq!(content.conditions.as_deref(), Some("@rules.json"));
        assert!(!content.is_empty());
        Ok(())
    }

    #[test]
    fn watch_add_without_content_options_has_none() -> Result<(), clap::Error> {
        let cli = parse(&["watch", "add", "https://example.com"])?;
        let Command::Watch {
            command: WatchCommand::Add { content, .. },
        } = cli.command
        else {
            panic!("expected watch add");
        };
        assert!(content.is_empty());
        Ok(())
    }

    #[test]
    fn unknown_processors_are_rejected() {
        assert!(parse(&["watch", "add", "https://example.com", "--processor", "xml"]).is_err());
    }

    #[test]
    fn watch_edit_changes_only_what_it_is_given() -> Result<(), clap::Error> {
        let cli = parse(&["watch", "edit", "abc123", "--every", "10m", "--name", "N"])?;
        let Command::Watch {
            command:
                WatchCommand::Edit {
                    id,
                    every,
                    name,
                    content,
                },
        } = cli.command
        else {
            panic!("expected watch edit");
        };
        assert_eq!(id, "abc123");
        assert_eq!(every, Some(Duration::from_secs(600)));
        assert_eq!(name.as_deref(), Some("N"));
        assert!(content.is_empty());
        assert!(parse(&["watch", "edit", "abc123", "--every", "0s"]).is_err());
        assert!(parse(&["watch", "edit"]).is_err());
        Ok(())
    }

    #[test]
    fn preview_takes_an_id_and_optional_overrides() -> Result<(), clap::Error> {
        let cli = parse(&["preview", "abc123", "--pipeline", "[]"])?;
        let Command::Preview { id, content } = cli.command else {
            panic!("expected preview");
        };
        assert_eq!(id, "abc123");
        assert_eq!(content.pipeline.as_deref(), Some("[]"));
        assert!(parse(&["preview"]).is_err());
        Ok(())
    }

    #[test]
    fn watch_show_takes_an_id() -> Result<(), clap::Error> {
        assert!(matches!(
            parse(&["watch", "show", "abc123"])?.command,
            Command::Watch { command: WatchCommand::Show { id } } if id == "abc123"
        ));
        Ok(())
    }

    #[test]
    fn missing_arguments_are_errors() {
        assert!(parse(&["check"]).is_err());
        assert!(parse(&["watch", "add"]).is_err());
        assert!(parse(&[]).is_err());
    }
}
