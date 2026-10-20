//! Tests for the health endpoint and routing fallbacks.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use vigil_store::Store;
use vigil_web::{AppState, router};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn app() -> Result<(tempfile::TempDir, axum::Router, Store), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    let app = router(AppState {
        store: store.clone(),
    });
    Ok((dir, app, store))
}

#[tokio::test]
async fn healthz_returns_ok() -> TestResult {
    let (_dir, app, _store) = app().await?;
    let response = app
        .oneshot(Request::get("/healthz").body(Body::empty())?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await?.to_bytes();
    assert_eq!(&body[..], b"ok");
    Ok(())
}

#[tokio::test]
async fn healthz_reports_unavailable_when_database_is_closed() -> TestResult {
    let (_dir, app, store) = app().await?;
    store.pool().close().await;
    let response = app
        .oneshot(Request::get("/healthz").body(Body::empty())?)
        .await?;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}

#[tokio::test]
async fn unknown_route_is_404() -> TestResult {
    let (_dir, app, _store) = app().await?;
    let response = app
        .oneshot(Request::get("/nope").body(Body::empty())?)
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    Ok(())
}
