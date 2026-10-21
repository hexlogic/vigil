//! Adding a watch and changing one from the form.

mod common;

use std::time::{Duration, Instant};

use axum::http::StatusCode;
use vigil_core::{Processor, Schedule, Scope, SelectMode, Step};
use vigil_store::{NewTag, NewTarget};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::*;

/// A form body from fields. Each form gets a draft id of its own unless it brings one: the
/// processing slot of a form is shared by the whole process, and tests run side by side.
fn form(fields: &[(&str, &str)]) -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let draft = format!("test-{}", NEXT.fetch_add(1, Ordering::Relaxed));
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

async fn wait_for<F, Fut>(what: &str, mut done: F) -> TestResult
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done().await {
        if Instant::now() > deadline {
            return Err(format!("timed out waiting for {what}").into());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Ok(())
}

#[tokio::test]
async fn the_empty_form_has_sensible_defaults() -> TestResult {
    let app = app().await?;
    let response = app.get("/watch/new").await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.body();
    assert!(body.contains("<h1>Add a watch</h1>"), "{body}");
    assert!(body.contains("action=\"/watch/new\""), "{body}");
    assert!(body.contains("name=\"every\" value=\"1h\""), "{body}");
    assert!(body.contains("<option value=\"text\" selected>"), "{body}");
    for flag in ["notify_added", "notify_removed", "notify_changed"] {
        assert!(body.contains(&format!("name=\"{flag}\" checked")), "{flag}");
    }
    assert!(body.contains("name=\"retention\" value=\"100\""), "{body}");
    assert!(
        body.contains("No filters: all of the text is compared."),
        "{body}"
    );
    assert!(body.contains("name=\"draft\" value=\""), "{body}");
    assert!(
        !body.contains(" style="),
        "the page policy forbids style attributes"
    );
    // The page also serves the language of the request.
    assert!(
        app.get_in("/watch/new", "uk")
            .await?
            .body()
            .contains("Нове відстеження")
    );
    Ok(())
}

#[tokio::test]
async fn a_valid_form_adds_a_watch_and_starts_its_first_check() -> TestResult {
    let app = app().await?;
    let server = page("<h1>Shop</h1><p class=\"price\">$10</p>").await;
    let tag = app
        .store
        .create_tag(&NewTag {
            name: "shops".into(),
            color: "blue".into(),
            default_interval_secs: None,
        })
        .await?;
    let target = app
        .store
        .create_target(&NewTarget {
            name: "phone".into(),
            url: "ntfy://ntfy.example/t".into(),
            default_template: None,
        })
        .await?;
    let url = server.uri();

    let body = form(&[
        ("draft", "abc"),
        ("name", "  Widget  "),
        ("url", &url),
        ("every", "15m"),
        ("processor", "text"),
        ("step-0-type", "css"),
        ("step-0-selector", ".price"),
        ("step-0-mode", "include"),
        ("step-0-required", "on"),
        ("step-1-type", "trim"),
        ("trigger_text", "in stock\n/sale/i\n"),
        ("block_text", "ad"),
        ("notify_added", "on"),
        ("notify_changed", "on"),
        ("timeout", "20"),
        ("user_agent", "my-agent"),
        ("headers", "Accept-Language: de\nX-Test: 1"),
        ("jitter", "30"),
        ("retention", "7"),
        ("tag", &tag.id.to_string()),
        ("target", &target.id.to_string()),
        ("template", "{{ watch_name }} changed"),
    ]);
    let response = app.post_form("/watch/new", &body).await?;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "{}",
        response.body()
    );
    assert_eq!(header_of(&response, "location"), "/");

    let watches = app.store.list_watches().await?;
    assert_eq!(watches.len(), 1);
    let watch = &watches[0];
    assert_eq!(watch.name, "Widget");
    assert_eq!(watch.url, url);
    assert_eq!(watch.schedule, Schedule::Interval(900));
    assert_eq!(watch.processor, Processor::Text);
    assert_eq!(
        watch.pipeline,
        [
            Step::Css {
                selector: ".price".into(),
                mode: SelectMode::Include,
                required: true
            },
            Step::Trim {}
        ]
    );
    assert_eq!(watch.conditions.trigger_text, ["in stock", "/sale/i"]);
    assert_eq!(watch.conditions.block_text, ["ad"]);
    assert!(
        watch.conditions.notify_on.added
            && !watch.conditions.notify_on.removed
            && watch.conditions.notify_on.changed
    );
    assert_eq!(watch.fetch.timeout_secs, Some(20));
    assert_eq!(watch.fetch.user_agent.as_deref(), Some("my-agent"));
    assert_eq!(
        watch
            .fetch
            .headers
            .get("Accept-Language")
            .map(String::as_str),
        Some("de")
    );
    assert_eq!(watch.fetch.headers.len(), 2);
    assert_eq!(watch.jitter_secs, 30);
    assert_eq!(watch.snapshot_retention, 7);
    assert_eq!(
        watch.notification_template.as_deref(),
        Some("{{ watch_name }} changed")
    );
    assert_eq!(app.store.tags_for_watch(watch.id).await?.len(), 1);
    assert_eq!(
        app.store.targets_for_watch(watch.id, false).await?[0].id,
        target.id
    );

    // Nobody waited for it, but the first check ran, with the filter applied.
    let id = watch.id;
    wait_for("the first check", || async {
        !app.store
            .list_snapshots(id, 1)
            .await
            .unwrap_or_default()
            .is_empty()
    })
    .await?;
    let snapshot = app.store.list_snapshots(id, 1).await?;
    assert_eq!(app.store.snapshot_content(snapshot[0].id).await?, "$10\n");
    Ok(())
}

