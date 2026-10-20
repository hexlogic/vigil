//! Command-line interface definition.

use std::path::PathBuf;

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
    /// Start the HTTP server.
    Serve,
}
