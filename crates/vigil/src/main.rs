//! Vigil binary: CLI, configuration and wiring.

mod cli;
mod config;
mod serve;
mod telemetry;

use anyhow::{Context, ensure};
use clap::Parser;

use crate::cli::{Cli, Command};
use crate::config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if let Some(path) = &cli.config {
        ensure!(path.exists(), "config file {} not found", path.display());
    }
    let config = Config::load(cli.config.as_deref()).context("loading configuration")?;
    telemetry::init(config.log_format);

    match cli.command {
        Command::Serve => serve::run(config).await,
    }
}
