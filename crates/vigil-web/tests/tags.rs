//! The tags page and its forms.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use vigil_core::Schedule;
use vigil_store::{NewTag, NewTarget, Tag};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::*;

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
    color: &str,
    every: Option<u64>,
) -> Result<Tag, Box<dyn std::error::Error>> {
    Ok(app
        .store
        .create_tag(&NewTag {
            name: name.into(),
            color: color.into(),
            default_interval_secs: every,
        })
        .await?)
}

#[tokio::test]
async fn an_empty_list_invites_adding_a_tag() -> TestResult {
    let app = app().await?;
    let body = app.get("/tags").await?.into_body();
    assert!(body.contains("<h1>Tags</h1>"), "{body}");
    assert!(body.contains("No tags yet."), "{body}");
    assert!(body.contains("href=\"/tags/new\""), "{body}");
    assert!(body.contains("aria-current=\"page\">Tags<"), "{body}");
    Ok(())
}

#[tokio::test]
async fn tags_are_listed_with_their_defaults_and_watch_counts() -> TestResult {
    let app = app().await?;
    let news = add(&app, "news", "green", Some(900)).await?;
    add(&app, "alpha", "weird colour", None).await?;
    let target = app
        .store
        .create_target(&NewTarget {
            name: "phone".into(),
            url: "ntfy://h.example/t".into(),
            default_template: None,
        })
        .await?;
    app.store.set_tag_targets(news.id, &[target.id]).await?;
    let watch = app.add_watch("w", "https://w.example/").await?;
    app.store.set_watch_tags(watch.id, &[news.id]).await?;

    let body = app.get("/tags").await?.into_body();
    assert!(body.contains("class=\"tag tag-green\">news<"), "{body}");
    assert!(
        body.contains("class=\"tag tag-gray\">alpha<"),
        "a colour outside the palette is gray: {body}"
    );
    assert!(!body.contains("weird colour"));
    assert!(body.contains("checks every 15m by default"), "{body}");
    assert!(body.contains("no default interval"), "{body}");
    assert!(body.contains("notifies phone"), "{body}");
    assert!(body.contains("1 watch"), "{body}");
    assert!(
        body.contains(&format!("href=\"/?tag={}\"", news.id)),
        "the tag links to the filtered list: {body}"
    );
    let (a, n) = (body.find(">alpha<"), body.find(">news<"));
    assert!(a < n, "listed by name");
    Ok(())
}

#[tokio::test]
async fn a_tag_is_added_from_the_form() -> TestResult {
    let app = app().await?;
    let target = app
        .store
        .create_target(&NewTarget {
            name: "phone".into(),
            url: "ntfy://h.example/t".into(),
            default_template: None,
        })
        .await?;
    let page = app.get("/tags/new").await?;
    assert_eq!(page.status(), StatusCode::OK);
    let body = page.body();
    assert!(body.contains("<h1>Add a tag</h1>"), "{body}");
    for color in [
        "gray", "red", "orange", "yellow", "green", "teal", "blue", "purple", "pink",
    ] {
        assert!(
            body.contains(&format!("name=\"color\" value=\"{color}\"")),
            "{color}"
        );
    }
    assert!(
        body.contains("name=\"color\" value=\"gray\" checked"),
        "gray is the default: {body}"
    );
    assert!(body.contains(&format!("name=\"target\" value=\"{}\"", target.id)));
    assert!(!body.contains(" style="));

    let response = app
        .post_form(
            "/tags/new",
            &form(&[
                ("name", "  shops  "),
                ("color", "teal"),
                ("every", "30m"),
                ("target", &target.id.to_string()),
            ]),
        )
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "{}",
        response.body()
    );
    assert_eq!(header_of(&response, "location"), "/tags");

    let tags = app.store.list_tags().await?;
    assert_eq!(tags.len(), 1);
    assert_eq!(
        (
            tags[0].name.as_str(),
            tags[0].color.as_str(),
            tags[0].default_interval_secs
        ),
        ("shops", "teal", Some(1800))
    );
    assert_eq!(app.store.tag_targets(tags[0].id).await?, [target.id]);
    Ok(())
}

#[tokio::test]
async fn a_colour_outside_the_palette_is_stored_as_gray() -> TestResult {
    let app = app().await?;
    app.post_form(
        "/tags/new",
        &form(&[("name", "x"), ("color", "expression(alert(1))")]),
    )
    .await?;
    assert_eq!(app.store.list_tags().await?[0].color, "gray");
    Ok(())
}

