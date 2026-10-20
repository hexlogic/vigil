//! The `serve` subcommand: open the store and run the HTTP server until signalled.

use anyhow::Context;
use tokio::net::TcpListener;
use vigil_store::Store;
use vigil_web::{AppState, router};

use crate::config::Config;

/// Run the HTTP server until Ctrl-C or SIGTERM.
pub async fn run(config: Config) -> anyhow::Result<()> {
    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("creating data dir {}", config.data_dir.display()))?;
    let db_path = config.data_dir.join("vigil.db");
    let store = Store::open(&db_path)
        .await
        .with_context(|| format!("opening database {}", db_path.display()))?;

    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("binding {}", config.listen))?;
    tracing::info!(addr = %listener.local_addr()?, db = %db_path.display(), "vigil listening");

    axum::serve(listener, router(AppState { store }))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("http server failed")?;
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
