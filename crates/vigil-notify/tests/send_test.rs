//! Test notifications.

mod support;

use std::time::Duration;

use support::*;
use vigil_notify::{RetryPolicy, send_test};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn quick() -> RetryPolicy {
    RetryPolicy {
        delays: vec![Duration::from_millis(10); 3],
        max_retry_after: Duration::from_secs(1),
    }
}

fn ntfy_url(server: &MockServer) -> String {
    format!("ntfy://127.0.0.1:{}/alerts", port(server))
}

#[tokio::test]
async fn a_test_notification_says_it_is_a_test() {
    let server = accepting_server().await;
    let channel = channel(&ntfy_url(&server));
    let report = send_test(&sender(&server), &channel, None, None, &quick()).await;
    assert_eq!(
        (report.attempts, report.result, report.note),
        (1, Ok(()), None)
    );

    let body = json_body(&only_request(&server).await);
    assert_eq!(body["topic"], "alerts");
    assert_eq!(body["title"], "Vigil test notification");
    let message = body["message"].as_str().expect("message");
    assert!(
        message.contains("test notification from Vigil"),
        "{message}"
    );
    assert!(body.get("click").is_none(), "no diff page to link to");
}

#[tokio::test]
async fn the_address_of_vigil_is_included_when_known() {
    let server = accepting_server().await;
    let channel = channel(&ntfy_url(&server));
    let base = Some("https://vigil.example.com");
    send_test(&sender(&server), &channel, None, base, &quick()).await;

    let body = json_body(&only_request(&server).await);
    let message = body["message"].as_str().expect("message");
    assert!(
        message.starts_with("https://vigil.example.com"),
        "{message}"
    );
}

#[tokio::test]
async fn a_custom_template_is_tried_and_a_broken_one_is_reported() {
    let server = accepting_server().await;
    let channel = channel(&ntfy_url(&server));
    let ok = send_test(
        &sender(&server),
        &channel,
        Some("custom: {{ watch_name }}"),
        None,
        &quick(),
    )
    .await;
    assert_eq!((ok.result, ok.note), (Ok(()), None));

    let broken = send_test(
        &sender(&server),
        &channel,
        Some("{{ nope }}"),
        None,
        &quick(),
    )
    .await;
    assert_eq!(
        broken.result,
        Ok(()),
        "still delivered, with the built-in template"
    );
    let note = broken.note.expect("a note about the template");
    assert!(
        note.contains("custom template") && note.contains("built-in"),
        "{note}"
    );

    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        json_body(&requests[0])["message"],
        "custom: Vigil test notification"
    );
}

#[tokio::test]
async fn a_target_that_does_not_work_reports_why() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403).set_body_string("no access"))
        .mount(&server)
        .await;
    let channel = channel(&ntfy_url(&server));
    let report = send_test(&sender(&server), &channel, None, None, &quick()).await;
    assert_eq!(report.attempts, 1);
    assert_eq!(
        report.result,
        Err("HTTP 403 Forbidden: no access".to_owned())
    );
}
