//! `vigil serve` sends notifications: the ones it finds, and the ones left from the last run.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::*;
use vigil_store::{NewPending, Store};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A running `vigil serve`, killed when dropped so a failing test leaves nothing behind.
struct Server(std::process::Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Start `vigil serve` on a free port with its data in `dir`.
fn start_server(dir: &Path) -> std::io::Result<Server> {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_vigil"));
    command
        .arg("serve")
        .current_dir(dir)
        .env("VIGIL_DATA_DIR", dir.join("data"))
        .env("VIGIL_LISTEN", "127.0.0.1:0")
        .env_remove("VIGIL_CONFIG")
        .env_remove("VIGIL_BASE_URL")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for var in PROXY_VARS {
        command.env_remove(var);
    }
    command.spawn().map(Server)
}

#[tokio::test(flavor = "multi_thread")]
async fn vigil_serve_notifies_on_its_own() -> TestResult {
    let dir = tempfile::tempdir()?;
    let page = MockServer::start().await;
    // The first request sees version 1, every later one version 2.
    let html = |body: &'static str| ResponseTemplate::new(200).set_body_raw(body, "text/html");
    Mock::given(method("GET"))
        .respond_with(html(V1))
        .up_to_n_times(1)
        .mount(&page)
        .await;
    Mock::given(method("GET"))
        .respond_with(html(V2))
        .mount(&page)
        .await;
    let ntfy = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&ntfy)
        .await;

    let url = format!("ntfy://127.0.0.1:{}/alerts", ntfy.address().port());
    assert!(
        vigil(dir.path(), &["notify", "add", "phone", &url])
            .await?
            .ok
    );
    let added = vigil(
        dir.path(),
        &[
            "watch",
            "add",
            &page.uri(),
            "--every",
            "1s",
            "--notify",
            "phone",
        ],
    )
    .await?;
    assert!(added.ok, "{}", added.stderr);

    let _server = start_server(dir.path())?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let received = ntfy.received_requests().await.unwrap_or_default();
        if !received.is_empty() {
            assert_eq!(received.len(), 1, "later checks see no further change");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no notification arrived in 30 seconds"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Ok(())
}

/// What [`with_a_saved_notification`] set up.
struct Saved {
    /// The watched page; kept so that it stays up.
    _page: MockServer,
    ntfy: MockServer,
    store: Store,
    row: i64,
}

/// A watch linked to an ntfy target, checked once and then due only in five minutes, so that
/// a server started now finds no change of its own, plus a saved notification for it in the
/// outbox: what a process that died in the middle of a delivery leaves behind.
async fn with_a_saved_notification(dir: &Path) -> Result<Saved, Box<dyn std::error::Error>> {
    let page = MockServer::start().await;
    serve_page(&page, V1).await;
    let ntfy = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&ntfy)
        .await;
    let url = format!("ntfy://127.0.0.1:{}/alerts", ntfy.address().port());
    assert!(vigil(dir, &["notify", "add", "phone", &url]).await?.ok);
    let added = vigil(
        dir,
        &[
            "watch",
            "add",
            &page.uri(),
            "--every",
            "5m",
            "--notify",
            "phone",
        ],
    )
    .await?;
    assert!(added.ok, "{}", added.stderr);
    let id = added
        .stdout
        .split_whitespace()
        .nth(1)
        .ok_or("no id")?
        .to_owned();
    assert!(vigil(dir, &["check", &id]).await?.ok);

    let store = Store::open(&dir.join("data").join("vigil.db")).await?;
    let watch = store.resolve_watch_id(&id).await?;
    let target = store.targets_for_watch(watch, false).await?.remove(0);
    let snapshot = store.list_snapshots(watch, 1).await?.remove(0).id;
    let mut event = vigil_notify::Event::sample();
    event.watch_name = "Left behind".into();
    let row = store
        .enqueue_notification(&NewPending {
            watch,
            target: target.id,
            snapshot,
            payload: event.to_json()?,
        })
        .await?;
    Ok(Saved {
        _page: page,
        ntfy,
        store,
        row,
    })
}

/// Wait for the first request to the ntfy server and check it is the saved notification.
async fn expect_the_saved_notification(ntfy: &MockServer) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let received = ntfy.received_requests().await.unwrap_or_default();
        if let Some(request) = received.first() {
            let body: serde_json::Value = serde_json::from_slice(&request.body)?;
            assert_eq!(body["title"], "Left behind changed");
            assert_eq!(received.len(), 1);
            return Ok(());
        }
        assert!(
            Instant::now() < deadline,
            "the saved notification was not sent"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_an_empty_outbox(store: &Store) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !store.pending_notifications().await?.is_empty() {
        assert!(Instant::now() < deadline, "the outbox was not emptied");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn vigil_serve_sends_what_the_last_run_left_behind() -> TestResult {
    let dir = tempfile::tempdir()?;
    let saved = with_a_saved_notification(dir.path()).await?;
    let _server = start_server(dir.path())?;
    expect_the_saved_notification(&saved.ntfy).await?;
    wait_for_an_empty_outbox(&saved.store).await
}

#[tokio::test(flavor = "multi_thread")]
async fn vigil_serve_retries_a_notification_that_was_waiting() -> TestResult {
    let dir = tempfile::tempdir()?;
    let saved = with_a_saved_notification(dir.path()).await?;
    // Its quick retries ran out in an earlier run, and the wait is over.
    saved
        .store
        .defer_notification(saved.row, vigil_store::now_ms() - 1000, 1)
        .await?;
    let _server = start_server(dir.path())?;
    expect_the_saved_notification(&saved.ntfy).await?;
    wait_for_an_empty_outbox(&saved.store).await
}
