//! Linking watches to notification targets, and notifying from `vigil check`.

mod common;

use std::path::Path;

use common::*;
use vigil_store::Store;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

struct World {
    page: MockServer,
    ntfy: MockServer,
    id: String,
}

/// A watch on a mock page, linked to a target named `phone` that posts to a mock ntfy server
/// answering `ntfy_status`. The page is at version 1 and has had its baseline check.
async fn world(dir: &Path, ntfy_status: u16) -> Result<World, Box<dyn std::error::Error>> {
    let page = MockServer::start().await;
    serve_page(&page, V1).await;
    let ntfy = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(ntfy_status))
        .mount(&ntfy)
        .await;
    let url = format!("ntfy://127.0.0.1:{}/alerts", ntfy.address().port());
    let added = vigil(dir, &["notify", "add", "phone", &url]).await?;
    assert!(added.ok, "{}", added.stderr);

    let run = vigil(
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
    assert!(run.ok, "{}", run.stderr);
    assert!(run.stdout.contains("notifies phone"), "{}", run.stdout);
    let id = run
        .stdout
        .split_whitespace()
        .nth(1)
        .ok_or("no id")?
        .to_owned();
    let baseline = vigil(dir, &["check", &id]).await?;
    assert!(
        baseline.ok && baseline.stdout.contains("baseline"),
        "{}",
        baseline.stdout
    );
    Ok(World { page, ntfy, id })
}

async fn show_version(page: &MockServer, body: &str) {
    page.reset().await;
    serve_page(page, body).await;
}

async fn ntfy_requests(world: &World) -> usize {
    world
        .ntfy
        .received_requests()
        .await
        .unwrap_or_default()
        .len()
}