#[tokio::test]
async fn a_tag_with_problems_is_shown_again() -> TestResult {
    let app = app().await?;
    add(&app, "taken", "red", None).await?;
    let cases = [
        (
            form(&[("name", ""), ("color", "red")]),
            "Give the tag a name.",
        ),
        (form(&[("name", &"x".repeat(51))]), "at most 50 characters"),
        (
            form(&[("name", "taken")]),
            "Another tag already has this name.",
        ),
        (
            form(&[("name", "n"), ("every", "soon")]),
            "Not an interval.",
        ),
        (form(&[("name", "n"), ("every", "0s")]), "at least 1s"),
        (
            form(&[("name", "n"), ("target", "nonsense")]),
            "That choice no longer exists.",
        ),
    ];
    for (body, message) in cases {
        let response = app.post_form("/tags/new", &body).await?;
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
    assert_eq!(app.store.list_tags().await?.len(), 1, "nothing was added");
    // What was typed stays.
    let again = app
        .post_form(
            "/tags/new",
            &form(&[
                ("name", "kept \"as is\""),
                ("color", "pink"),
                ("every", "soon"),
            ]),
        )
        .await?;
    assert!(
        again.body().contains("value=\"kept &#34;as is&#34;\""),
        "{}",
        again.body()
    );
    assert!(
        again
            .body()
            .contains("name=\"color\" value=\"pink\" checked"),
        "{}",
        again.body()
    );
    Ok(())
}

#[tokio::test]
async fn a_tag_is_edited_and_its_targets_replaced() -> TestResult {
    let app = app().await?;
    let tag = add(&app, "old", "red", Some(60)).await?;
    let (a, b) = (
        app.store
            .create_target(&NewTarget {
                name: "a".into(),
                url: "ntfy://h.example/a".into(),
                default_template: None,
            })
            .await?,
        app.store
            .create_target(&NewTarget {
                name: "b".into(),
                url: "ntfy://h.example/b".into(),
                default_template: None,
            })
            .await?,
    );
    app.store.set_tag_targets(tag.id, &[a.id]).await?;

    let page = app
        .get(&format!("/tags/{}/edit", tag.id))
        .await?
        .into_body();
    assert!(
        page.contains("<h1>Edit tag</h1>") && page.contains("value=\"old\""),
        "{page}"
    );
    assert!(
        page.contains("name=\"color\" value=\"red\" checked"),
        "{page}"
    );
    assert!(page.contains("name=\"every\" value=\"1m\""), "{page}");
    assert!(
        page.contains(&format!("name=\"target\" value=\"{}\" checked", a.id)),
        "{page}"
    );
    assert!(!page.contains(&format!("name=\"target\" value=\"{}\" checked", b.id)));

    let response = app
        .post_form(
            &format!("/tags/{}/edit", tag.id),
            &form(&[
                ("name", "new"),
                ("color", "blue"),
                ("every", ""),
                ("target", &b.id.to_string()),
            ]),
        )
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "{}",
        response.body()
    );
    let after = app.store.get_tag(tag.id).await?;
    assert_eq!(
        (
            after.name.as_str(),
            after.color.as_str(),
            after.default_interval_secs
        ),
        ("new", "blue", None)
    );
    assert_eq!(app.store.tag_targets(tag.id).await?, [b.id]);

    add(&app, "taken", "red", None).await?;
    let clash = app
        .post_form(
            &format!("/tags/{}/edit", tag.id),
            &form(&[("name", "taken")]),
        )
        .await?;
    assert_eq!(clash.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let missing = "00000000-0000-7000-8000-000000000000";
    assert_eq!(
        app.get(&format!("/tags/{missing}/edit")).await?.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.post_form(&format!("/tags/{missing}/edit"), &form(&[("name", "x")]))
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get("/tags/nonsense/edit").await?.status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn deleting_a_tag_asks_first_and_leaves_the_watches_alone() -> TestResult {
    let app = app().await?;
    let tag = add(&app, "doomed", "red", None).await?;
    let watch = app.add_watch("w", "https://w.example/").await?;
    app.store.set_watch_tags(watch.id, &[tag.id]).await?;

    let ask = app.get(&format!("/tags/{}/delete", tag.id)).await?;
    assert!(
        ask.body()
            .contains("Delete the tag “doomed”? It is taken off 1 watch."),
        "{}",
        ask.body()
    );
    assert!(app.store.get_tag(tag.id).await.is_ok());

    let done = app
        .send(htmx_post(&format!("/tags/{}/delete", tag.id))?)
        .await?;
    assert_eq!((done.status(), done.body().as_str()), (StatusCode::OK, ""));
    assert!(app.store.get_tag(tag.id).await.is_err());
    assert!(app.store.get_watch(watch.id).await.is_ok());
    assert!(app.store.tags_for_watch(watch.id).await?.is_empty());
    assert_eq!(
        app.send(htmx_post(&format!("/tags/{}/delete", tag.id))?)
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.get(&format!("/tags/{}/row", tag.id)).await?.status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn a_tag_gives_its_watches_a_default_interval_and_its_targets_their_notifications()
-> TestResult {
    let app = app().await?;
    // The tag, with a default interval and a default target that is a local ntfy server.
    let ntfy = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&ntfy)
        .await;
    let target = app
        .store
        .create_target(&NewTarget {
            name: "local".into(),
            url: format!("ntfy://127.0.0.1:{}/t", ntfy.address().port()),
            default_template: None,
        })
        .await?;
    let created = app
        .post_form(
            "/tags/new",
            &form(&[
                ("name", "shops"),
                ("color", "blue"),
                ("every", "10m"),
                ("target", &target.id.to_string()),
            ]),
        )
        .await?;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let tag = app.store.list_tags().await?.remove(0);

    // A watch added with the tag, and no interval of its own.
    let page = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("<p>v1</p>", "text/html"))
        .mount(&page)
        .await;
    let added = app
        .post_form(
            "/watch/new",
            &form(&[
                ("draft", "tags-e2e"),
                ("url", &page.uri()),
                ("every", ""),
                ("processor", "text"),
                ("notify_added", "on"),
                ("notify_changed", "on"),
                ("tag", &tag.id.to_string()),
            ]),
        )
        .await?;
    assert_eq!(added.status(), StatusCode::SEE_OTHER, "{}", added.body());
    let watch = app.store.list_watches().await?.remove(0);
    assert_eq!(
        watch.schedule,
        Schedule::Interval(600),
        "the tag's default interval"
    );

    // The first check stores the baseline; a change notifies the tag's target, not any
    // target the watch itself has.
    let mut waited = 0;
    while app.store.list_snapshots(watch.id, 1).await?.is_empty() && waited < 200 {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        waited += 1;
    }
    page.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("<p>v2</p>", "text/html"))
        .mount(&page)
        .await;
    assert!(
        app.store
            .targets_for_watch(watch.id, false)
            .await?
            .is_empty()
    );
    let checked = app.checker.check(watch.id).await?;
    assert!(
        matches!(checked.result, vigil_scheduler::CheckResult::Changed { .. }),
        "{checked:?}"
    );
    app.notifier.close_and_wait().await;
    assert_eq!(
        ntfy.received_requests().await.unwrap_or_default().len(),
        1,
        "the tag's target was notified"
    );
    Ok(())
}

#[tokio::test]
async fn everything_here_needs_a_session_and_a_request_from_this_site() -> TestResult {
    let app = app().await?;
    let tag = add(&app, "t", "red", None).await?;
    let guest = app.signed_out();
    for path in [
        "/tags".to_owned(),
        "/tags/new".to_owned(),
        format!("/tags/{}/edit", tag.id),
        format!("/tags/{}/row", tag.id),
        format!("/tags/{}/delete", tag.id),
    ] {
        assert_eq!(
            guest.get(&path).await?.status(),
            StatusCode::SEE_OTHER,
            "{path}"
        );
    }
    assert_eq!(
        guest
            .send(htmx_post(&format!("/tags/{}/delete", tag.id))?)
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.post_form_from(
            &format!("/tags/{}/delete", tag.id),
            "",
            "https://evil.example"
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.post_form_from(
            "/tags/new",
            &form(&[("name", "evil")]),
            "https://evil.example"
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(app.store.list_tags().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn the_page_speaks_ukrainian_on_request() -> TestResult {
    let app = app().await?;
    add(&app, "t", "red", None).await?;
    let body = app.get_in("/tags", "uk").await?.into_body();
    assert!(
        body.contains("<h1>Мітки</h1>") && body.contains("без типового інтервалу"),
        "{body}"
    );
    let form = app.get_in("/tags/new", "uk").await?.into_body();
    assert!(
        form.contains("Червоний") && form.contains("Бірюзовий"),
        "{form}"
    );
    Ok(())
}
