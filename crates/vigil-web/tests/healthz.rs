//! Tests for the health endpoint and routing fallbacks.

mod common;

use axum::http::StatusCode;

use common::*;

#[tokio::test]
async fn healthz_returns_ok() -> TestResult {
    let app = anonymous().await?;
    let response = app.get("/healthz").await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.body(), "ok");
    Ok(())
}

#[tokio::test]
async fn healthz_reports_unavailable_when_database_is_closed() -> TestResult {
    let app = anonymous().await?;
    app.store.pool().close().await;
    let response = app.get("/healthz").await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}

#[tokio::test]
async fn unknown_route_is_404() -> TestResult {
    let app = anonymous().await?;
    assert_eq!(app.get("/nope").await?.status(), StatusCode::NOT_FOUND);
    Ok(())
}
