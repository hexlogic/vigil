//! The notification targets page and its forms.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use vigil_store::{NewTarget, NotificationTarget};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::*;

const SECRET: &str = "very-secret-topic-name";

fn form(fields: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish()
}

fn htmx_post(path: &str) -> Result<Request<Body>, axum::http::Error> {
    Request::post(path)
        .header("hx-request", "true")
        .header("origin", "http://localhost")
        .body(Body::empty())
}

async fn add(
    app: &App,
    name: &str,
    url: &str,
) -> Result<NotificationTarget, Box<dyn std::error::Error>> {
    Ok(app
        .store
        .create_target(&NewTarget {
            name: name.into(),
            url: url.into(),
            default_template: None,
        })
        .await?)
}

async fn ntfy(status: u16) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(status))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn an_empty_list_invites_adding_a_target() -> TestResult {
    let app = app().await?;
    let body = app.get("/notifications").await?.into_body();
    assert!(body.contains("<h1>Notification targets</h1>"), "{body}");
    assert!(body.contains("No targets yet."), "{body}");
    assert!(body.contains("href=\"/notifications/new\""), "{body}");
    assert!(
        body.contains("aria-current=\"page\">Notifications<"),
        "{body}"
    );
    Ok(())
}

#[tokio::test]
async fn targets_are_listed_with_their_secrets_hidden() -> TestResult {
    let app = app().await?;
    let phone = add(&app, "phone", &format!("ntfys://ntfy.example.com/{SECRET}")).await?;
    add(&app, "plain", &format!("ntfy://ntfy.example.com/{SECRET}")).await?;
    add(&app, "broken", "nonsense://x").await?;
    let off = add(&app, "off", "jsons://hooks.example.com/path").await?;
    app.store.set_target_enabled(off.id, false).await?;
    let watch = app.add_watch("w", "https://w.example/").await?;
    app.store.link_target(watch.id, phone.id).await?;

    let body = app.get("/notifications").await?.into_body();
    assert!(!body.contains(SECRET), "no secret in the list: {body}");
    assert!(body.contains("ntfys://ntfy.example.com/***"), "{body}");
    assert!(body.contains("1 watch"), "{body}");
    assert!(
        body.contains("Sends a secret over plain HTTP"),
        "only for the plain one: {body}"
    );
    assert_eq!(body.matches("Sends a secret over plain HTTP").count(), 1);
    assert!(body.contains("cannot be read"), "{body}");
    assert!(body.contains(">Off<") && body.contains(">On<"), "{body}");
    assert!(
        body.contains(&format!("/notifications/{}/test", phone.id)),
        "{body}"
    );
    // Listed by name.
    let order: Vec<_> = ["broken", "off", "phone", "plain"]
        .iter()
        .map(|n| body.find(&format!(">{n}</span>")).unwrap_or(usize::MAX))
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    Ok(())
}

#[tokio::test]
async fn a_target_is_added_from_the_form() -> TestResult {
    let app = app().await?;
    let page = app.get("/notifications/new").await?;
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.body().contains("<h1>Add a notification target</h1>"));
    assert!(
        page.body().contains("name=\"url\" value=\"\""),
        "{}",
        page.body()
    );

    let body = form(&[
        ("name", "  phone  "),
        ("url", &format!("ntfys://ntfy.example.com/{SECRET}")),
        ("template", "{{ watch_name }} changed"),
    ]);
    let response = app.post_form("/notifications/new", &body).await?;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "{}",
        response.body()
    );
    assert_eq!(header_of(&response, "location"), "/notifications");

    let targets = app.store.list_targets().await?;
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].name, "phone");
    assert_eq!(
        targets[0].url,
        format!("ntfys://ntfy.example.com/{SECRET}"),
        "the address is kept as typed"
    );
    assert_eq!(
        targets[0].default_template.as_deref(),
        Some("{{ watch_name }} changed")
    );
    assert!(targets[0].enabled);
    Ok(())
}

#[tokio::test]
async fn a_target_with_problems_is_shown_again_without_its_address() -> TestResult {
    let app = app().await?;
    add(&app, "taken", "ntfy://h.example/t").await?;

    let bad_scheme = form(&[("name", "x"), ("url", &format!("telegraph://{SECRET}"))]);
    let response = app.post_form("/notifications/new", &bad_scheme).await?;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        response.body().contains("not supported yet") || response.body().contains("telegraph"),
        "{}",
        response.body()
    );
    assert!(
        !response.body().contains(SECRET),
        "the address is not echoed back: {}",
        response.body()
    );

    let cases = [
        (
            form(&[("name", ""), ("url", "ntfy://h.example/t")]),
            "Give the target a name.",
        ),
        (
            form(&[("name", "taken"), ("url", "ntfy://h.example/t2")]),
            "Another target already has this name.",
        ),
        (form(&[("name", "n"), ("url", "")]), "Enter the address."),
        (
            form(&[
                ("name", "n"),
                ("url", "ntfy://h.example/t"),
                ("template", "{{ nonsense }}"),
            ]),
            "template:",
        ),
        (
            form(&[("name", &"x".repeat(101)), ("url", "ntfy://h.example/t")]),
            "at most 100 characters",
        ),
    ];
    for (body, message) in cases {
        let response = app.post_form("/notifications/new", &body).await?;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{message}"
        );
        assert!(
            response.body().contains(message),
            "{message} in {}",
            response.body()
        );
    }
    assert_eq!(
        app.store.list_targets().await?.len(),
        1,
        "nothing was added"
    );
    Ok(())
}

