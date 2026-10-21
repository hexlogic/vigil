//! The watch list and the actions on its rows.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use vigil_store::{CheckSuccess, Content, NewTag};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::*;

async fn page(body: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/html; charset=utf-8"))
        .mount(&server)
        .await;
    server
}

fn htmx_post(path: &str) -> Result<Request<Body>, axum::http::Error> {
    Request::post(path)
        .header("hx-request", "true")
        .header("origin", "http://localhost")
        .body(Body::empty())
}

#[tokio::test]
async fn an_empty_list_says_so() -> TestResult {
    let app = app().await?;
    let body = app.get("/").await?.into_body();
    assert!(body.contains("No watches yet."), "{body}");
    assert!(!body.contains("<table"), "{body}");
    assert!(body.contains("0 watches"), "{body}");
    Ok(())
}

#[tokio::test]
async fn watches_are_listed_by_name_with_their_state() -> TestResult {
    let app = app().await?;
    let zebra = app.add_watch("zebra", "https://z.example/page").await?;
    app.add_watch("Apple", "https://a.example/").await?;
    let ok = app.add_watch("mango", "https://m.example/").await?;
    app.store
        .record_error(zebra.id, 1_000, 5, "connection refused")
        .await?;
    app.store
        .record_success(
            ok.id,
            &CheckSuccess {
                at: vigil_store::now_ms() - 5 * 60 * 1000,
                duration_ms: 3,
                http_status: Some(200),
                content_type: None,
                etag: None,
                last_modified: None,
                content: Some(Content {
                    text: "hi\n".into(),
                    raw_size: 2,
                }),
                settings_version: None,
            },
        )
        .await?;
    let paused = app.add_watch("paused one", "https://p.example/").await?;
    app.store.set_paused(paused.id, true).await?;

    let body = app.get("/").await?.into_body();
    let order: Vec<usize> = ["Apple", "mango", "paused one", "zebra"]
        .iter()
        .map(|name| body.find(&format!(">{name}</a>")).unwrap_or(usize::MAX))
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?} in {body}");
    assert!(body.contains("4 watches"));
    assert!(body.contains("Not checked yet"), "the pending one");
    assert!(body.contains("badge badge-ok\">OK<"), "{body}");
    assert!(body.contains("Error ×1"), "{body}");
    assert!(
        body.contains("connection refused"),
        "the last error is shown"
    );
    assert!(body.contains(">Paused<"));
    assert!(body.contains("5 minutes ago"), "{body}");
    assert!(
        body.contains("every") || body.contains("1h"),
        "the interval"
    );
    Ok(())
}

#[tokio::test]
async fn names_and_addresses_are_escaped() -> TestResult {
    let app = app().await?;
    app.add_watch("<script>alert(1)</script>", "https://x.example/?a=1&b=<b>")
        .await?;
    let body = app.get("/").await?.into_body();
    assert!(!body.contains("<script>alert"), "{body}");
    assert!(
        body.contains("&lt;script&gt;alert(1)&lt;/script&gt;") || body.contains("&#60;script&#62;"),
        "{body}"
    );
    assert!(!body.contains("<b>"), "{body}");
    Ok(())
}

#[tokio::test]
async fn the_list_can_be_narrowed_to_a_tag() -> TestResult {
    let app = app().await?;
    let news = app.add_watch("news site", "https://n.example/").await?;
    app.add_watch("shop", "https://s.example/").await?;
    let tag = app
        .store
        .create_tag(&NewTag {
            name: "daily".into(),
            color: "green".into(),
            default_interval_secs: None,
        })
        .await?;
    let odd = app
        .store
        .create_tag(&NewTag {
            name: "odd".into(),
            color: "url(javascript:x)".into(),
            default_interval_secs: None,
        })
        .await?;
    app.store.set_watch_tags(news.id, &[tag.id, odd.id]).await?;

    let all = app.get("/").await?.into_body();
    assert!(all.contains("news site") && all.contains("shop"));
    assert!(all.contains("class=\"tag tag-green\""), "{all}");
    assert!(
        all.contains("class=\"tag tag-gray\">odd<"),
        "a colour outside the palette is gray: {all}"
    );
    assert!(!all.contains("javascript:x"), "{all}");

    let only = app.get(&format!("/?tag={}", tag.id)).await?.into_body();
    assert!(
        only.contains("news site") && !only.contains(">shop</a>"),
        "{only}"
    );
    assert!(only.contains("2 watches"), "the count is of all watches");

    // A tag nobody has, or nonsense, filters nothing out.
    let nonsense = app.get("/?tag=nonsense").await?.into_body();
    assert!(nonsense.contains(">shop</a>"));
    let empty_tag = app
        .store
        .create_tag(&NewTag {
            name: "empty".into(),
            color: "red".into(),
            default_interval_secs: None,
        })
        .await?;
    let none = app
        .get(&format!("/?tag={}", empty_tag.id))
        .await?
        .into_body();
    assert!(none.contains("No watches have this tag."), "{none}");
    Ok(())
}

