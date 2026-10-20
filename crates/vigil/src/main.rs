//! Vigil binary: CLI, configuration and wiring.

mod cli;
mod commands;
mod config;
mod format;
mod serve;
mod telemetry;

use std::process::ExitCode;

use anyhow::{Context, ensure};
use clap::Parser;

use crate::cli::{Cli, Command, WatchCommand};
use crate::config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();
    if let Some(path) = &cli.config {
        ensure!(path.exists(), "config file {} not found", path.display());
    }
    let config = Config::load(cli.config.as_deref()).context("loading configuration")?;
    telemetry::init(config.log_format, cli.command.is_server());

    match cli.command {
        Command::Serve => serve::run(config).await.map(|()| ExitCode::SUCCESS),
        Command::Watch { command } => {
            match command {
                WatchCommand::Add {
                    url,
                    every,
                    name,
                    content,
                } => commands::watch::add(&config, &url, every, name, &content).await?,
                WatchCommand::Edit {
                    id,
                    every,
                    name,
                    content,
                } => commands::watch::edit(&config, &id, every, name, &content).await?,
                WatchCommand::Show { id } => commands::watch::show(&config, &id).await?,
                WatchCommand::List => commands::watch::list(&config).await?,
                WatchCommand::Remove { id } => commands::watch::remove(&config, &id).await?,
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Check { id } => commands::check::run(&config, &id).await,
        Command::Diff { id } => commands::diff::run(&config, &id)
            .await
            .map(|()| ExitCode::SUCCESS),
    }
}
