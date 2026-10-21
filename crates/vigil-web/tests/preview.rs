//! The live preview beside the watch form.

mod common;

use axum::http::StatusCode;
use vigil_store::{CheckSuccess, Content};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::*;

fn form(fields: &[(&str, &str)]) -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let draft = format!("preview-test-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    if !fields.iter().any(|(name, _)| *name == "draft") {
        serializer.append_pair("draft", &draft);
    }
    serializer.extend_pairs(fields).finish()
}

async fn page(body: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/html; charset=utf-8"))
        .mount(&server)
        .await;
    server
}

async fn requests(server: &MockServer) -> usize {
    server.received_requests().await.unwrap_or_default().len()
}

async fn preview(
    app: &App,
    query: &str,
    fields: &[(&str, &str)],
) -> Result<String, Box<dyn std::error::Error>> {
    let response = app
        .post_form(&format!("/watch/preview{query}"), &form(fields))
        .await?;
    assert_eq!(response.status(), StatusCode::OK, "{}", response.body());
    Ok(response.into_body())
}

#[tokio::test]
async fn without_an_address_there_is_nothing_to_preview() -> TestResult {
    let app = app().await?;
    let body = preview(&app, "", &[("url", "  "), ("processor", "text")]).await?;
    assert!(
        body.contains("Enter an address to see a preview."),
        "{body}"
    );
    assert!(!body.contains("<html"), "a fragment");
    Ok(())
}

#[tokio::test]
async fn the_preview_shows_what_the_filters_keep() -> TestResult {
    let app = app().await?;
    let server =
        page("<h1>Shop</h1><p class=\"price\">$10</p><p class=\"stock\">in stock</p>").await;
    let body = preview(
        &app,
        "",
        &[
            ("url", &server.uri()),
            ("processor", "text"),
            ("step-0-type", "css"),
            ("step-0-selector", ".price"),
            ("step-0-mode", "include"),
        ],
    )
    .await?;
    assert!(
        body.contains("HTTP 200 · text/html; charset=utf-8"),
        "{body}"
    );
    assert!(body.contains("<pre class=\"output\">$10\n</pre>"), "{body}");
    assert!(
        !body.contains("in stock") && !body.contains("Shop"),
        "{body}"
    );
    assert!(body.contains("1 line"), "{body}");
    assert!(body.contains("Fetched in"), "{body}");
    assert!(
        body.contains("A new watch stores this as its first version."),
        "{body}"
    );
    // Nothing was kept.
    assert!(app.store.list_watches().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn the_page_is_fetched_once_while_the_form_is_edited() -> TestResult {
    let app = app().await?;
    let server = page("<p class=\"a\">alpha</p><p class=\"b\">beta</p>").await;
    let url = server.uri();
    let fields = |selector: &'static str| -> Vec<(&'static str, String)> {
        vec![
            ("draft", "one-form".into()),
            ("url", url.clone()),
            ("processor", "text".into()),
            ("step-0-type", "css".into()),
            ("step-0-selector", selector.into()),
        ]
    };
    let send = |query: &'static str, selector: &'static str| {
        let app = app.clone();
        let owned = fields(selector);
        async move {
            let pairs: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
            preview(&app, query, &pairs).await
        }
    };

    let first = send("", ".a").await?;
    assert!(first.contains("alpha\n</pre>"), "{first}");
    assert!(first.contains("Fetched in"));

    let second = send("", ".b").await?;
    assert!(
        second.contains("beta\n</pre>"),
        "the new filter applies: {second}"
    );
    assert!(second.contains("Using the page fetched"), "{second}");
    assert_eq!(requests(&server).await, 1, "the page was reused");

    // Asking again fetches again.
    let again = send("?refetch=1", ".b").await?;
    assert!(again.contains("Fetched in"), "{again}");
    assert_eq!(requests(&server).await, 2);

    // Another request (here, another header) is another page, and so is another form.
    let mut other = fields(".b");
    other.push(("headers", "X-Test: 1".into()));
    let pairs: Vec<(&str, &str)> = other.iter().map(|(k, v)| (*k, v.as_str())).collect();
    preview(&app, "", &pairs).await?;
    assert_eq!(requests(&server).await, 3);
    let mut elsewhere = fields(".b");
    elsewhere.retain(|(name, _)| *name != "draft");
    elsewhere.push(("draft", "another-form".into()));
    let pairs: Vec<(&str, &str)> = elsewhere.iter().map(|(k, v)| (*k, v.as_str())).collect();
    preview(&app, "", &pairs).await?;
    assert_eq!(requests(&server).await, 4);
    Ok(())
}

#[tokio::test]
async fn problems_with_the_settings_are_listed_and_the_site_is_left_alone() -> TestResult {
    let app = app().await?;
    let server = page("<p>x</p>").await;
    let body = preview(
        &app,
        "",
        &[
            ("url", &server.uri()),
            ("processor", "text"),
            ("step-0-type", "trim"),
            ("step-1-type", "css"),
            ("step-1-selector", "div[["),
            ("trigger_text", "/(/"),
            ("headers", "no colon"),
        ],
    )
    .await?;
    assert!(body.contains("Fix this to see a preview:"), "{body}");
    assert!(
        body.contains("Filter 2:") && body.contains("step 2 (css)"),
        "{body}"
    );
    assert!(body.contains("conditions: trigger_text[0]"), "{body}");
    assert!(body.contains("Line 1 is not a header"), "{body}");
    assert_eq!(
        requests(&server).await,
        0,
        "no request for settings that cannot work"
    );

    let bad_url = preview(
        &app,
        "",
        &[("url", "ftp://example.com/"), ("processor", "text")],
    )
    .await?;
    assert!(bad_url.contains("Fix this to see a preview:"), "{bad_url}");
    Ok(())
}

#[tokio::test]
async fn problems_with_things_the_preview_does_not_use_do_not_stop_it() -> TestResult {
    let app = app().await?;
    let server = page("<p>fine</p>").await;
    let body = preview(
        &app,
        "",
        &[
            ("url", &server.uri()),
            ("processor", "text"),
            ("every", "whenever"),
            ("retention", "0"),
            ("template", "{{ nonsense }}"),
            ("tag", "not-a-tag"),
        ],
    )
    .await?;
    assert!(body.contains("fine\n</pre>"), "{body}");
    assert!(!body.contains("Fix this"), "{body}");
    Ok(())
}

#[tokio::test]
async fn a_page_that_cannot_be_fetched_is_reported_with_a_way_to_try_again() -> TestResult {
    let app = app().await?;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let body = preview(&app, "", &[("url", &server.uri()), ("processor", "text")]).await?;
    assert!(body.contains("The page could not be fetched:"), "{body}");
    assert!(body.contains("500"), "{body}");
    assert!(body.contains("Fetch again"), "{body}");
    Ok(())
}

#[tokio::test]
async fn a_filter_that_matches_nothing_fails_the_preview_as_it_would_fail_a_check() -> TestResult {
    let app = app().await?;
    let server = page("<p>x</p>").await;
    let body = preview(
        &app,
        "",
        &[
            ("url", &server.uri()),
            ("processor", "text"),
            ("step-0-type", "css"),
            ("step-0-selector", ".missing"),
            ("step-0-required", "on"),
        ],
    )
    .await?;
    assert!(
        body.contains("The page was fetched, but processing it failed:"),
        "{body}"
    );
    assert!(body.contains("step 1 (css): matched nothing"), "{body}");
    Ok(())
}

#[tokio::test]
async fn the_page_text_is_escaped() -> TestResult {
    let app = app().await?;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw("<<script>alert(1)</script>>", "text/plain"),
        )
        .mount(&server)
        .await;
    let body = preview(&app, "", &[("url", &server.uri()), ("processor", "text")]).await?;
    assert!(!body.contains("<script>"), "{body}");
    assert!(body.contains("&#60;"), "{body}");
    Ok(())
}