#[tokio::test]
async fn an_empty_name_uses_the_address_and_an_empty_interval_the_tag_default() -> TestResult {
    let app = app().await?;
    let slow = app
        .store
        .create_tag(&NewTag {
            name: "slow".into(),
            color: "gray".into(),
            default_interval_secs: Some(7200),
        })
        .await?;
    let quick = app
        .store
        .create_tag(&NewTag {
            name: "quick".into(),
            color: "red".into(),
            default_interval_secs: Some(600),
        })
        .await?;
    let body = form(&[
        ("url", "https://example.com/a"),
        ("every", ""),
        ("processor", "text"),
        ("notify_added", "on"),
        ("tag", &slow.id.to_string()),
        ("tag", &quick.id.to_string()),
    ]);
    let response = app.post_form("/watch/new", &body).await?;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "{}",
        response.body()
    );
    let watch = &app.store.list_watches().await?[0];
    assert_eq!(watch.name, "https://example.com/a");
    assert_eq!(
        watch.schedule,
        Schedule::Interval(600),
        "the shortest of the tags' defaults"
    );

    let plain = form(&[
        ("url", "https://example.com/b"),
        ("processor", "text"),
        ("notify_added", "on"),
    ]);
    app.post_form("/watch/new", &plain).await?;
    let watches = app.store.list_watches().await?;
    let second = watches
        .iter()
        .find(|w| w.url.ends_with("/b"))
        .ok_or("not added")?;
    assert_eq!(second.schedule, Schedule::Interval(3600));
    Ok(())
}

