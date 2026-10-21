//! `vigil notify`: managing and testing notification targets from the command line.

mod common;

use common::*;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn ntfy_server(status: u16) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(status))
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread")]
async fn targets_can_be_added_listed_and_removed_with_their_secrets_hidden() -> TestResult {
    let dir = tempfile::tempdir()?;
    let add = vigil(
        dir.path(),
        &["notify", "add", "phone", "ntfy://127.0.0.1:1/S3CRETTOPIC"],
    )
    .await?;
    assert!(add.ok, "{}", add.stderr);
    assert!(
        add.stdout.contains("phone (ntfy://127.0.0.1:1/***)"),
        "{}",
        add.stdout
    );
    assert!(!add.stdout.contains("S3CRET"));

    let tgram = vigil(
        dir.path(),
        &["notify", "add", "chat", "tgram://123456:S3CRETBOT/-100123"],
    )
    .await?;
    assert!(tgram.ok, "{}", tgram.stderr);

    let list = vigil(dir.path(), &["notify", "list"]).await?;
    assert!(list.ok);
    assert!(!list.stdout.contains("S3CRET"), "{}", list.stdout);
    assert!(
        list.stdout.contains("tgram://***/-100123"),
        "{}",
        list.stdout
    );
    assert!(
        list.stdout.contains("ntfy://127.0.0.1:1/***"),
        "{}",
        list.stdout
    );
    let phone = list
        .stdout
        .lines()
        .find(|l| l.contains("phone"))
        .ok_or("no phone row")?;
    assert!(
        phone.contains("ntfy") && phone.contains("enabled") && phone.trim_end().ends_with('0'),
        "{phone}"
    );

    let remove = vigil(dir.path(), &["notify", "remove", "phone"]).await?;
    assert!(
        remove.ok && remove.stdout.starts_with("removed "),
        "{}{}",
        remove.stdout,
        remove.stderr
    );
    let list = vigil(dir.path(), &["notify", "list"]).await?;
    assert!(!list.stdout.contains("phone") && list.stdout.contains("chat"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_list_says_how_to_add_one() -> TestResult {
    let dir = tempfile::tempdir()?;
    let list = vigil(dir.path(), &["notify", "list"]).await?;
    assert!(
        list.ok && list.stdout.contains("vigil notify add"),
        "{}",
        list.stdout
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bad_url_is_refused_without_repeating_it() -> TestResult {
    let dir = tempfile::tempdir()?;
    for (url, expected) in [
        (
            "matrix://user:S3CRETPW@matrix.org/room",
            "not supported yet",
        ),
        ("tgram://123456:S3CRETBOT", "expected tgram://"),
        (
            "mailto://u%40x.test:S3CRETPW@mail.example.com?tls=none",
            "without TLS",
        ),
        ("S3CRET-no-scheme", "not a notification URL"),
    ] {
        let run = vigil(dir.path(), &["notify", "add", "x", url]).await?;
        assert!(!run.ok, "{url}");
        assert!(run.stderr.contains(expected), "{url}: {}", run.stderr);
        assert!(
            !run.stderr.contains("S3CRET") && !run.stdout.contains("S3CRET"),
            "{url}: {}",
            run.stderr
        );
    }
    assert!(
        vigil(dir.path(), &["notify", "list"])
            .await?
            .stdout
            .contains("no notification targets")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_can_only_be_used_once() -> TestResult {
    let dir = tempfile::tempdir()?;
    assert!(
        vigil(dir.path(), &["notify", "add", "phone", "ntfy://h/t"])
            .await?
            .ok
    );
    let again = vigil(dir.path(), &["notify", "add", "phone", "ntfy://h/other"]).await?;
    assert!(
        !again.ok && again.stderr.contains("already exists"),
        "{}",
        again.stderr
    );
    let blank = vigil(dir.path(), &["notify", "add", "  ", "ntfy://h/other"]).await?;
    assert!(
        !blank.ok && blank.stderr.contains("needs a name"),
        "{}",
        blank.stderr
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn templates_are_checked_when_saved() -> TestResult {
    let dir = tempfile::tempdir()?;
    let bad = vigil(
        dir.path(),
        &[
            "notify",
            "add",
            "x",
            "ntfy://h/t",
            "--template",
            "{{ no_such_variable }}",
        ],
    )
    .await?;
    assert!(
        !bad.ok && bad.stderr.contains("invalid --template"),
        "{}",
        bad.stderr
    );

    let file = dir.path().join("template.txt");
    std::fs::write(&file, "{{ watch_name }} moved\n")?;
    let from_file = format!("@{}", file.display());
    let good = vigil(
        dir.path(),
        &["notify", "add", "x", "ntfy://h/t", "--template", &from_file],
    )
    .await?;
    assert!(good.ok, "{}", good.stderr);
    let missing = vigil(
        dir.path(),
        &[
            "notify",
            "add",
            "y",
            "ntfy://h/t",
            "--template",
            "@/nonexistent.txt",
        ],
    )
    .await?;
    assert!(
        !missing.ok && missing.stderr.contains("reading /nonexistent.txt"),
        "{}",
        missing.stderr
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn plain_http_secrets_get_a_warning() -> TestResult {
    let dir = tempfile::tempdir()?;
    let risky = vigil(
        dir.path(),
        &["notify", "add", "a", "ntfy://me:pw@ntfy.lan/t"],
    )
    .await?;
    assert!(
        risky.ok && risky.stderr.contains("warning"),
        "{}",
        risky.stderr
    );
    // A topic or a webhook path is a secret too.
    for (name, url) in [
        ("t", "ntfy://ntfy.sh/my-secret-topic"),
        ("w", "json://hooks.lan/in/tok"),
    ] {
        let run = vigil(dir.path(), &["notify", "add", name, url]).await?;
        assert!(
            run.ok && run.stderr.contains("warning"),
            "{url}: {}",
            run.stderr
        );
    }
    for (name, url) in [
        ("b", "ntfys://me:pw@ntfy.lan/t"),
        ("c", "ntfy://me:pw@127.0.0.1/t"),
        ("d", "ntfys://ntfy.lan/t"),
        ("e", "json://127.0.0.1/hooks/tok"),
    ] {
        let run = vigil(dir.path(), &["notify", "add", name, url]).await?;
        assert!(
            run.ok && !run.stderr.contains("warning"),
            "{url}: {}",
            run.stderr
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_test_notification_reaches_the_server() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = ntfy_server(200).await;
    let url = format!("ntfy://127.0.0.1:{}/alerts", server.address().port());
    assert!(
        vigil(dir.path(), &["notify", "add", "phone", &url])
            .await?
            .ok
    );

    let run = vigil(dir.path(), &["notify", "test", "phone"]).await?;
    assert!(run.ok, "{}", run.stderr);
    assert!(
        run.stdout.contains("sent a test notification to phone"),
        "{}",
        run.stdout
    );
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body)?;
    assert_eq!(body["topic"], "alerts");
    assert_eq!(body["title"], "Vigil test notification");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_test_that_fails_says_why_and_does_not_retry() -> TestResult {
    let dir = tempfile::tempdir()?;
    let server = ntfy_server(500).await;
    let url = format!("ntfy://127.0.0.1:{}/alerts", server.address().port());
    assert!(
        vigil(dir.path(), &["notify", "add", "phone", &url])
            .await?
            .ok
    );

    let run = vigil(dir.path(), &["notify", "test", "phone"]).await?;
    assert!(!run.ok);
    assert!(
        run.stderr.contains("could not notify phone: HTTP 500"),
        "{}",
        run.stderr
    );
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        1
    );

    let missing = vigil(dir.path(), &["notify", "test", "nope"]).await?;
    assert!(
        !missing.ok && missing.stderr.contains("not found"),
        "{}",
        missing.stderr
    );
    Ok(())
}
