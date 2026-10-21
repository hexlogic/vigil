//! `vigil check <id>`.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use vigil_fetch::Fetcher;
use vigil_scheduler::{CheckResult, Checker};

use super::notify::notifier;
use super::{open_store, resolve};
use crate::config::Config;

/// Check a watch right now. Exits with failure when the page could not be checked.
///
/// A change the watch's conditions want announced is delivered to its targets before the
/// command returns, unless `no_notify`. A delivery that fails is reported on stderr but does
/// not make the check fail: the page was checked fine.
pub async fn run(config: &Config, id: &str, no_notify: bool) -> anyhow::Result<ExitCode> {
    let store = open_store(config).await?;
    let id = resolve(&store, id).await?;
    let watch = store.get_watch(id).await?;
    let fetcher = Fetcher::new(config.fetch_config()).context("building the http client")?;
    let mut checker = Checker::new(store.clone(), fetcher);
    let notifier = if no_notify {
        None
    } else {
        Some(Arc::new(notifier(config, &store)?.collecting_reports()))
    };
    if let Some(notifier) = &notifier {
        checker = checker.with_notifier(Arc::clone(notifier));
    }
    let report = checker.check(id).await?;
    let took = format!("{}ms", report.duration.as_millis());

    match report.result {
        CheckResult::Baseline => {
            println!(
                "{}: baseline stored, nothing to compare yet ({took})",
                watch.url
            );
        }
        CheckResult::Unchanged => println!("{}: unchanged ({took})", watch.url),
        CheckResult::Changed { diff, verdict, .. } => {
            let notify = if verdict.notify {
                String::new()
            } else {
                format!("; no notification: {}", verdict.reason)
            };
            println!(
                "{}: changed, +{} -{} lines ({took}){notify}; run `vigil diff {id}` to see it",
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
    if let Some(notifier) = notifier {
        for sent in notifier.close_and_wait().await {
            match sent.report.result {
                Ok(()) => println!("notified {}", sent.target),
                Err(error) if sent.queued => eprintln!(
                    "could not notify {}: {error}; it is kept, and a running `vigil serve` sends it \
                     when the service is back",
                    sent.target
                ),
                Err(error) => eprintln!("could not notify {}: {error}", sent.target),
            }
            if let Some(note) = sent.report.note {
                eprintln!("note: {note}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
