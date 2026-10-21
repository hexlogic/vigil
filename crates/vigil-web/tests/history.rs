//! The page of one watch, its history, one stored version and the difference between two.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use vigil_core::Watch;
use vigil_store::{
    CheckSuccess, Content, DeliveryOutcome, DeliveryRecord, NewTag, NewTarget, RecordOutcome,
    SnapshotId, now_ms,
};

use common::*;

/// Record a successful check that found `text`; the id of the version it made, if it made one.
async fn record(
    app: &App,
    watch: &Watch,
    text: &str,
) -> Result<Option<SnapshotId>, Box<dyn std::error::Error>> {
    let outcome = app
        .store
        .record_success(
            watch.id,
            &CheckSuccess {
                at: now_ms(),
                duration_ms: 12,
                http_status: Some(200),
                content_type: Some("text/html".into()),
                etag: None,
                last_modified: None,
                content: Some(Content {
                    text: text.into(),
                    raw_size: 2048,
                }),
                settings_version: None,
            },
        )
        .await?;
    Ok(match outcome {
        RecordOutcome::Baseline { snapshot } => Some(snapshot),
        RecordOutcome::Changed { current, .. } => Some(current),
        RecordOutcome::Unchanged => None,
    })
}

/// A watch with two versions, and their ids (older, newer).
async fn two_versions(
    app: &App,
) -> Result<(Watch, SnapshotId, SnapshotId), Box<dyn std::error::Error>> {
    let watch = app.add_watch("shop", "https://shop.example/").await?;
    let old = record(app, &watch, "Widget\nPrice: $10\nStock: yes\n")
        .await?
        .ok_or("no baseline")?;
    let new = record(app, &watch, "Widget\nPrice: $12\nStock: yes\n")
        .await?
        .ok_or("no change")?;
    Ok((watch, old, new))
}

