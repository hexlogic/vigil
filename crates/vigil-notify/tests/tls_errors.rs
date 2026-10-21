//! Failures that retrying cannot fix are not retried.

mod support;

use std::time::Duration;

use support::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use vigil_notify::{Event, RetryPolicy, deliver};
use wiremock::MockServer;

/// A server that is not speaking TLS: it answers the first bytes with plain HTTP.
async fn plain_http_server() -> u16 {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => panic!("bind: {error}"),
    };
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(error) => panic!("addr: {error}"),
    };
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buffer = [0u8; 512];
                let _ = stream.read(&mut buffer).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
                    .await;
            });
        }
    });
    port
}

#[tokio::test]
async fn https_to_a_plain_http_port_fails_at_once() {
    let port = plain_http_server().await;
    let server = MockServer::start().await;
    let channel = channel(&format!("ntfys://127.0.0.1:{port}/alerts"));
    let event = Event::sample();
    let policy = RetryPolicy {
        delays: vec![Duration::from_millis(10); 3],
        max_retry_after: Duration::from_secs(1),
    };
    let report = deliver(
        &sender(&server),
        &channel,
        &prepared(&channel, &event),
        &event,
        &policy,
        &CancellationToken::new(),
    )
    .await;
    assert_eq!(report.attempts, 1, "{:?}", report.result);
    let error = report.result.expect_err("failed");
    assert!(error.contains("corrupt message"), "{error}");
}