#[tokio::test]
async fn a_form_with_problems_is_shown_again_with_what_was_typed() -> TestResult {
    let app = app().await?;
    let body = form(&[
        ("url", ""),
        ("name", "\"quoted\" <b>"),
        ("every", "soon"),
        ("processor", "json"),
        ("step-0-type", "css"),
        ("step-0-selector", ""),
        ("headers", "Accept-Language: de\nno colon here"),
        ("timeout", "forever"),
        ("retention", "0"),
        ("jitter", "x"),
        ("tag", "not-an-id"),
    ]);
    let response = app.post_form("/watch/new", &body).await?;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = response.body();
    for message in [
        "Enter the address to watch.",
        "Not an interval.",
        "This filter needs a value.",
        "Line 2 is not a header written as Name: value.",
        "Enter a number of seconds from 1 to 3600",
        "Enter a number from 1 up.",
        "Enter a whole number, or leave it empty.",
        "That choice no longer exists.",
        "Choose at least one kind of change",
    ] {
        assert!(html.contains(message), "{message} in {html}");
    }
    // What was typed is still there, escaped.
    assert!(
        html.contains("value=\"&#34;quoted&#34; &#60;b&#62;\""),
        "{html}"
    );
    assert!(html.contains("<option value=\"json\" selected>"), "{html}");
    assert!(
        html.contains("name=\"step-0-type\" value=\"css\""),
        "{html}"
    );
    assert!(html.contains("aria-invalid=\"true\""));
    assert!(
        app.store.list_watches().await?.is_empty(),
        "nothing was created"
    );
    Ok(())
}