#[tokio::test]
async fn editing_never_shows_the_address_and_an_empty_one_keeps_it() -> TestResult {
    let app = app().await?;
    let url = format!("ntfys://ntfy.example.com/{SECRET}");
    let target = add(&app, "phone", &url).await?;
    app.store
        .update_target(
            target.id,
            &NewTarget {
                name: "phone".into(),
                url: url.clone(),
                default_template: Some("tmpl".into()),
            },
        )
        .await?;

    let page = app
        .get(&format!("/notifications/{}/edit", target.id))
        .await?;
    assert_eq!(page.status(), StatusCode::OK);
    let body = page.body();
    assert!(!body.contains(SECRET), "{body}");
    assert!(
        body.contains("placeholder=\"ntfys://ntfy.example.com/***\""),
        "{body}"
    );
    assert!(body.contains("name=\"url\" value=\"\""), "{body}");
    assert!(
        body.contains("value=\"phone\"") && body.contains(">tmpl</textarea>"),
        "{body}"
    );

    // Rename only: the address stays.
    let response = app
        .post_form(
            &format!("/notifications/{}/edit", target.id),
            &form(&[("name", "mobile"), ("url", ""), ("template", "")]),
        )
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "{}",
        response.body()
    );
    let after = app.store.get_target(target.id).await?;
    assert_eq!(
        (
            after.name.as_str(),
            after.url.as_str(),
            after.default_template.as_deref()
        ),
        ("mobile", url.as_str(), None)
    );

    // A new address replaces it.
    app.post_form(
        &format!("/notifications/{}/edit", target.id),
        &form(&[("name", "mobile"), ("url", "ntfys://other.example/new")]),
    )
    .await?;
    assert_eq!(
        app.store.get_target(target.id).await?.url,
        "ntfys://other.example/new"
    );

    // A bad new address changes nothing and is not echoed.
    let bad = app
        .post_form(
            &format!("/notifications/{}/edit", target.id),
            &form(&[("name", "mobile"), ("url", &format!("nonsense://{SECRET}"))]),
        )
        .await?;
    assert_eq!(bad.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!bad.body().contains(SECRET));
    assert_eq!(
        app.store.get_target(target.id).await?.url,
        "ntfys://other.example/new"
    );

    // Taking another target's name is refused.
    add(&app, "taken", "ntfy://h.example/t").await?;
    let clash = app
        .post_form(
            &format!("/notifications/{}/edit", target.id),
            &form(&[("name", "taken")]),
        )
        .await?;
    assert_eq!(clash.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        clash
            .body()
            .contains("Another target already has this name.")
    );

    let missing = "00000000-0000-7000-8000-000000000000";
    assert_eq!(
        app.get(&format!("/notifications/{missing}/edit"))
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.post_form(
            &format!("/notifications/{missing}/edit"),
            &form(&[("name", "x")])
        )
        .await?
        .status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn the_test_button_sends_one_message_and_says_how_it_went() -> TestResult {
    let app = app().await?;
    let server = ntfy(200).await;
    let target = add(
        &app,
        "local",
        &format!("ntfy://127.0.0.1:{}/{SECRET}", server.address().port()),
    )
    .await?;

    let response = app
        .send(htmx_post(&format!("/notifications/{}/test", target.id))?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.body();
    assert!(body.starts_with("<tr id=\"target-"), "{body}");
    assert!(body.contains("The test message was sent."), "{body}");
    assert!(!body.contains(SECRET), "{body}");
    let requests = server.received_requests().await.unwrap_or_default();
    assert_eq!(requests.len(), 1);
    assert!(String::from_utf8_lossy(&requests[0].body).contains("Vigil test notification"));

    // It is in the delivery log, without a watch.
    let logged: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_log WHERE watch_id IS NULL AND outcome = 'sent'",
    )
    .fetch_one(app.store.pool())
    .await?;
    assert_eq!(logged, 1);
    Ok(())
}

#[tokio::test]
async fn a_failed_test_is_reported_after_one_attempt_without_the_address() -> TestResult {
    let app = app().await?;
    let server = ntfy(500).await;
    let target = add(
        &app,
        "local",
        &format!("ntfy://127.0.0.1:{}/{SECRET}", server.address().port()),
    )
    .await?;

    let body = app
        .send(htmx_post(&format!("/notifications/{}/test", target.id))?)
        .await?
        .into_body();
    assert!(body.contains("The test message was not sent:"), "{body}");
    assert!(body.contains("500"), "{body}");
    assert!(!body.contains(SECRET), "{body}");
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        1,
        "no retries behind a button"
    );
    Ok(())
}

