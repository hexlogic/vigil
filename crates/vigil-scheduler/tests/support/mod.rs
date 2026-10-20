//! Shared setup for the notification tests: a store, a checker with a notifier, a mock web
//! server for the watched page and a mock ntfy server.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use vigil_core::{Schedule, Watch};
use vigil_fetch::{FetchConfig, Fetcher};
use vigil_notify::{RetryPolicy, Sender, SenderConfig};
use vigil_scheduler::{CheckResult, Checker, Notifier};
use vigil_store::{NewTarget, NewWatch, NotificationTarget, Store};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

pub const PAGE_V1: &str = "<html><body><h1>Price list</h1><p>Widget: $10</p></body></html>";
pub const PAGE_V2: &str = "<html><body><h1>Price list</h1><p>Widget: $12</p></body></html>";

pub struct Setup {
    pub _dir: tempfile::TempDir,
    pub store: Store,
    pub checker: Checker,
    pub notifier: Arc<Notifier>,
    pub page: MockServer,
    pub ntfy: MockServer,
}

pub fn quick_retries(delay: Duration) -> RetryPolicy {
    RetryPolicy {
        delays: vec![delay; 3],
        max_retry_after: Duration::from_secs(1),
    }
}

pub async fn setup_with(retry: RetryPolicy) -> Result<Setup, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    let notifier = Arc::new(
        Notifier::new(store.clone(), Sender::new(SenderConfig::default())?)
            .with_base_url(Some("http://vigil.test/".into()))
            .with_retry(retry)
            .collecting_reports(),
    );
    let checker = Checker::new(store.clone(), Fetcher::new(FetchConfig::default())?)
        .with_notifier(Arc::clone(&notifier));
    let ntfy = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&ntfy)
        .await;
    Ok(Setup {
        _dir: dir,
        store,
        checker,
        notifier,
        page: MockServer::start().await,
        ntfy,
    })
}

pub async fn setup() -> Result<Setup, Box<dyn std::error::Error>> {
    setup_with(quick_retries(Duration::from_millis(10))).await
}

pub async fn serve(page: &MockServer, body: &str) {
    page.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/html; charset=utf-8"))
        .mount(page)
        .await;
}

pub async fn add_target(
    s: &Setup,
    name: &str,
    topic: &str,
) -> Result<NotificationTarget, Box<dyn std::error::Error>> {
    let url = format!("ntfy://127.0.0.1:{}/{topic}", s.ntfy.address().port());
    let new = NewTarget {
        name: name.into(),
        url,
        default_template: None,
    };
    Ok(s.store.create_target(&new).await?)
}

/// A watch on the page server, linked to a new ntfy target named `phone` with topic `vigil-test`.
pub async fn watch_with_target(
    s: &Setup,
    edit: impl FnOnce(&mut NewWatch),
) -> Result<Watch, Box<dyn std::error::Error>> {
    let mut new = NewWatch::new(s.page.uri(), Schedule::Interval(3600));
    edit(&mut new);
    let watch = s.store.create_watch(new).await?;
    let target = add_target(s, "phone", "vigil-test").await?;
    s.store.link_target(watch.id, target.id).await?;
    Ok(watch)
}

/// Check the page as `v1`, then as `v2`, and return the second result.
pub async fn change(s: &Setup, watch: &Watch) -> Result<CheckResult, Box<dyn std::error::Error>> {
    serve(&s.page, PAGE_V1).await;
    let first = s.checker.check(watch.id).await?.result;
    assert!(matches!(first, CheckResult::Baseline), "{first:?}");
    serve(&s.page, PAGE_V2).await;
    Ok(s.checker.check(watch.id).await?.result)
}

pub async fn ntfy_bodies(s: &Setup) -> Result<Vec<Value>, serde_json::Error> {
    let requests = s.ntfy.received_requests().await.unwrap_or_default();
    requests
        .iter()
        .map(|r| serde_json::from_slice(&r.body))
        .collect()
}

pub async fn wait_until(what: &str, mut done: impl AsyncFnMut() -> bool) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done().await {
        if Instant::now() > deadline {
            return Err(format!("timed out waiting for {what}").into());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Ok(())
}
