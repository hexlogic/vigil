//! The `serve` subcommand: run the HTTP server and the check scheduler until signalled.

use anyhow::Context;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use vigil_fetch::Fetcher;
use vigil_scheduler::{Checker, Scheduler};
use vigil_web::{AppState, router};

use crate::commands::open_store;
use crate::config::Config;

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
    let scheduler = Scheduler::new(
        Checker::new(store.clone(), fetcher),
        config.scheduler_settings(),
    );
    let cancel = CancellationToken::new();
    let scheduler_task = tokio::spawn({
        let cancel = cancel.clone();
        async move { scheduler.run(cancel).await }
    });

    let shutdown = cancel.clone();
    let served = axum::serve(listener, router(AppState { store }))
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            shutdown.cancel();
        })
        .await;

    // Also covers the server failing on its own: stop the scheduler either way.
    cancel.cancel();
    if let Err(err) = scheduler_task.await {
        tracing::error!(%err, "scheduler task failed");
    }
    served.context("http server failed")?;
    tracing::info!("shut down cleanly");
    Ok(())
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