#[tokio::test]
async fn a_long_result_is_cut_and_says_so() -> TestResult {
    let app = app().await?;
    let server = MockServer::start().await;
    let long = "word ".repeat(10_000);
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(long, "text/plain"))
        .mount(&server)
        .await;
    let body = preview(&app, "", &[("url", &server.uri()), ("processor", "text")]).await?;
    assert!(body.contains("Showing the first 20000 of"), "{body}");
    assert!(body.len() < 60_000, "{}", body.len());
    Ok(())
}

async fn stored(
    app: &App,
    name: &str,
    url: &str,
    text: &str,
) -> Result<vigil_core::Watch, Box<dyn std::error::Error>> {
    let watch = app.add_watch(name, url).await?;
    app.store
        .record_success(
            watch.id,
            &CheckSuccess {
                at: vigil_store::now_ms(),
                duration_ms: 1,
                http_status: Some(200),
                content_type: None,
                etag: None,
                last_modified: None,
                content: Some(Content {
                    text: text.into(),
                    raw_size: 1,
                }),
                settings_version: None,
            },
        )
        .await?;
    Ok(watch)
}

#[tokio::test]
async fn editing_a_watch_compares_the_preview_with_its_last_stored_version() -> TestResult {
    let app = app().await?;
    let server = page("<p>Price: $12</p>").await;
    let watch = stored(&app, "w", &server.uri(), "Price: $10\n").await?;
    let id = watch.id.to_string();
    let before = app.store.get_watch(watch.id).await?;

    let body = preview(
        &app,
        "",
        &[
            ("url", &server.uri()),
            ("processor", "text"),
            ("watch", &id),
            ("notify_added", "on"),
            ("notify_removed", "on"),
            ("notify_changed", "on"),
        ],
    )
    .await?;
    assert!(
        body.contains("Compared with the last stored version"),
        "{body}"
    );
    assert!(
        body.contains("<mark>$10</mark>") && body.contains("<mark>$12</mark>"),
        "{body}"
    );
    assert!(
        body.contains("class=\"dl del\"") && body.contains("class=\"dl add\""),
        "{body}"
    );
    assert!(body.contains("Would notify."), "{body}");

    // A condition that holds the change back.
    let blocked = preview(
        &app,
        "",
        &[
            ("url", &server.uri()),
            ("processor", "text"),
            ("watch", &id),
            ("trigger_text", "in stock"),
            ("notify_added", "on"),
        ],
    )
    .await?;
    assert!(blocked.contains("Would not notify."), "{blocked}");

    // The same text is no change.
    let same = stored(&app, "same", &server.uri(), "Price: $12\n").await?;
    let nothing = preview(
        &app,
        "",
        &[
            ("url", &server.uri()),
            ("processor", "text"),
            ("watch", &same.id.to_string()),
        ],
    )
    .await?;
    assert!(nothing.contains("No difference."), "{nothing}");
    assert!(!nothing.contains("class=\"diff\""), "{nothing}");

    // A watch without a stored version has nothing to compare with.
    let fresh = app.add_watch("fresh", &server.uri()).await?;
    let first = preview(
        &app,
        "",
        &[
            ("url", &server.uri()),
            ("processor", "text"),
            ("watch", &fresh.id.to_string()),
        ],
    )
    .await?;
    assert!(first.contains("has no stored version yet"), "{first}");

    // Previewing stored nothing.
    assert_eq!(app.store.list_snapshots(watch.id, 10).await?.len(), 1);
    assert_eq!(app.store.get_watch(watch.id).await?, before);
    Ok(())
}