#[tokio::test]
async fn a_row_can_be_fetched_on_its_own() -> TestResult {
    let app = app().await?;
    let watch = app.add_watch("solo", "https://solo.example/").await?;
    let row = app.get(&format!("/watch/{}/row", watch.id)).await?;
    assert_eq!(row.status(), StatusCode::OK);
    let body = row.body();
    assert!(body.starts_with("<tr id=\"watch-"), "{body}");
    assert!(!body.contains("<html"), "a fragment, not a page");
    assert!(body.contains("solo"));

    for bad in ["nonsense", "00000000-0000-7000-8000-000000000000"] {
        assert_eq!(
            app.get(&format!("/watch/{bad}/row")).await?.status(),
            StatusCode::NOT_FOUND,
            "{bad}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn checking_now_fetches_the_page_and_answers_with_the_updated_row() -> TestResult {
    let app = app().await?;
    let server = page("<p>hello</p>").await;
    let watch = app.add_watch("hello", &server.uri()).await?;

    let response = app
        .send(htmx_post(&format!("/watch/{}/check", watch.id))?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.body();
    assert!(body.starts_with("<tr id=\"watch-"), "{body}");
    assert!(body.contains("badge badge-ok\">OK<"), "{body}");
    assert!(body.contains("just now"), "{body}");
    assert_eq!(app.store.list_snapshots(watch.id, 5).await?.len(), 1);
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn a_failing_page_shows_as_an_error_on_the_row() -> TestResult {
    let app = app().await?;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let watch = app.add_watch("broken", &server.uri()).await?;

    let body = app
        .send(htmx_post(&format!("/watch/{}/check", watch.id))?)
        .await?
        .into_body();
    assert!(body.contains("Error ×1"), "{body}");
    assert!(body.contains("500"), "the reason is shown: {body}");
    Ok(())
}

#[tokio::test]
async fn checking_without_htmx_goes_back_to_the_list() -> TestResult {
    let app = app().await?;
    let server = page("<p>x</p>").await;
    let watch = app.add_watch("plain", &server.uri()).await?;
    let response = app
        .post_form(&format!("/watch/{}/check", watch.id), "")
        .await?;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(header_of(&response, "location"), "/");
    assert_eq!(
        app.store.list_snapshots(watch.id, 5).await?.len(),
        1,
        "it was checked"
    );
    Ok(())
}

#[tokio::test]
async fn checking_a_watch_that_is_not_there_is_not_found() -> TestResult {
    let app = app().await?;
    let missing = "00000000-0000-7000-8000-000000000000";
    assert_eq!(
        app.send(htmx_post(&format!("/watch/{missing}/check"))?)
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.send(htmx_post("/watch/nonsense/check")?)
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn a_paused_watch_can_still_be_checked_by_hand() -> TestResult {
    let app = app().await?;
    let server = page("<p>x</p>").await;
    let watch = app.add_watch("held", &server.uri()).await?;
    app.store.set_paused(watch.id, true).await?;

    let body = app
        .send(htmx_post(&format!("/watch/{}/check", watch.id))?)
        .await?
        .into_body();
    assert!(body.contains(">Paused<"), "{body}");
    assert_eq!(
        app.store.list_snapshots(watch.id, 5).await?.len(),
        1,
        "it was checked"
    );
    assert!(
        app.store.get_watch(watch.id).await?.paused,
        "and is still paused"
    );
    Ok(())
}

#[tokio::test]
async fn a_watch_can_be_paused_and_resumed() -> TestResult {
    let app = app().await?;
    let watch = app.add_watch("w", "https://w.example/").await?;

    let paused = app
        .send(htmx_post(&format!("/watch/{}/pause", watch.id))?)
        .await?;
    assert_eq!(paused.status(), StatusCode::OK);
    assert!(
        paused.body().contains(">Paused<") && paused.body().contains(">Resume<"),
        "{}",
        paused.body()
    );
    assert!(
        paused
            .body()
            .contains(&format!("/watch/{}/resume", watch.id))
    );
    assert!(app.store.get_watch(watch.id).await?.paused);

    let resumed = app
        .send(htmx_post(&format!("/watch/{}/resume", watch.id))?)
        .await?;
    assert!(resumed.body().contains(">Pause<"), "{}", resumed.body());
    assert!(!app.store.get_watch(watch.id).await?.paused);

    let missing = "00000000-0000-7000-8000-000000000000";
    assert_eq!(
        app.send(htmx_post(&format!("/watch/{missing}/pause"))?)
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn deleting_asks_first_and_then_removes_the_watch() -> TestResult {
    let app = app().await?;
    let watch = app.add_watch("doomed", "https://d.example/").await?;

    let ask = app.get(&format!("/watch/{}/delete", watch.id)).await?;
    assert_eq!(ask.status(), StatusCode::OK);
    assert!(
        ask.body().contains("Delete “doomed” and its history?"),
        "{}",
        ask.body()
    );
    assert!(
        app.store.get_watch(watch.id).await.is_ok(),
        "asking deletes nothing"
    );

    let done = app
        .send(htmx_post(&format!("/watch/{}/delete", watch.id))?)
        .await?;
    assert_eq!(done.status(), StatusCode::OK);
    assert!(done.body().is_empty(), "the row is swapped for nothing");
    assert!(matches!(
        app.store.get_watch(watch.id).await,
        Err(vigil_store::StoreError::NotFound(_))
    ));

    // Doing it again is fine, and asking about a watch that is gone is not found.
    let again = app
        .send(htmx_post(&format!("/watch/{}/delete", watch.id))?)
        .await?;
    assert_eq!(again.status(), StatusCode::OK);
    assert_eq!(
        app.get(&format!("/watch/{}/delete", watch.id))
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn deleting_without_htmx_goes_back_to_the_list() -> TestResult {
    let app = app().await?;
    let watch = app.add_watch("plain", "https://p.example/").await?;
    let response = app
        .post_form(&format!("/watch/{}/delete", watch.id), "")
        .await?;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(app.store.get_watch(watch.id).await.is_err());
    Ok(())
}

#[tokio::test]
async fn nothing_can_be_done_to_a_watch_without_signing_in() -> TestResult {
    let app = app().await?;
    let server = page("<p>x</p>").await;
    let watch = app.add_watch("guarded", &server.uri()).await?;
    let guest = app.signed_out();

    for action in ["check", "pause", "resume", "delete"] {
        let response = guest
            .send(htmx_post(&format!("/watch/{}/{action}", watch.id))?)
            .await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{action}");
        assert!(!header_of(&response, "hx-redirect").is_empty(), "{action}");
    }
    for path in [
        format!("/watch/{}/row", watch.id),
        format!("/watch/{}/delete", watch.id),
        "/".to_owned(),
    ] {
        assert_eq!(
            guest.get(&path).await?.status(),
            StatusCode::SEE_OTHER,
            "{path}"
        );
    }
    assert!(app.store.get_watch(watch.id).await.is_ok());
    assert!(!app.store.get_watch(watch.id).await?.paused);
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "nothing was fetched"
    );
    Ok(())
}

#[tokio::test]
async fn an_action_from_another_site_is_refused() -> TestResult {
    let app = app().await?;
    let server = page("<p>x</p>").await;
    let watch = app.add_watch("guarded", &server.uri()).await?;

    for action in ["check", "pause", "delete"] {
        let response = app
            .post_form_from(
                &format!("/watch/{}/{action}", watch.id),
                "",
                "https://evil.example",
            )
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{action}");
    }
    assert!(app.store.get_watch(watch.id).await.is_ok());
    assert!(!app.store.get_watch(watch.id).await?.paused);
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn the_list_speaks_ukrainian_on_request() -> TestResult {
    let app = app().await?;
    app.add_watch("w", "https://w.example/").await?;
    let body = app.get_in("/", "uk").await?.into_body();
    assert!(
        body.contains("Не перевірено") || body.contains("Ще не перевірено"),
        "{body}"
    );
    assert!(
        body.contains(">Перевірити зараз<") || body.contains("Перевірити зараз"),
        "{body}"
    );
    assert!(body.contains("1 відстеження"), "{body}");
    Ok(())
}
