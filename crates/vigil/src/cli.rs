//! Command-line interface definition.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};

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
}

impl Command {
    /// Long-running commands log at info level to stdout; the others keep stdout for results.
    pub fn is_server(&self) -> bool {
        matches!(self, Self::Serve)
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
            command: WatchCommand::Add { url, every, name },
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
    fn missing_arguments_are_errors() {
        assert!(parse(&["check"]).is_err());
        assert!(parse(&["watch", "add"]).is_err());
        assert!(parse(&[]).is_err());
    }
}