#[tokio::test]
async fn the_page_of_a_watch_shows_its_settings_and_what_happened_lately() -> TestResult {
    let app = app().await?;
    let (watch, _, _) = two_versions(&app).await?;
    let tag = app
        .store
        .create_tag(&NewTag {
            name: "shops".into(),
            color: "green".into(),
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
    app.store.set_watch_tags(watch.id, &[tag.id]).await?;
    app.store.link_target(watch.id, target.id).await?;
    app.store
        .record_error(watch.id, now_ms(), 40, "connection refused")
        .await?;
    app.store
        .record_delivery(&DeliveryRecord {
            watch: Some(watch.id),
            target: target.id,
            snapshot: None,
            at: now_ms(),
            attempts: 2,
            outcome: DeliveryOutcome::Failed,
            error: Some("503 from the server".into()),
        })
        .await?;

    let response = app.get(&format!("/watch/{}", watch.id)).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.body();
    assert!(body.contains("<title>shop · Vigil</title>"), "{body}");
    assert!(
        body.contains(
            "href=\"https://shop.example/\" rel=\"noreferrer noopener\" target=\"_blank\""
        ),
        "{body}"
    );
    assert!(body.contains("<dd class=\"mono\">1h</dd>"), "{body}");
    assert!(body.contains("Text or a web page"), "{body}");
    assert!(body.contains("class=\"tag tag-green\">shops<"), "{body}");
    assert!(body.contains("<dd>phone</dd>"), "{body}");
    assert!(body.contains("2 versions"), "{body}");
    assert!(
        body.contains("connection refused"),
        "the last error: {body}"
    );
    assert!(
        body.contains("Failed") && body.contains("503 from the server"),
        "{body}"
    );
    assert!(
        body.contains("Changed") && body.contains("Failed"),
        "the check log: {body}"
    );
    assert!(body.contains("Latest change"), "{body}");
    assert!(
        body.contains(&format!("/watch/{}/history", watch.id)),
        "{body}"
    );
    assert!(!body.contains(" style="));
    Ok(())
}

#[tokio::test]
async fn a_watch_without_history_says_so() -> TestResult {
    let app = app().await?;
    let watch = app.add_watch("new", "https://n.example/").await?;
    let body = app.get(&format!("/watch/{}", watch.id)).await?.into_body();
    assert!(body.contains("Not checked yet."), "{body}");
    assert!(body.contains("No notification has been sent"), "{body}");
    assert!(
        !body.contains("Latest change"),
        "one version gives nothing to compare"
    );
    Ok(())
}

#[tokio::test]
async fn the_buttons_on_the_page_of_a_watch_come_back_to_it() -> TestResult {
    let app = app().await?;
    let watch = app.add_watch("w", "https://w.example/").await?;
    let page = format!("/watch/{}", watch.id);

    let paused = app
        .post_form(
            &format!("/watch/{}/pause", watch.id),
            &format!("return={page}"),
        )
        .await?;
    assert_eq!(paused.status(), StatusCode::SEE_OTHER);
    assert_eq!(header_of(&paused, "location"), page);
    assert!(app.store.get_watch(watch.id).await?.paused);

    // Only a place on this site is a place to come back to.
    for evil in ["//evil.example", "https://evil.example/", "/\\evil.example"] {
        let response = app
            .post_form(
                &format!("/watch/{}/resume", watch.id),
                &format!(
                    "return={}",
                    url::form_urlencoded::byte_serialize(evil.as_bytes()).collect::<String>()
                ),
            )
            .await?;
        assert_eq!(header_of(&response, "location"), "/", "{evil}");
    }
    Ok(())
}

#[tokio::test]
async fn the_history_lists_the_versions_newest_first() -> TestResult {
    let app = app().await?;
    let (watch, old, new) = two_versions(&app).await?;
    let response = app.get(&format!("/watch/{}/history", watch.id)).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.body();
    assert!(body.contains("History of shop"), "{body}");
    assert!(body.contains("2 versions kept, of at most 100."), "{body}");
    let (newer_at, older_at) = (
        body.find(&new.to_string()).ok_or("no newer")?,
        body.find(&old.to_string()).ok_or("no older")?,
    );
    assert!(newer_at < older_at, "newest first");
    assert!(body.contains(">newest<"), "{body}");
    assert!(body.contains("2.0 KiB"), "{body}");
    // The newer version links to its difference from the older; the oldest has no such link.
    assert!(
        body.contains(&format!("/watch/{}/diff?from={old}&#38;to={new}", watch.id)),
        "{body}"
    );
    assert_eq!(body.matches("Changes from the version before").count(), 1);

    let empty = app.add_watch("empty", "https://e.example/").await?;
    assert!(
        app.get(&format!("/watch/{}/history", empty.id))
            .await?
            .body()
            .contains("Nothing has been stored yet.")
    );
    Ok(())
}

#[tokio::test]
async fn one_version_can_be_read() -> TestResult {
    let app = app().await?;
    let watch = app.add_watch("shop", "https://shop.example/").await?;
    let old = record(&app, &watch, "first <b>version</b>\n")
        .await?
        .ok_or("baseline")?;
    let new = record(&app, &watch, "second version\n")
        .await?
        .ok_or("change")?;

    let response = app
        .get(&format!("/watch/{}/history/{old}", watch.id))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.body();
    assert!(body.contains("A version of shop"), "{body}");
    assert!(body.contains("HTTP 200 · text/html · 2.0 KiB"), "{body}");
    assert!(
        body.contains("first &#60;b&#62;version&#60;/b&#62;"),
        "escaped: {body}"
    );
    assert!(
        !body.contains("Changes from the version before"),
        "the oldest has no earlier version"
    );

    let newer = app
        .get(&format!("/watch/{}/history/{new}", watch.id))
        .await?
        .into_body();
    assert!(newer.contains(&format!("from={old}")), "{newer}");
    Ok(())
}

#[tokio::test]
async fn a_version_of_another_watch_or_a_made_up_one_is_not_found() -> TestResult {
    let app = app().await?;
    let (watch, old, _) = two_versions(&app).await?;
    let other = app.add_watch("other", "https://o.example/").await?;
    for path in [
        format!("/watch/{}/history/{old}", other.id),
        format!(
            "/watch/{}/history/00000000-0000-7000-8000-000000000000",
            watch.id
        ),
        format!("/watch/{}/history/nonsense", watch.id),
        "/watch/nonsense/history".to_owned(),
        "/watch/00000000-0000-7000-8000-000000000000".to_owned(),
        "/watch/00000000-0000-7000-8000-000000000000/history".to_owned(),
    ] {
        assert_eq!(
            app.get(&path).await?.status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn the_diff_page_shows_the_latest_change_by_default() -> TestResult {
    let app = app().await?;
    let (watch, old, new) = two_versions(&app).await?;
    let response = app.get(&format!("/watch/{}/diff", watch.id)).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.body();
    assert!(body.contains("Changes to shop"), "{body}");
    assert!(body.contains("+1 −1 lines"), "{body}");
    assert!(
        body.contains("class=\"dl del\"") && body.contains("class=\"dl add\""),
        "{body}"
    );
    assert!(
        body.contains("Price: <mark>$10</mark>") && body.contains("Price: <mark>$12</mark>"),
        "{body}"
    );
    // The lines around the change are shown, and nothing is left out of so small a text.
    assert!(
        body.contains("Widget") && body.contains("Stock: yes"),
        "{body}"
    );
    assert!(!body.contains("class=\"gap\""), "{body}");
    // The pickers have both versions, with the older as the start and the newer as the end.
    assert!(
        body.contains(&format!("<option value=\"{old}\" selected>")),
        "{body}"
    );
    assert!(
        body.contains(&format!("<option value=\"{new}\" selected>")),
        "{body}"
    );
    assert!(body.contains("aria-current=\"true\">Inline<"), "{body}");
    assert!(
        !body.contains(" style="),
        "the page policy forbids style attributes"
    );
    Ok(())
}

#[tokio::test]
async fn the_link_in_a_notification_opens_the_same_diff() -> TestResult {
    let app = app().await?;
    let (watch, old, new) = two_versions(&app).await?;
    // The address the notifier builds: /watch/{id}/diff?from={previous}&to={current}.
    let by_link = app
        .get(&format!("/watch/{}/diff?from={old}&to={new}", watch.id))
        .await?;
    let by_default = app.get(&format!("/watch/{}/diff", watch.id)).await?;
    assert_eq!(by_link.status(), StatusCode::OK);
    let marks = |html: &str| html.matches("<mark>").count();
    assert_eq!(marks(by_link.body()), marks(by_default.body()));
    assert!(by_link.body().contains("<mark>$12</mark>"));

    // Either end alone is enough.
    let only_to = app
        .get(&format!("/watch/{}/diff?to={new}", watch.id))
        .await?;
    assert!(only_to.body().contains("<mark>$12</mark>"));
    let only_from = app
        .get(&format!("/watch/{}/diff?from={old}", watch.id))
        .await?;
    assert!(only_from.body().contains("<mark>$12</mark>"));
    Ok(())
}

#[tokio::test]
async fn the_diff_can_be_read_side_by_side() -> TestResult {
    let app = app().await?;
    let (watch, old, new) = two_versions(&app).await?;
    let response = app
        .get(&format!(
            "/watch/{}/diff?from={old}&to={new}&view=side",
            watch.id
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.body();
    assert!(body.contains("class=\"sbs\""), "{body}");
    assert!(
        body.contains("class=\"cell del\"") && body.contains("class=\"cell add\""),
        "{body}"
    );
    assert!(!body.contains("class=\"dl "), "not the inline layout");
    assert!(
        body.contains("aria-current=\"true\">Side by side<"),
        "{body}"
    );
    assert!(
        body.contains("name=\"view\" value=\"side\""),
        "the pickers keep the layout: {body}"
    );
    assert!(
        !body.contains(" style="),
        "the page policy forbids style attributes"
    );
    Ok(())
}

#[tokio::test]
async fn comparing_a_version_with_itself_or_in_reverse_works() -> TestResult {
    let app = app().await?;
    let (watch, old, new) = two_versions(&app).await?;
    let same = app
        .get(&format!("/watch/{}/diff?from={new}&to={new}", watch.id))
        .await?;
    assert!(same.body().contains("No difference."), "{}", same.body());
    assert!(!same.body().contains("class=\"diff\""));

    let reversed = app
        .get(&format!("/watch/{}/diff?from={new}&to={old}", watch.id))
        .await?;
    assert_eq!(reversed.status(), StatusCode::OK);
    assert!(
        reversed.body().contains("<mark>$10</mark>")
            && reversed.body().contains("<mark>$12</mark>")
    );
    Ok(())
}

#[tokio::test]
async fn a_version_that_retention_removed_gets_a_page_that_says_so() -> TestResult {
    let app = app().await?;
    let mut new =
        vigil_store::NewWatch::new("https://short.example/", vigil_core::Schedule::Interval(60));
    new.snapshot_retention = 1;
    let watch = app.store.create_watch(new).await?;
    let first = record(&app, &watch, "one\n").await?.ok_or("baseline")?;
    let second = record(&app, &watch, "two\n").await?.ok_or("change")?;

    let response = app
        .get(&format!(
            "/watch/{}/diff?from={first}&to={second}",
            watch.id
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response.body();
    assert!(body.contains("That version is gone"), "{body}");
    assert!(
        body.contains(&format!("/watch/{}/history", watch.id)),
        "{body}"
    );
    assert!(body.contains("1 version"), "what is left is said: {body}");
    Ok(())
}

#[tokio::test]
async fn the_diff_page_refuses_versions_that_are_not_this_watchs() -> TestResult {
    let app = app().await?;
    let (watch, old, new) = two_versions(&app).await?;
    let other = app.add_watch("other", "https://o.example/").await?;
    let foreign = record(&app, &other, "somewhere else\n")
        .await?
        .ok_or("baseline")?;
    for query in [
        format!("from={foreign}&to={new}"),
        format!("from={old}&to={foreign}"),
        "from=nonsense".to_owned(),
        "to=%2F%2Fevil".to_owned(),
    ] {
        let response = app
            .get(&format!("/watch/{}/diff?{query}", watch.id))
            .await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{query}");
        assert!(!response.body().contains("somewhere else"), "{query}");
    }
    Ok(())
}

#[tokio::test]
async fn a_watch_with_fewer_than_two_versions_has_nothing_to_compare() -> TestResult {
    let app = app().await?;
    let none = app.add_watch("none", "https://n.example/").await?;
    let one = app.add_watch("one", "https://o.example/").await?;
    record(&app, &one, "only\n").await?;
    for watch in [none, one] {
        let response = app.get(&format!("/watch/{}/diff", watch.id)).await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.body().contains("needs two stored versions"),
            "{}",
            response.body()
        );
    }
    Ok(())
}

#[tokio::test]
async fn these_pages_need_a_session() -> TestResult {
    let app = app().await?;
    let (watch, old, new) = two_versions(&app).await?;
    let guest = app.signed_out();
    for path in [
        format!("/watch/{}", watch.id),
        format!("/watch/{}/history", watch.id),
        format!("/watch/{}/history/{old}", watch.id),
        format!("/watch/{}/diff?from={old}&to={new}", watch.id),
    ] {
        let response = guest.get(&path).await?;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{path}");
        assert!(
            header_of(&response, "location").starts_with("/login?next="),
            "{path}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn the_pages_speak_ukrainian_on_request() -> TestResult {
    let app = app().await?;
    let (watch, _, _) = two_versions(&app).await?;
    let diff = app
        .send(
            Request::get(format!("/watch/{}/diff", watch.id))
                .header("accept-language", "uk")
                .body(Body::empty())?,
        )
        .await?
        .into_body();
    assert!(diff.contains("Зміни: shop"), "{diff}");
    assert!(diff.contains("+1 −1 рядків"), "{diff}");
    let history = app
        .get_in(&format!("/watch/{}/history", watch.id), "uk")
        .await?
        .into_body();
    assert!(
        history.contains("Історія: shop")
            && history.contains("Збережено 2 версії із щонайбільше 100."),
        "{history}"
    );
    Ok(())
}