#[tokio::test]
async fn the_form_page_carries_the_preview_and_the_filter_buttons_wake_it() -> TestResult {
    let app = app().await?;
    let watch = app.add_watch("w", "https://w.example/").await?;
    for path in ["/watch/new".to_owned(), format!("/watch/{}/edit", watch.id)] {
        let body = app.get(&path).await?.into_body();
        assert!(
            body.contains("hx-post=\"/watch/preview\""),
            "{path}: {body}"
        );
        assert!(body.contains("hx-include=\"#watch-form\""), "{path}");
        assert!(body.contains("hx-sync=\"this:replace\""), "{path}");
        assert!(
            body.contains("input from:#watch-form delay:700ms"),
            "{path}"
        );
        assert!(body.contains("stepschanged from:body"), "{path}");
        assert!(
            !body.contains(" style="),
            "{path}: the page policy forbids style attributes"
        );
    }
    let edit = app
        .get(&format!("/watch/{}/edit", watch.id))
        .await?
        .into_body();
    assert!(
        edit.contains(&format!("name=\"watch\" value=\"{}\"", watch.id)),
        "{edit}"
    );
    assert!(
        app.get("/watch/new")
            .await?
            .body()
            .contains("name=\"watch\" value=\"\"")
    );

    let steps = app
        .post_form("/watch/steps?op=add", &form(&[("new_step_type", "trim")]))
        .await?;
    assert_eq!(header_of(&steps, "hx-trigger"), "stepschanged");
    Ok(())
}

#[tokio::test]
async fn the_preview_needs_a_session_and_a_request_from_this_site() -> TestResult {
    let app = app().await?;
    let server = page("<p>x</p>").await;
    let body = form(&[("url", &server.uri()), ("processor", "text")]);

    let guest = app.signed_out().post_form("/watch/preview", &body).await?;
    assert_eq!(guest.status(), StatusCode::SEE_OTHER);
    let cross = app
        .post_form_from("/watch/preview", &body, "https://evil.example")
        .await?;
    assert_eq!(cross.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        requests(&server).await,
        0,
        "nothing was fetched on their behalf"
    );
    Ok(())
}

#[tokio::test]
async fn the_preview_speaks_ukrainian_on_request() -> TestResult {
    let app = app().await?;
    let body = app
        .send(
            axum::http::Request::post("/watch/preview")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("origin", "http://localhost")
                .header("accept-language", "uk")
                .body(axum::body::Body::from(form(&[
                    ("url", ""),
                    ("processor", "text"),
                ])))?,
        )
        .await?
        .into_body();
    assert!(
        body.contains("Вкажіть адресу, щоб побачити перегляд."),
        "{body}"
    );
    Ok(())
}
