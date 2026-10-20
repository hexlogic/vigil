//! What each HTTP channel sends, checked against a mock server.

mod support;

use serde_json::json;
use support::*;

#[tokio::test]
async fn ntfy_posts_json_to_the_server_root() {
    let server = accepting_server().await;
    let url = format!(
        "ntfy://127.0.0.1:{}/alerts?priority=high&tags=eyes,warning",
        port(&server)
    );
    send_once(&server, &url).await.expect("sent");

    let request = only_request(&server).await;
    assert_eq!((request.method.as_str(), request.url.path()), ("POST", "/"));
    assert_eq!(header(&request, "content-type"), "application/json");
    let body = json_body(&request);
    assert_eq!(body["topic"], "alerts");
    assert_eq!(body["title"], "Example product page changed");
    assert_eq!(body["priority"], 4);
    assert_eq!(body["tags"], json!(["eyes", "warning"]));
    assert!(
        body["click"]
            .as_str()
            .is_some_and(|c| c.contains("/diff?from=a&to=b"))
    );
    let message = body["message"].as_str().expect("message");
    assert!(
        message.contains("Widget: $12") && message.contains("Widget: $10"),
        "{message}"
    );
}

#[tokio::test]
async fn ntfy_under_a_path_prefix_with_basic_auth() {
    let server = accepting_server().await;
    let url = format!("ntfy://me:pw@127.0.0.1:{}/team/alerts", port(&server));
    send_once(&server, &url).await.expect("sent");

    let request = only_request(&server).await;
    assert_eq!(request.url.path(), "/team/");
    assert_eq!(header(&request, "authorization"), "Basic bWU6cHc=");
}

#[tokio::test]
async fn ntfy_with_a_bearer_token() {
    let server = accepting_server().await;
    let url = format!("ntfy://127.0.0.1:{}/alerts?token=tk_secret", port(&server));
    send_once(&server, &url).await.expect("sent");
    assert_eq!(
        header(&only_request(&server).await, "authorization"),
        "Bearer tk_secret"
    );
}

#[tokio::test]
async fn telegram_sends_html_to_the_bot_api() {
    let server = accepting_server().await;
    send_once(&server, "tgram://123456:ABC-def/-100123")
        .await
        .expect("sent");

    let request = only_request(&server).await;
    assert_eq!(request.url.path(), "/bot123456:ABC-def/sendMessage");
    let body = json_body(&request);
    assert_eq!(body["chat_id"], -100123);
    assert_eq!(body["parse_mode"], "HTML");
    assert_eq!(body["link_preview_options"]["is_disabled"], true);
    let text = body["text"].as_str().expect("text");
    assert!(
        text.starts_with("<b>Example product page</b> changed"),
        "{text}"
    );
    assert!(text.contains("<pre>@@ -1,2 +1,2 @@"), "{text}");
}

#[tokio::test]
async fn telegram_channel_names_stay_strings() {
    let server = accepting_server().await;
    send_once(&server, "tgram://123456:ABC-def/@my_channel")
        .await
        .expect("sent");
    assert_eq!(
        json_body(&only_request(&server).await)["chat_id"],
        "@my_channel"
    );
}

#[tokio::test]
async fn discord_pings_nobody() {
    let server = accepting_server().await;
    send_once(&server, "discord://123456/tok_en-1?username=Vigil%20Bot")
        .await
        .expect("sent");

    let request = only_request(&server).await;
    assert_eq!(request.url.path(), "/api/webhooks/123456/tok_en-1");
    assert_eq!(request.url.query(), Some("wait=true"));
    let body = json_body(&request);
    assert_eq!(body["allowed_mentions"], json!({ "parse": [] }));
    assert_eq!(body["username"], "Vigil Bot");
    assert!(
        body["content"]
            .as_str()
            .is_some_and(|c| c.starts_with("**Example product page** changed"))
    );
}

#[tokio::test]
async fn slack_posts_text_to_the_webhook_path() {
    let server = accepting_server().await;
    send_once(&server, "slack://T000/B000/XXXX")
        .await
        .expect("sent");

    let request = only_request(&server).await;
    assert_eq!(request.url.path(), "/services/T000/B000/XXXX");
    let text = json_body(&request)["text"]
        .as_str()
        .expect("text")
        .to_owned();
    assert!(text.starts_with("*Example product page* changed"), "{text}");
}

#[tokio::test]
async fn json_webhooks_get_the_whole_event() {
    let server = accepting_server().await;
    let url = format!("json://me:pw@127.0.0.1:{}/hooks/in?room=7", port(&server));
    send_once(&server, &url).await.expect("sent");

    let request = only_request(&server).await;
    assert_eq!(
        (request.url.path(), request.url.query()),
        ("/hooks/in", Some("room=7"))
    );
    assert_eq!(header(&request, "content-type"), "application/json");
    assert_eq!(header(&request, "authorization"), "Basic bWU6cHc=");
    let body = json_body(&request);
    assert_eq!(body["version"], 1);
    assert_eq!(body["watch"]["name"], "Example product page");
    assert_eq!(body["watch"]["url"], "https://example.com/products/widget");
    assert_eq!(body["checked_at"], "2026-01-01T00:00:00Z");
    assert_eq!(body["added"], json!(["Widget: $12"]));
    assert_eq!(body["removed"], json!(["Widget: $10"]));
    assert_eq!(body["triggered_text"], json!(["Widget: $12"]));
    assert_eq!(body["truncated"], false);
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("Widget: $12"))
    );
}

#[tokio::test]
async fn form_webhooks_send_urlencoded_fields() {
    let server = accepting_server().await;
    send_once(&server, &format!("form://127.0.0.1:{}/in", port(&server)))
        .await
        .expect("sent");

    let request = only_request(&server).await;
    assert_eq!(
        header(&request, "content-type"),
        "application/x-www-form-urlencoded"
    );
    let fields: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(&request.body)
            .into_owned()
            .collect();
    assert_eq!(fields["watch_name"], "Example product page");
    assert_eq!(fields["added"], "Widget: $12");
    assert_eq!(fields["truncated"], "false");
    assert!(fields["message"].contains("Widget: $10"));
}

#[tokio::test]
async fn gotify_sends_the_key_in_a_header() {
    let server = accepting_server().await;
    send_once(
        &server,
        &format!(
            "gotify://127.0.0.1:{}/push/Aabc123?priority=7",
            port(&server)
        ),
    )
    .await
    .expect("sent");

    let request = only_request(&server).await;
    assert_eq!(request.url.path(), "/push/message");
    assert_eq!(request.url.query(), None);
    assert_eq!(header(&request, "x-gotify-key"), "Aabc123");
    let body = json_body(&request);
    assert_eq!(body["title"], "Example product page changed");
    assert_eq!(body["priority"], 7);
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("Widget: $12"))
    );
}

#[tokio::test]
async fn long_webhook_line_lists_are_bounded() {
    use vigil_notify::{Event, prepare};
    let server = accepting_server().await;
    let channel = channel(&format!("json://127.0.0.1:{}/in", port(&server)));
    let mut event = Event::sample();
    event.added = (0..40_000)
        .map(|i| format!("{i}: {}", "x".repeat(40)))
        .collect();
    let prepared = prepare(channel.markup(), channel.limit(), &event, None);
    sender(&server)
        .send(&channel, &prepared, &event)
        .await
        .expect("sent");

    let body = json_body(&only_request(&server).await);
    let lines = body["added"].as_array().expect("added").len();
    assert!(lines > 1000 && lines < 40_000, "{lines}");
    assert_eq!(body["truncated"], true);
}
