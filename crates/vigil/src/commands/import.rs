//! `vigil import`: read a file, say what it holds, and make the watches.

use std::io::Read;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, bail};
use vigil_import::{Existing, Format, MAX_INPUT_BYTES, Options, plan};

use super::open_store;
use crate::config::Config;

/// Read the whole file, or standard input for `-`, up to the largest file an import reads.
fn read_input(file: &str) -> anyhow::Result<Vec<u8>> {
    let limit = u64::try_from(MAX_INPUT_BYTES).unwrap_or(u64::MAX);
    let mut bytes = Vec::new();
    if file == "-" {
        std::io::stdin()
            .lock()
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .context("reading standard input")?;
    } else {
        std::fs::File::open(file)
            .with_context(|| format!("opening {file}"))?
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading {file}"))?;
    }
    if bytes.len() as u64 > limit {
        bail!("the file is larger than the {MAX_INPUT_BYTES} byte limit");
    }
    Ok(bytes)
}

/// Print what a file would import and, unless it is a dry run, import it.
pub async fn run(
    config: &Config,
    file: &str,
    format: Option<Format>,
    dry_run: bool,
    every: Duration,
    allow_duplicates: bool,
) -> anyhow::Result<ExitCode> {
    let bytes = read_input(file)?;
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
    let plan = plan(&bytes, format, &options, &existing)?;
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
