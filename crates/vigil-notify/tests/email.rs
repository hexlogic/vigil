//! Email delivery against a local SMTP server.

mod support;

use std::time::Duration;

use support::smtp::{Rcpt, SmtpServer, start};
use tokio_util::sync::CancellationToken;
use vigil_notify::{DeliveryReport, Event, RetryPolicy, Sender, SenderConfig, deliver};

fn sender() -> Sender {
    let config = SenderConfig {
        timeout: Duration::from_millis(1500),
        ..SenderConfig::default()
    };
    match Sender::new(config) {
        Ok(sender) => sender,
        Err(error) => panic!("sender: {error}"),
    }
}

fn quick(retries: usize) -> RetryPolicy {
    RetryPolicy {
        delays: vec![Duration::from_millis(10); retries],
        max_retry_after: Duration::from_secs(1),
    }
}

async fn send(url: &str, event: &Event, retries: usize) -> DeliveryReport {
    let channel = support::channel(url);
    deliver(
        &sender(),
        &channel,
        &support::prepared(&channel, event),
        event,
        &quick(retries),
        &CancellationToken::new(),
    )
    .await
}

fn plain_url(server: &SmtpServer) -> String {
    format!(
        "mailto://127.0.0.1:{}?tls=none&from=vigil@x.test&to=a@b.test,c@d.test",
        server.port
    )
}

#[tokio::test]
async fn a_message_arrives_with_the_right_envelope_and_text() {
    let server = start(Rcpt::Accept).await;
    let report = send(&plain_url(&server), &Event::sample(), 0).await;
    assert_eq!((report.attempts, report.result), (1, Ok(())));

    let received = server.received();
    assert_eq!(received.len(), 1);
    let mail = &received[0];
    assert_eq!(mail.from, "vigil@x.test");
    assert_eq!(mail.rcpts, ["a@b.test", "c@d.test"]);
    assert!(mail.auth.is_none());
    assert!(
        mail.data.contains("Subject: Example product page changed"),
        "{}",
        mail.data
    );
    assert!(mail.data.contains("From: vigil@x.test"), "{}", mail.data);
    assert!(
        mail.data
            .to_lowercase()
            .contains("content-type: text/plain; charset=utf-8"),
        "{}",
        mail.data
    );
    assert!(
        mail.data.contains("Widget: $12") && mail.data.contains("Widget: $10"),
        "{}",
        mail.data
    );
    assert!(
        mail.data.contains("https://example.com/products/widget"),
        "{}",
        mail.data
    );
}

#[tokio::test]
async fn non_ascii_text_is_encoded_for_transport() {
    let server = start(Rcpt::Accept).await;
    let mut event = Event::sample();
    event.watch_name = "Ціна товару".into();
    event.added = vec!["Ціна: 12 ₴".into()];
    let report = send(&plain_url(&server), &event, 0).await;
    assert_eq!(report.result, Ok(()));

    let mail = &server.received()[0];
    assert!(mail.data.contains("Subject: =?utf-8?"), "{}", mail.data);
    let data = mail.data.to_lowercase();
    assert!(data.contains("content-transfer-encoding:"), "{}", mail.data);
    assert!(
        !data.contains("content-transfer-encoding: 7bit"),
        "{}",
        mail.data
    );
}

#[tokio::test]
async fn credentials_are_sent_and_the_user_name_is_the_default_sender() {
    let server = start(Rcpt::Accept).await;
    let url = format!("mailto://me%40x.test:pw@127.0.0.1:{}?tls=none", server.port);
    let report = send(&url, &Event::sample(), 0).await;
    assert_eq!(report.result, Ok(()));

    let mail = &server.received()[0];
    assert_eq!(mail.auth, Some(("me@x.test".into(), "pw".into())));
    assert_eq!(mail.from, "me@x.test");
    assert_eq!(mail.rcpts, ["me@x.test"]);
}

#[tokio::test]
async fn a_temporary_rejection_is_retried() {
    let server = start(Rcpt::TemporaryFailures(1)).await;
    let report = send(&plain_url(&server), &Event::sample(), 3).await;
    assert_eq!((report.attempts, report.result), (2, Ok(())));
    assert_eq!(server.connections(), 2);
    assert_eq!(server.received().len(), 1);
}

#[tokio::test]
async fn a_permanent_rejection_is_not_retried() {
    let server = start(Rcpt::Reject).await;
    let report = send(&plain_url(&server), &Event::sample(), 3).await;
    assert_eq!(report.attempts, 1);
    let error = report.result.expect_err("rejected");
    assert!(error.contains("550"), "{error}");
    assert!(server.received().is_empty());
}

#[tokio::test]
async fn starttls_is_required_unless_the_url_says_otherwise() {
    // The server does not offer STARTTLS, and the URL does not say tls=none.
    let server = start(Rcpt::Accept).await;
    let url = format!(
        "mailto://me%40x.test:S3CRETSMTP@127.0.0.1:{}?from=vigil@x.test",
        server.port
    );
    let report = send(&url, &Event::sample(), 3).await;
    assert_eq!(
        report.attempts, 1,
        "a missing STARTTLS is a configuration problem"
    );
    let error = report.result.expect_err("refused");
    assert!(!error.contains("S3CRETSMTP"), "{error}");
    assert!(
        server.received().is_empty(),
        "nothing may be sent in the clear"
    );
}

#[tokio::test]
async fn implicit_tls_against_a_plaintext_server_fails_without_sending() {
    let server = start(Rcpt::Accept).await;
    let url = format!("mailtos://127.0.0.1:{}?from=vigil@x.test", server.port);
    let report = send(&url, &Event::sample(), 0).await;
    assert!(report.result.is_err());
    assert!(server.received().is_empty());
}

#[tokio::test]
async fn an_ipv6_server_is_reached_without_brackets() {
    use support::smtp::start_on;
    let Some(server) = start_on("[::1]:0", Rcpt::Accept).await else {
        return; // this machine has no IPv6 loopback
    };
    let url = format!(
        "mailto://[::1]:{}?tls=none&from=vigil@x.test&to=a@b.test",
        server.port
    );
    let report = send(&url, &Event::sample(), 0).await;
    assert_eq!((report.attempts, report.result), (1, Ok(())));
    assert_eq!(server.received().len(), 1);
}