#[tokio::test(flavor = "multi_thread")]
async fn vigil_check_notifies_linked_targets_and_waits_for_them() -> TestResult {
    let dir = tempfile::tempdir()?;
    let w = world(dir.path(), 200).await?;
    assert_eq!(ntfy_requests(&w).await, 0, "a baseline notifies nobody");

    show_version(&w.page, V2).await;
    let run = vigil(dir.path(), &["check", &w.id]).await?;
    assert!(run.ok, "{}", run.stderr);
    assert!(
        run.stdout.contains("changed") && run.stdout.contains("notified phone"),
        "{}",
        run.stdout
    );
    assert_eq!(ntfy_requests(&w).await, 1);

    let bodies = w.ntfy.received_requests().await.unwrap_or_default();
    let body: serde_json::Value = serde_json::from_slice(&bodies[0].body)?;
    assert_eq!(body["topic"], "alerts");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("Widget: $12")),
        "{body}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn no_notify_sends_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let w = world(dir.path(), 200).await?;
    show_version(&w.page, V2).await;
    let run = vigil(dir.path(), &["check", &w.id, "--no-notify"]).await?;
    assert!(
        run.ok && run.stdout.contains("changed") && !run.stdout.contains("notified"),
        "{}",
        run.stdout
    );
    assert_eq!(ntfy_requests(&w).await, 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn targets_can_be_unlinked_and_linked_again() -> TestResult {
    let dir = tempfile::tempdir()?;
    let w = world(dir.path(), 200).await?;

    let unlink = vigil(
        dir.path(),
        &["watch", "edit", &w.id, "--no-notify", "phone"],
    )
    .await?;
    assert!(unlink.ok, "{}", unlink.stderr);
    show_version(&w.page, V2).await;
    assert!(vigil(dir.path(), &["check", &w.id]).await?.ok);
    assert_eq!(ntfy_requests(&w).await, 0);

    let link = vigil(dir.path(), &["watch", "edit", &w.id, "--notify", "phone"]).await?;
    assert!(link.ok, "{}", link.stderr);
    show_version(&w.page, V1).await;
    assert!(vigil(dir.path(), &["check", &w.id]).await?.ok);
    assert_eq!(ntfy_requests(&w).await, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn linking_is_not_a_settings_edit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let w = world(dir.path(), 200).await?;
    let other = vigil(dir.path(), &["notify", "add", "chat", "ntfy://h/other"]).await?;
    assert!(other.ok);

    let store = Store::open(&dir.path().join("data").join("vigil.db")).await?;
    let id = store.resolve_watch_id(&w.id).await?;
    let before = store.get_watch(id).await?;
    assert!(
        vigil(dir.path(), &["watch", "edit", &w.id, "--notify", "chat"])
            .await?
            .ok
    );
    assert!(
        vigil(
            dir.path(),
            &[
                "watch",
                "edit",
                &w.id,
                "--notify-template",
                "{{ watch_name }}"
            ]
        )
        .await?
        .ok
    );
    let after = store.get_watch(id).await?;
    assert_eq!(after.updated_at, before.updated_at);
    assert_eq!(after.state, before.state);

    assert!(
        vigil(dir.path(), &["watch", "edit", &w.id, "--name", "Renamed"])
            .await?
            .ok
    );
    assert!(store.get_watch(id).await?.updated_at > before.updated_at);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_targets_and_bad_templates_are_refused_before_anything_changes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let page = MockServer::start().await;
    let url = page.uri();

    let unknown = vigil(dir.path(), &["watch", "add", &url, "--notify", "nope"]).await?;
    assert!(
        !unknown.ok && unknown.stderr.contains("not found"),
        "{}",
        unknown.stderr
    );
    let bad = vigil(
        dir.path(),
        &["watch", "add", &url, "--notify-template", "{{ nope }}"],
    )
    .await?;
    assert!(
        !bad.ok && bad.stderr.contains("invalid --notify-template"),
        "{}",
        bad.stderr
    );
    assert!(
        vigil(dir.path(), &["watch", "list"])
            .await?
            .stdout
            .contains("no watches yet")
    );

    let w = world(dir.path(), 200).await?;
    let edit = vigil(
        dir.path(),
        &["watch", "edit", &w.id, "--every", "10m", "--notify", "nope"],
    )
    .await?;
    assert!(
        !edit.ok && edit.stderr.contains("not found"),
        "{}",
        edit.stderr
    );
    let shown = vigil(dir.path(), &["watch", "show", &w.id]).await?;
    let json: serde_json::Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        json["schedule"],
        serde_json::json!({"interval": 300}),
        "the edit was not applied"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn show_lists_the_targets_and_the_template() -> TestResult {
    let dir = tempfile::tempdir()?;
    let w = world(dir.path(), 200).await?;
    let show = |dir: &Path| {
        let dir = dir.to_path_buf();
        let id = w.id.clone();
        async move {
            let run = vigil(&dir, &["watch", "show", &id]).await?;
            Ok::<_, Box<dyn std::error::Error>>(serde_json::from_str::<serde_json::Value>(
                &run.stdout,
            )?)
        }
    };
    let json = show(dir.path()).await?;
    assert_eq!(json["notify"], serde_json::json!(["phone"]));
    assert_eq!(json["notification_template"], serde_json::Value::Null);

    assert!(
        vigil(
            dir.path(),
            &[
                "watch",
                "edit",
                &w.id,
                "--notify-template",
                "{{ watch_name }} moved"
            ]
        )
        .await?
        .ok
    );
    assert_eq!(
        show(dir.path()).await?["notification_template"],
        "{{ watch_name }} moved"
    );
    assert!(
        vigil(
            dir.path(),
            &["watch", "edit", &w.id, "--notify-template", ""]
        )
        .await?
        .ok
    );
    assert_eq!(
        show(dir.path()).await?["notification_template"],
        serde_json::Value::Null
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_delivery_is_reported_but_does_not_fail_the_check() -> TestResult {
    let dir = tempfile::tempdir()?;
    // 401 is a refusal, so there are no retries to wait for.
    let w = world(dir.path(), 401).await?;
    show_version(&w.page, V2).await;
    let run = vigil(dir.path(), &["check", &w.id]).await?;
    assert!(run.ok, "the page was checked fine: {}", run.stderr);
    assert!(run.stdout.contains("changed"), "{}", run.stdout);
    assert!(
        run.stderr.contains("could not notify phone: HTTP 401"),
        "{}",
        run.stderr
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn editing_nothing_names_every_option() -> TestResult {
    let dir = tempfile::tempdir()?;
    let w = world(dir.path(), 200).await?;
    let run = vigil(dir.path(), &["watch", "edit", &w.id]).await?;
    assert!(!run.ok && run.stderr.contains("--notify"), "{}", run.stderr);
    Ok(())
}

/// A running `vigil serve`, killed when dropped so a failing test leaves nothing behind.
struct Server(std::process::Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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

    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_vigil"));
    command
        .arg("serve")
        .current_dir(dir.path())
        .env("VIGIL_DATA_DIR", dir.path().join("data"))
        .env("VIGIL_LISTEN", "127.0.0.1:0")
        .env_remove("VIGIL_CONFIG")
        .env_remove("VIGIL_BASE_URL")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for var in PROXY_VARS {
        command.env_remove(var);
    }
    let _server = Server(command.spawn()?);

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
