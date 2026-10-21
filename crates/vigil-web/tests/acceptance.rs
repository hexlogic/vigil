//! Phase 4's acceptance test: a person adds a watch with a CSS filter and a live preview,
//! is notified of a change, and follows the link in the notification to its difference.
//!
//! Everything goes through the web UI's own pages and forms, as a browser would use them.

mod common;

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::*;
use vigil_web::{Auth, WebConfig};

fn form(fields: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish()
}

fn shop(price: &str, ad: &str) -> String {
    format!("<h1>Shop</h1><p class=\"price\">{price}</p><p class=\"ad\">{ad}</p>")
}

async fn serve(server: &MockServer, body: &str) {
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(body.to_owned(), "text/html; charset=utf-8"),
        )
        .mount(server)
        .await;
}

async fn wait_for<F, Fut>(what: &str, mut done: F) -> TestResult
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done().await {
        if Instant::now() > deadline {
            return Err(format!("timed out waiting for {what}").into());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Ok(())
}

#[tokio::test]
async fn add_a_filtered_watch_get_notified_and_read_the_diff() -> TestResult {
    // A server that knows its address, so that notifications can link back to it.
    let app = anonymous_with(WebConfig::new(Auth::Password, Some("http://localhost"))).await?;

    // Sign in at the login page.
    let login = app.get("/login").await?;
    assert!(login.body().contains("Sign in"));
    let cookie = app.sign_in().await?;
    let app = App {
        cookie: Some(cookie),
        ..app
    };
    assert_eq!(app.get("/").await?.status(), StatusCode::OK);

    // A notification target, added from its form, and tested from its page.
    let ntfy = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&ntfy)
        .await;
    let ntfy_url = format!("ntfy://127.0.0.1:{}/vigil-e2e", ntfy.address().port());
    let added = app
        .post_form(
            "/notifications/new",
            &form(&[("name", "phone"), ("url", &ntfy_url)]),
        )
        .await?;
    assert_eq!(added.status(), StatusCode::SEE_OTHER, "{}", added.body());
    let target = app.store.list_targets().await?.remove(0);
    let tested = app
        .send(
            Request::post(format!("/notifications/{}/test", target.id))
                .header("hx-request", "true")
                .header("origin", "http://localhost")
                .body(Body::empty())?,
        )
        .await?;
    assert!(
        tested.body().contains("The test message was sent."),
        "{}",
        tested.body()
    );
    assert_eq!(ntfy.received_requests().await.unwrap_or_default().len(), 1);

    // The page to watch: a price, and an ad that changes all the time.
    let site = MockServer::start().await;
    serve(&site, &shop("$10", "buy now")).await;
    let address = site.uri();

    // The add page, with its preview of the filter as it is typed.
    let page = app.get("/watch/new").await?;
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.body().contains("hx-post=\"/watch/preview\""));
    let fields: Vec<(&str, &str)> = vec![
        ("draft", "acceptance"),
        ("name", "Widget price"),
        ("url", &address),
        ("every", "1h"),
        ("processor", "text"),
        ("step-0-type", "css"),
        ("step-0-selector", ".price"),
        ("step-0-mode", "include"),
        ("notify_added", "on"),
        ("notify_removed", "on"),
        ("notify_changed", "on"),
    ];
    let preview = app.post_form("/watch/preview", &form(&fields)).await?;
    assert_eq!(preview.status(), StatusCode::OK);
    assert!(
        preview.body().contains("<pre class=\"output\">$10\n</pre>"),
        "{}",
        preview.body()
    );
    assert!(
        !preview.body().contains("buy now"),
        "the filter keeps only the price"
    );

    // Save it, with the target.
    let mut save = fields.clone();
    let target_id = target.id.to_string();
    save.push(("target", &target_id));
    let saved = app.post_form("/watch/new", &form(&save)).await?;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER, "{}", saved.body());
    let watch = app.store.list_watches().await?.remove(0);
    assert_eq!(watch.name, "Widget price");

    // Its first check stores the baseline without notifying.
    wait_for("the first check", || async {
        !app.store
            .list_snapshots(watch.id, 1)
            .await
            .unwrap_or_default()
            .is_empty()
    })
    .await?;
    let list = app.get("/").await?.into_body();
    assert!(
        list.contains("Widget price") && list.contains("badge-ok"),
        "{list}"
    );

    // The price changes, and a person presses "Check now".
    serve(&site, &shop("$12", "buy now")).await;
    let checked = app
        .send(
            Request::post(format!("/watch/{}/check", watch.id))
                .header("hx-request", "true")
                .header("origin", "http://localhost")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(checked.status(), StatusCode::OK);
    assert!(checked.body().contains("badge-ok"), "{}", checked.body());

    // The notification arrives, with a link to the difference.
    app.notifier.close_and_wait().await;
    let requests = ntfy.received_requests().await.unwrap_or_default();
    assert_eq!(requests.len(), 2, "the test message and the change");
    let message: Value = serde_json::from_slice(&requests[1].body)?;
    assert_eq!(message["title"], "Widget price changed");
    assert!(
        message["message"]
            .as_str()
            .ok_or("no message")?
            .contains("$12"),
        "{message}"
    );
    let link = message["click"]
        .as_str()
        .ok_or("the notification has no link")?;
    let path = link
        .strip_prefix("http://localhost")
        .ok_or_else(|| format!("unexpected link {link}"))?;
    assert!(
        path.starts_with(&format!("/watch/{}/diff?from=", watch.id)),
        "{path}"
    );

    // The link opens the difference, in the browser that is signed in...
    let diff = app.get(path).await?;
    assert_eq!(diff.status(), StatusCode::OK);
    assert!(
        diff.body().contains("Changes to Widget price"),
        "{}",
        diff.body()
    );
    let body = diff.body();
    let (removed, added) = (
        body.find("class=\"dl del\"").ok_or("no removed line")?,
        body.find("class=\"dl add\"").ok_or("no added line")?,
    );
    assert!(
        body[removed..added].contains("$10") && body[added..].contains("$12"),
        "{body}"
    );
    // ...and sends one who is not to the login, and back afterwards.
    let guest = app.signed_out().get(path).await?;
    assert_eq!(guest.status(), StatusCode::SEE_OTHER);
    let login_page = header_of(&guest, "location").to_owned();
    assert!(login_page.starts_with("/login?next="), "{login_page}");
    let back = app
        .signed_out()
        .post_form(
            "/login",
            &format!(
                "password=correct+horse+battery&next={}",
                url::form_urlencoded::byte_serialize(path.as_bytes()).collect::<String>()
            ),
        )
        .await?;
    assert_eq!(header_of(&back, "location"), path);

    // A change the filter hides does not notify, and is no change at all.
    serve(&site, &shop("$12", "half price today")).await;
    let again = app
        .send(
            Request::post(format!("/watch/{}/check", watch.id))
                .header("hx-request", "true")
                .header("origin", "http://localhost")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(again.status(), StatusCode::OK);
    app.notifier.close_and_wait().await;
    assert_eq!(
        ntfy.received_requests().await.unwrap_or_default().len(),
        2,
        "no new notification"
    );
    assert_eq!(
        app.store.list_snapshots(watch.id, 10).await?.len(),
        2,
        "no new version"
    );

    // The history shows the two versions.
    let history = app
        .get(&format!("/watch/{}/history", watch.id))
        .await?
        .into_body();
    assert!(history.contains("2 versions kept"), "{history}");
    Ok(())
}
