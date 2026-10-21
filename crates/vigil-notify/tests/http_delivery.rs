//! Retries, and that failures never leak credentials.

mod support;

use std::time::{Duration, Instant};

use support::*;
use tokio_util::sync::CancellationToken;
use vigil_notify::{Event, RetryPolicy, deliver};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn quick() -> RetryPolicy {
    RetryPolicy {
        delays: vec![Duration::from_millis(10); 3],
        max_retry_after: Duration::from_secs(60),
    }
}

async fn run(server: &MockServer, url: &str, policy: &RetryPolicy) -> vigil_notify::DeliveryReport {
    let channel = channel(url);
    let event = Event::sample();
    deliver(
        &sender(server),
        &channel,
        &prepared(&channel, &event),
        &event,
        policy,
        &CancellationToken::new(),
    )
    .await
}

fn ntfy_url(server: &MockServer) -> String {
    format!("ntfy://127.0.0.1:{}/alerts", port(server))
}

#[tokio::test]
async fn a_server_error_is_retried_until_it_works() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let report = run(&server, &ntfy_url(&server), &quick()).await;
    assert_eq!((report.attempts, report.result), (3, Ok(())));
}

#[tokio::test]
async fn a_refused_request_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .mount(&server)
        .await;

    let report = run(&server, &ntfy_url(&server), &quick()).await;
    assert_eq!(report.attempts, 1);
    assert_eq!(
        report.result,
        Err("HTTP 401 Unauthorized: unauthorized".to_owned())
    );
}

#[tokio::test]
async fn a_server_that_stays_down_gets_four_attempts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let report = run(&server, &ntfy_url(&server), &quick()).await;
    assert_eq!(report.attempts, 4);
    assert!(
        !report.interrupted,
        "running out of retries is not an interruption"
    );
    let error = report.result.expect_err("failed");
    assert!(error.starts_with("HTTP 503"), "{error}");
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        4
    );
}

#[tokio::test]
async fn too_many_requests_is_retried_and_retry_after_is_honoured() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "1"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    // The policy's own delay is 10 ms; the server asked for a second.
    let started = Instant::now();
    let report = run(&server, &ntfy_url(&server), &quick()).await;
    assert_eq!((report.attempts, report.result), (2, Ok(())));
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn retry_after_is_capped() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "3600"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let policy = RetryPolicy {
        max_retry_after: Duration::from_millis(20),
        ..quick()
    };
    let started = Instant::now();
    let report = run(&server, &ntfy_url(&server), &policy).await;
    assert_eq!(report.result, Ok(()));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn cancelling_stops_the_wait_between_attempts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let channel = channel(&ntfy_url(&server));
    let event = Event::sample();
    let policy = RetryPolicy {
        delays: vec![Duration::from_secs(60)],
        ..quick()
    };
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        stopper.cancel();
    });

    let started = Instant::now();
    let report = deliver(
        &sender(&server),
        &channel,
        &prepared(&channel, &event),
        &event,
        &policy,
        &cancel,
    )
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(report.attempts, 1);
    assert!(report.interrupted, "a cut-short delivery says so");
    let error = report.result.expect_err("failed");
    assert!(
        error.contains("HTTP 500") && error.contains("shutting down"),
        "{error}"
    );
}

#[tokio::test]
async fn a_closed_port_fails_without_naming_the_url() {
    // Reserve a port, then close it so that connecting is refused.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr").port()
    };
    let server = MockServer::start().await;
    let url = format!("json://me:S3CRETPW@127.0.0.1:{port}/hooks/S3CRETPATH?key=S3CRETQ");
    let report = run(&server, &url, &quick()).await;

    assert_eq!(report.attempts, 4, "connection errors are retried");
    let error = report.result.expect_err("failed");
    assert!(
        !error.contains("S3CRET") && !error.contains("hooks"),
        "{error}"
    );
    assert!(
        !error.contains("127.0.0.1") && !error.contains(&port.to_string()),
        "{error}"
    );
}

#[tokio::test]
async fn a_reply_that_echoes_the_path_is_scrubbed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hooks/S3CRETPATH"))
        .respond_with(ResponseTemplate::new(404).set_body_string("Cannot POST /hooks/S3CRETPATH"))
        .mount(&server)
        .await;

    let url = format!("json://127.0.0.1:{}/hooks/S3CRETPATH", port(&server));
    let report = run(&server, &url, &quick()).await;
    let error = report.result.expect_err("failed");
    assert_eq!(error, "HTTP 404 Not Found: Cannot POST ***");
    assert_eq!(report.attempts, 1);
}

#[tokio::test]
async fn a_telegram_token_does_not_survive_into_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(404).set_body_string(
            r#"{"ok":false,"description":"Not Found: /bot123456:S3CRETBOT/sendMessage"}"#,
        ))
        .mount(&server)
        .await;

    let report = run(&server, "tgram://123456:S3CRETBOT/-100123", &quick()).await;
    let error = report.result.expect_err("failed");
    assert!(
        error.starts_with("HTTP 404") && !error.contains("S3CRET"),
        "{error}"
    );
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(307).insert_header("location", "http://127.0.0.1:1/elsewhere"),
        )
        .mount(&server)
        .await;

    let report = run(&server, &ntfy_url(&server), &quick()).await;
    assert_eq!(report.attempts, 1);
    assert!(report.result.expect_err("failed").starts_with("HTTP 307"));
}

#[tokio::test]
async fn a_slow_server_times_out() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;

    let channel = channel(&ntfy_url(&server));
    let event = Event::sample();
    let sender = vigil_notify::Sender::new(vigil_notify::SenderConfig {
        timeout: Duration::from_millis(200),
        ..vigil_notify::SenderConfig::default()
    })
    .expect("sender");
    let policy = RetryPolicy {
        delays: vec![],
        ..quick()
    };
    let report = deliver(
        &sender,
        &channel,
        &prepared(&channel, &event),
        &event,
        &policy,
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(report.result, Err("timed out".to_owned()));
}

#[tokio::test]
async fn an_ntfy_topic_echoed_by_the_server_is_scrubbed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(400).set_body_string(r#"{"error":"bad topic S3CRETTOPIC"}"#),
        )
        .mount(&server)
        .await;

    let url = format!("ntfy://127.0.0.1:{}/S3CRETTOPIC", port(&server));
    let report = run(&server, &url, &quick()).await;
    let error = report.result.expect_err("failed");
    assert!(
        error.starts_with("HTTP 400") && !error.contains("S3CRET"),
        "{error}"
    );
}
