//! The `serve` subcommand: run the HTTP server and the check scheduler until signalled.

use std::future::{Future, IntoFuture};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use vigil_fetch::Fetcher;
use vigil_scheduler::{Checker, Scheduler};
use vigil_web::{AppState, router};

use crate::commands::notify::notifier;
use crate::commands::open_store;
use crate::config::Config;

/// How long shutdown waits for notifications that are being delivered.
const NOTIFY_GRACE: Duration = Duration::from_secs(30);

/// Run until Ctrl-C or SIGTERM: stop starting checks, let running ones finish, then exit.
pub async fn run(config: Config) -> anyhow::Result<()> {
    let store = open_store(&config).await?;

    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("binding {}", config.listen))?;
    tracing::info!(
        addr = %listener.local_addr()?,
        db = %config.db_path().display(),
        "vigil listening"
    );

    let fetcher = Fetcher::new(config.fetch_config()).context("building the http client")?;
    let notifier = Arc::new(notifier(&config, &store)?);
    // Notifications that a shutdown or a crash cut short are sent before anything new is checked.
    let resumed = notifier.resume().await;
    if resumed > 0 {
        tracing::info!(resumed, "sending notifications left over from the last run");
    }
    let scheduler = Scheduler::new(
        Checker::new(store.clone(), fetcher).with_notifier(Arc::clone(&notifier)),
        config.scheduler_settings(),
    );
    let cancel = CancellationToken::new();
    let scheduler_task = tokio::spawn({
        let cancel = cancel.clone();
        async move { scheduler.run(cancel).await }
    });

    let shutdown = cancel.clone();
    let server =
        axum::serve(listener, router(AppState { store })).with_graceful_shutdown(async move {
            shutdown_signal().await;
            shutdown.cancel();
        });
    let served = supervise(server.into_future(), scheduler_task, &cancel).await;
    // Notifications still being delivered get a moment after the last check has finished.
    notifier.shutdown(NOTIFY_GRACE).await;
    served?;
    tracing::info!("shut down cleanly");
    Ok(())
}

/// Drive the server to completion while watching the scheduler task.
///
/// The scheduler only ends on its own after `cancel` fired. If it ends earlier (it panicked),
/// nothing would be checked any more while `/healthz` still answers, so that is an error and
/// the process should exit for its supervisor to restart.
async fn supervise<S>(
    server: S,
    mut scheduler: JoinHandle<()>,
    cancel: &CancellationToken,
) -> anyhow::Result<()>
where
    S: Future<Output = std::io::Result<()>>,
{
    tokio::pin!(server);
    tokio::select! {
        served = &mut server => {
            // Covers the server failing on its own: stop the scheduler either way.
            cancel.cancel();
            if let Err(err) = (&mut scheduler).await {
                tracing::error!(%err, "scheduler task failed");
            }
            served.context("http server failed")
        }
        joined = &mut scheduler => {
            if cancel.is_cancelled() {
                // Orderly shutdown: the scheduler finished draining, so wait for the server.
                if let Err(err) = joined {
                    tracing::error!(%err, "scheduler task failed");
                }
                server.await.context("http server failed")
            } else {
                match joined {
                    Ok(()) => anyhow::bail!("scheduler stopped unexpectedly"),
                    Err(err) => anyhow::bail!("scheduler task failed: {err}"),
                }
            }
        }
    }
}

/// Resolve when the process receives Ctrl-C or (on Unix) SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(%err, "failed to listen for Ctrl-C");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sigterm) => {
                sigterm.recv().await;
            }
            Err(err) => {
                tracing::error!(%err, "failed to listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutdown signal received");
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::*;

    /// A scheduler stand-in that, like the real one, ends when cancelled.
    fn scheduler_until_cancelled(cancel: &CancellationToken) -> JoinHandle<()> {
        let cancel = cancel.clone();
        tokio::spawn(async move { cancel.cancelled().await })
    }

    #[tokio::test]
    async fn a_panicking_scheduler_fails_the_server() {
        let cancel = CancellationToken::new();
        let scheduler = tokio::spawn(async { panic!("scheduler blew up") });
        let result = supervise(std::future::pending(), scheduler, &cancel).await;
        let message = format!("{:#}", result.expect_err("must fail"));
        assert!(message.contains("scheduler task failed"), "{message}");
    }

    #[tokio::test]
    async fn a_scheduler_that_stops_early_fails_the_server() {
        let cancel = CancellationToken::new();
        let scheduler = tokio::spawn(async {});
        let result = supervise(std::future::pending(), scheduler, &cancel).await;
        let message = format!("{:#}", result.expect_err("must fail"));
        assert!(message.contains("stopped unexpectedly"), "{message}");
    }

    #[tokio::test]
    async fn the_scheduler_is_stopped_when_the_server_ends() -> anyhow::Result<()> {
        let cancel = CancellationToken::new();
        let scheduler = scheduler_until_cancelled(&cancel);
        supervise(async { Ok(()) }, scheduler, &cancel).await?;
        assert!(cancel.is_cancelled());
        Ok(())
    }

    #[tokio::test]
    async fn the_scheduler_is_stopped_when_the_server_fails() {
        let cancel = CancellationToken::new();
        let scheduler = scheduler_until_cancelled(&cancel);
        let failing = async { Err(std::io::Error::other("bind lost")) };
        let result = supervise(failing, scheduler, &cancel).await;
        let message = format!("{:#}", result.expect_err("must fail"));
        assert!(message.contains("http server failed"), "{message}");
        assert!(cancel.is_cancelled());
    }

    #[tokio::test]
    async fn after_a_shutdown_signal_the_server_is_awaited_to_the_end() -> anyhow::Result<()> {
        let cancel = CancellationToken::new();
        let scheduler = scheduler_until_cancelled(&cancel);
        let finished = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&finished);
        // The server drains slowly: the scheduler is done long before it is.
        let server = async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            flag.store(true, Ordering::SeqCst);
            Ok(())
        };
        cancel.cancel();
        supervise(server, scheduler, &cancel).await?;
        assert!(finished.load(Ordering::SeqCst));
        Ok(())
    }
}