#[tokio::test]
async fn an_address_that_is_not_http_is_refused() -> TestResult {
    let app = app().await?;
    for bad in [
        "ftp://example.com/",
        "file:///etc/passwd",
        "example.com",
        "javascript:alert(1)",
    ] {
        let body = form(&[("url", bad), ("processor", "text"), ("notify_added", "on")]);
        let response = app.post_form("/watch/new", &body).await?;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
    assert!(app.store.list_watches().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_bad_selector_is_reported_on_the_filter_it_belongs_to() -> TestResult {
    let app = app().await?;
    let body = form(&[
        ("url", "https://example.com/"),
        ("processor", "text"),
        ("notify_added", "on"),
        ("step-0-type", "trim"),
        ("step-1-type", "css"),
        ("step-1-selector", "div[["),
    ]);
    let response = app.post_form("/watch/new", &body).await?;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = response.body();
    // The message is the library's, in English, and sits in the second filter.
    let second = html.find("2. ").ok_or("no second filter")?;
    let message = html
        .find("step 2 (css)")
        .ok_or_else(|| format!("no step message in {html}"))?;
    assert!(message > second, "{html}");
    assert!(html.contains("step-error"), "{html}");
    Ok(())
}

#[tokio::test]
async fn filters_in_the_wrong_order_are_refused_with_a_reason() -> TestResult {
    let app = app().await?;
    let body = form(&[
        ("url", "https://example.com/"),
        ("processor", "text"),
        ("notify_added", "on"),
        ("step-0-type", "trim"),
        ("step-1-type", "css"),
        ("step-1-selector", "p"),
    ]);
    let response = app.post_form("/watch/new", &body).await?;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        response.body().contains("step 2 (css)"),
        "{}",
        response.body()
    );

    let wrong_processor = form(&[
        ("url", "https://example.com/"),
        ("processor", "json"),
        ("notify_added", "on"),
        ("step-0-type", "css"),
        ("step-0-selector", "p"),
    ]);
    let response = app.post_form("/watch/new", &wrong_processor).await?;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        response.body().contains("step 1 (css)"),
        "{}",
        response.body()
    );
    Ok(())
}

#[tokio::test]
async fn a_bad_condition_or_template_is_reported() -> TestResult {
    let app = app().await?;
    let body = form(&[
        ("url", "https://example.com/"),
        ("processor", "text"),
        ("notify_added", "on"),
        ("trigger_text", "/(/"),
        ("template", "{{ nonsense_variable }}"),
    ]);
    let response = app.post_form("/watch/new", &body).await?;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = response.body();
    assert!(html.contains("conditions: trigger_text[0]"), "{html}");
    assert!(html.contains("template:"), "{html}");
    Ok(())
}

#[tokio::test]
async fn the_filter_buttons_change_the_list_from_the_form_as_it_is() -> TestResult {
    let app = app().await?;
    let current = [
        ("step-0-type", "css"),
        ("step-0-selector", ".a"),
        ("step-1-type", "trim"),
        ("step-2-type", "sort"),
        ("new_step_type", "jq"),
    ];
    let post = |query: &str| {
        let app = app.clone();
        let query = query.to_owned();
        let body = form(&current);
        async move { app.post_form(&format!("/watch/steps?{query}"), &body).await }
    };

    let added = post("op=add").await?;
    assert_eq!(added.status(), StatusCode::OK);
    let html = added.body();
    assert!(!html.contains("<html"), "a fragment");
    assert!(html.contains("name=\"step-3-type\" value=\"jq\""), "{html}");
    assert!(
        html.contains("value=\".a\""),
        "what was typed stays: {html}"
    );

    let removed = post("op=remove&i=1").await?;
    assert!(
        !removed.body().contains("value=\"trim\""),
        "{}",
        removed.body()
    );
    assert!(
        removed
            .body()
            .contains("name=\"step-1-type\" value=\"sort\""),
        "renumbered: {}",
        removed.body()
    );

    let moved = post("op=up&i=2").await?;
    assert!(moved.body().contains("name=\"step-1-type\" value=\"sort\""));
    assert!(moved.body().contains("name=\"step-2-type\" value=\"trim\""));

    assert_eq!(post("op=nonsense").await?.status(), StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn editing_shows_the_watch_as_it_is() -> TestResult {
    let app = app().await?;
    let watch = app.add_watch("shop", "https://shop.example/").await?;
    let response = app.get(&format!("/watch/{}/edit", watch.id)).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.body();
    assert!(body.contains("<h1>Edit watch</h1>"));
    assert!(body.contains(&format!("action=\"/watch/{}/edit\"", watch.id)));
    assert!(body.contains("value=\"shop\"") && body.contains("value=\"https://shop.example/\""));
    assert!(body.contains("name=\"every\" value=\"1h\""), "{body}");

    for bad in ["nonsense", "00000000-0000-7000-8000-000000000000"] {
        assert_eq!(
            app.get(&format!("/watch/{bad}/edit")).await?.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            app.post_form(&format!("/watch/{bad}/edit"), "url=https%3A%2F%2Fa.example")
                .await?
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    Ok(())
}

#[tokio::test]
async fn saving_changes_the_watch_but_only_resets_the_check_state_when_the_check_changed()
-> TestResult {
    let app = app().await?;
    let watch = app.add_watch("shop", "https://shop.example/").await?;
    app.store.record_error(watch.id, 1_000, 5, "boom").await?;
    app.store.record_error(watch.id, 2_000, 5, "boom").await?;
    let tag = app
        .store
        .create_tag(&NewTag {
            name: "t".into(),
            color: "red".into(),
            default_interval_secs: None,
        })
        .await?;
    let target = app
        .store
        .create_target(&NewTarget {
            name: "n".into(),
            url: "ntfy://ntfy.example/t".into(),
            default_template: None,
        })
        .await?;

    // The form, filled in as the edit page would show it, with a tag, a target and a template.
    let same = |extra: &[(&'static str, String)]| {
        let mut fields: Vec<(&str, String)> = vec![
            ("url", "https://shop.example/".into()),
            ("name", "shop".into()),
            ("every", "1h".into()),
            ("processor", "text".into()),
            ("retention", "100".into()),
            ("notify_added", "on".into()),
            ("notify_removed", "on".into()),
            ("notify_changed", "on".into()),
        ];
        fields.extend(extra.iter().cloned());
        let pairs: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
        form(&pairs)
    };
    let tags_targets = same(&[
        ("tag", tag.id.to_string()),
        ("target", target.id.to_string()),
        ("template", "hello".into()),
    ]);
    let response = app
        .post_form(&format!("/watch/{}/edit", watch.id), &tags_targets)
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "{}",
        response.body()
    );

    let after = app.store.get_watch(watch.id).await?;
    assert_eq!(
        after.state.consecutive_errors, 2,
        "tags, targets and template do not reset the streak"
    );
    assert_eq!(after.updated_at, watch.updated_at);
    assert_eq!(after.notification_template.as_deref(), Some("hello"));
    assert_eq!(app.store.tags_for_watch(watch.id).await?.len(), 1);
    assert_eq!(app.store.targets_for_watch(watch.id, false).await?.len(), 1);

    // Changing what is fetched does.
    let moved = same(&[]).replace("shop.example", "other.example");
    app.post_form(&format!("/watch/{}/edit", watch.id), &moved)
        .await?;
    let after = app.store.get_watch(watch.id).await?;
    assert_eq!(after.url, "https://other.example/");
    assert_eq!(
        after.state.consecutive_errors, 0,
        "an edit of the check starts the streak over"
    );
    // And leaving them out of the form removes them.
    assert!(app.store.tags_for_watch(watch.id).await?.is_empty());
    assert!(
        app.store
            .targets_for_watch(watch.id, false)
            .await?
            .is_empty()
    );
    assert_eq!(after.notification_template, None);
    Ok(())
}

#[tokio::test]
async fn an_invalid_edit_changes_nothing() -> TestResult {
    let app = app().await?;
    let watch = app.add_watch("shop", "https://shop.example/").await?;
    let body = form(&[
        ("url", "https://other.example/"),
        ("every", "never"),
        ("processor", "text"),
        ("notify_added", "on"),
    ]);
    let response = app
        .post_form(&format!("/watch/{}/edit", watch.id), &body)
        .await?;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        response
            .body()
            .contains(&format!("action=\"/watch/{}/edit\"", watch.id))
    );
    assert_eq!(
        app.store.get_watch(watch.id).await?.url,
        "https://shop.example/"
    );
    Ok(())
}

#[tokio::test]
async fn the_form_needs_a_session_and_a_request_from_this_site() -> TestResult {
    let app = app().await?;
    let watch = app.add_watch("w", "https://w.example/").await?;
    let guest = app.signed_out();
    for path in ["/watch/new".to_owned(), format!("/watch/{}/edit", watch.id)] {
        assert_eq!(
            guest.get(&path).await?.status(),
            StatusCode::SEE_OTHER,
            "{path}"
        );
        let body = form(&[
            ("url", "https://evil.example/"),
            ("processor", "text"),
            ("notify_added", "on"),
        ]);
        assert_eq!(
            guest.post_form(&path, &body).await?.status(),
            StatusCode::SEE_OTHER,
            "{path}"
        );
        let cross = app
            .post_form_from(&path, &body, "https://evil.example")
            .await?;
        assert_eq!(cross.status(), StatusCode::FORBIDDEN, "{path}");
    }
    assert_eq!(
        app.store.list_watches().await?.len(),
        1,
        "nothing was added"
    );
    assert_eq!(
        app.store.get_watch(watch.id).await?.url,
        "https://w.example/"
    );
    Ok(())
}

#[tokio::test]
async fn the_list_offers_to_add_a_watch() -> TestResult {
    let app = app().await?;
    let body = app.get("/").await?.into_body();
    assert!(body.contains("href=\"/watch/new\""), "{body}");
    Ok(())
}

#[tokio::test]
async fn saves_of_the_same_form_at_the_same_time_take_turns_instead_of_refusing_each_other()
-> TestResult {
    let app = app().await?;
    let tasks: Vec<_> = (0..16)
        .map(|n| {
            let app = app.clone();
            tokio::spawn(async move {
                let body = form(&[
                    ("draft", "one-form-saved-many-times"),
                    ("url", &format!("https://example.com/{n}")),
                    ("processor", "text"),
                    ("notify_added", "on"),
                    ("step-0-type", "trim"),
                ]);
                app.post_form("/watch/new", &body)
                    .await
                    .map(|response| response.status())
                    .map_err(|error| error.to_string())
            })
        })
        .collect();
    for task in tasks {
        let status = task.await??;
        assert_eq!(
            status,
            StatusCode::SEE_OTHER,
            "an overlapping save was refused"
        );
    }
    assert_eq!(app.store.list_watches().await?.len(), 16);
    Ok(())
}

#[tokio::test]
async fn the_whole_text_scope_is_saved_shown_and_can_be_turned_off() -> TestResult {
    let app = app().await?;
    let server = page("<p>Out of stock</p>").await;
    let url = server.uri();

    let body = form(&[
        ("name", "stock"),
        ("url", &url),
        ("processor", "text"),
        ("trigger_text", "out of stock"),
        ("scope_text", "on"),
        ("notify_added", "on"),
        ("notify_changed", "on"),
    ]);
    let response = app.post_form("/watch/new", &body).await?;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "{}",
        response.body()
    );
    let watch = app
        .store
        .list_watches()
        .await?
        .into_iter()
        .next()
        .ok_or("no watch was added")?;
    assert_eq!(watch.conditions.scope, Scope::Text);

    let shown = app.get(&format!("/watch/{}/edit", watch.id)).await?;
    assert!(
        shown.body().contains("name=\"scope_text\" checked"),
        "{}",
        shown.body()
    );

    let body = form(&[
        ("name", "stock"),
        ("url", &url),
        ("processor", "text"),
        ("trigger_text", "out of stock"),
        ("notify_added", "on"),
        ("notify_changed", "on"),
    ]);
    let response = app
        .post_form(&format!("/watch/{}/edit", watch.id), &body)
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "{}",
        response.body()
    );
    assert_eq!(
        app.store.get_watch(watch.id).await?.conditions.scope,
        Scope::Added
    );
    Ok(())
}

#[tokio::test]
async fn the_method_and_body_are_saved_shown_and_checked_together() -> TestResult {
    let app = app().await?;
    let server = page("<p>ok</p>").await;
    let url = server.uri();

    let body = form(&[
        ("name", "search"),
        ("url", &url),
        ("processor", "text"),
        ("method", "POST"),
        ("body", "q=widgets\r\npage=1"),
        ("notify_added", "on"),
    ]);
    let response = app.post_form("/watch/new", &body).await?;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "{}",
        response.body()
    );
    let watch = app
        .store
        .list_watches()
        .await?
        .into_iter()
        .next()
        .ok_or("no watch was added")?;
    assert_eq!(watch.fetch.method, vigil_core::Method::Post);
    assert_eq!(watch.fetch.body.as_deref(), Some("q=widgets\npage=1"));

    let shown = app.get(&format!("/watch/{}/edit", watch.id)).await?;
    assert!(
        shown.body().contains("<option value=\"POST\" selected>"),
        "{}",
        shown.body()
    );
    assert!(shown.body().contains("q=widgets"), "{}", shown.body());

    // The same body with GET is refused, and nothing is saved.
    let refused = form(&[
        ("name", "search"),
        ("url", &url),
        ("processor", "text"),
        ("method", "GET"),
        ("body", "q=widgets"),
        ("notify_added", "on"),
    ]);
    let response = app
        .post_form(&format!("/watch/{}/edit", watch.id), &refused)
        .await?;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        response.body().contains("A request body needs POST"),
        "{}",
        response.body()
    );
    assert_eq!(
        app.store.get_watch(watch.id).await?.fetch.method,
        vigil_core::Method::Post
    );

    let unknown = form(&[("url", &url), ("method", "TRACE"), ("notify_added", "on")]);
    let response = app.post_form("/watch/new", &unknown).await?;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        response.body().contains("Choose one of GET"),
        "{}",
        response.body()
    );
    Ok(())
}
