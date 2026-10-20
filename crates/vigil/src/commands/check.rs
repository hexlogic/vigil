//! `vigil check <id>`.

use std::process::ExitCode;

use anyhow::Context;
use vigil_fetch::Fetcher;
use vigil_scheduler::{CheckResult, Checker};

use super::{open_store, resolve};
use crate::config::Config;

/// Check a watch right now. Exits with failure when the page could not be checked.
pub async fn run(config: &Config, id: &str) -> anyhow::Result<ExitCode> {
    let store = open_store(config).await?;
    let id = resolve(&store, id).await?;
    let watch = store.get_watch(id).await?;
    let fetcher = Fetcher::new(config.fetch_config()).context("building the http client")?;
    let report = Checker::new(store, fetcher).check(id).await?;
    let took = format!("{}ms", report.duration.as_millis());

    match report.result {
        CheckResult::Baseline => {
            println!(
                "{}: baseline stored, nothing to compare yet ({took})",
                watch.url
            );
        }
        CheckResult::Unchanged => println!("{}: unchanged ({took})", watch.url),
        CheckResult::Changed { diff, .. } => {
            println!(
                "{}: changed, +{} -{} lines ({took}); run `vigil diff {id}` to see it",
                watch.url,
                diff.added_count(),
                diff.removed_count(),
            );
        }
        CheckResult::Failed {
            message,
            consecutive_errors,
        } => {
            eprintln!("{}: check failed ({took}): {message}", watch.url);
            if consecutive_errors > 1 {
                eprintln!("{consecutive_errors} failed checks in a row");
            }
            return Ok(ExitCode::FAILURE);
        }
    }
    Ok(ExitCode::SUCCESS)
}
