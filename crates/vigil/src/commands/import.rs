//! `vigil import`: read a file, say what it holds, and make the watches.

use std::io::Read;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use vigil_import::{Existing, Format, MAX_INPUT_BYTES, Options, plan, plan_reader};

use super::open_store;
use crate::config::Config;

/// Print what a file would import and, unless it is a dry run, import it.
pub async fn run(
    config: &Config,
    file: &str,
    format: Option<Format>,
    dry_run: bool,
    every: Duration,
    allow_duplicates: bool,
) -> anyhow::Result<ExitCode> {
    let store = open_store(config).await?;
    let existing = Existing::new(
        &store.list_tags().await?,
        &store.list_targets().await?,
        &store.list_watches().await?,
    );
    let options = Options {
        every,
        allow_duplicates,
    };
    let plan = if file == "-" {
        // Standard input cannot seek, so it is read whole, up to the largest file.
        let limit = u64::try_from(MAX_INPUT_BYTES).unwrap_or(u64::MAX);
        let mut bytes = Vec::new();
        std::io::stdin()
            .lock()
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .context("reading standard input")?;
        plan(&bytes, format, &options, &existing)?
    } else {
        // A file is read as it is needed: a large backup is never held in memory.
        let opened = std::fs::File::open(file).with_context(|| format!("opening {file}"))?;
        let len = opened
            .metadata()
            .with_context(|| format!("reading {file}"))?
            .len();
        plan_reader(opened, len, format, &options, &existing)?
    };
    print!("{}", plan.report.render_text());
    if dry_run {
        println!("\ndry run: nothing was changed");
        return Ok(ExitCode::SUCCESS);
    }
    let outcome = store.apply_import(&plan.batch).await?;
    let made = outcome.watches.iter().flatten().count();
    println!(
        "\nmade {made} watch(es), {} tag(s) and {} notification target(s)",
        outcome.tags_created, outcome.targets_created
    );
    if made > 0 {
        println!(
            "a running `vigil serve` starts checking them within 30 seconds; the first check of each is its baseline and does not notify"
        );
    }
    Ok(ExitCode::SUCCESS)
}