#[tokio::test]
async fn a_target_can_be_turned_off_and_on() -> TestResult {
    let app = app().await?;
    let target = add(&app, "t", "ntfy://h.example/t").await?;
    let off = app
        .send(htmx_post(&format!("/notifications/{}/disable", target.id))?)
        .await?;
    assert!(
        off.body().contains(">Off<") && off.body().contains(">Turn on<"),
        "{}",
        off.body()
    );
    assert!(!app.store.get_target(target.id).await?.enabled);
    let on = app
        .send(htmx_post(&format!("/notifications/{}/enable", target.id))?)
        .await?;
    assert!(
        on.body().contains(">On<") && on.body().contains(">Turn off<"),
        "{}",
        on.body()
    );
    assert!(app.store.get_target(target.id).await?.enabled);

    let missing = "00000000-0000-7000-8000-000000000000";
    assert_eq!(
        app.send(htmx_post(&format!("/notifications/{missing}/disable"))?)
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn deleting_a_target_asks_first_and_leaves_the_watches_alone() -> TestResult {
    let app = app().await?;
    let target = add(&app, "doomed", &format!("ntfy://h.example/{SECRET}")).await?;
    let watch = app.add_watch("w", "https://w.example/").await?;
    app.store.link_target(watch.id, target.id).await?;

    let ask = app
        .get(&format!("/notifications/{}/delete", target.id))
        .await?;
    assert!(
        ask.body()
            .contains("Delete “doomed”? It stops notifying 1 watch."),
        "{}",
        ask.body()
    );
    assert!(!ask.body().contains(SECRET));
    assert!(app.store.get_target(target.id).await.is_ok());

    let done = app
        .send(htmx_post(&format!("/notifications/{}/delete", target.id))?)
        .await?;
    assert_eq!((done.status(), done.body().as_str()), (StatusCode::OK, ""));
    assert!(app.store.get_target(target.id).await.is_err());
    assert!(app.store.get_watch(watch.id).await.is_ok());
    assert!(
        app.store
            .targets_for_watch(watch.id, false)
            .await?
            .is_empty()
    );
    assert_eq!(
        app.get(&format!("/notifications/{}/delete", target.id))
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn everything_here_needs_a_session_and_a_request_from_this_site() -> TestResult {
    let app = app().await?;
    let server = ntfy(200).await;
    let target = add(
        &app,
        "local",
        &format!("ntfy://127.0.0.1:{}/t", server.address().port()),
    )
    .await?;
    let guest = app.signed_out();

    for path in [
        "/notifications".to_owned(),
        "/notifications/new".to_owned(),
        format!("/notifications/{}/edit", target.id),
        format!("/notifications/{}/row", target.id),
    ] {
        assert_eq!(
            guest.get(&path).await?.status(),
            StatusCode::SEE_OTHER,
            "{path}"
        );
    }
    for action in ["test", "enable", "disable", "delete"] {
        let path = format!("/notifications/{}/{action}", target.id);
        assert_eq!(
            guest.send(htmx_post(&path)?).await?.status(),
            StatusCode::UNAUTHORIZED,
            "{action}"
        );
        assert_eq!(
            app.post_form_from(&path, "", "https://evil.example")
                .await?
                .status(),
            StatusCode::FORBIDDEN,
            "{action}"
        );
    }
    let create = form(&[("name", "evil"), ("url", "ntfy://evil.example/t")]);
    assert_eq!(
        app.post_form_from("/notifications/new", &create, "https://evil.example")
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(app.store.list_targets().await?.len(), 1);
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "no test message was sent"
    );
    Ok(())
}

#[tokio::test]
async fn the_page_speaks_ukrainian_on_request() -> TestResult {
    let app = app().await?;
    add(&app, "t", "ntfy://h.example/t").await?;
    let body = app.get_in("/notifications", "uk").await?.into_body();
    assert!(
        body.contains("Цілі сповіщень") && body.contains("Надіслати пробне"),
        "{body}"
    );
    Ok(())
}

#[tokio::test]
async fn the_watch_form_offers_the_targets_by_name_and_kind_only() -> TestResult {
    let app = app().await?;
    add(&app, "phone", &format!("ntfys://ntfy.example.com/{SECRET}")).await?;
    let body = app.get("/watch/new").await?.into_body();
    assert!(body.contains("phone"), "{body}");
    assert!(body.contains("ntfy"), "{body}");
    assert!(!body.contains(SECRET), "{body}");
    Ok(())
}
