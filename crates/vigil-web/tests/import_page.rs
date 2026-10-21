//! The import page: choose a file, read the preview, confirm.

mod common;

use std::io::Write;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use vigil_web::{Auth, WebConfig};

use common::*;

const BOUNDARY: &str = "----vigil-test-boundary";

/// A `multipart/form-data` body of text fields and, optionally, a file.
fn multipart(fields: &[(&str, &str)], file: Option<(&str, &[u8])>) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in fields {
        let _ = write!(
            body,
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        );
    }
    let (filename, content) = file.unwrap_or(("", b""));
    let _ = write!(
        body,
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    );
    body.extend_from_slice(content);
    let _ = write!(body, "\r\n--{BOUNDARY}--\r\n");
    body
}

async fn send(
    app: &App,
    path: &str,
    body: Vec<u8>,
) -> Result<axum::http::Response<String>, Box<dyn std::error::Error>> {
    app.send(
        Request::post(path)
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={BOUNDARY}"),
            )
            .header(header::ORIGIN, &app.origin)
            .body(Body::from(body))?,
    )
    .await
}

fn plan_id(page: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(page
        .split("name=\"plan\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .ok_or("the page has no plan to confirm")?
        .to_owned())
}

#[tokio::test]
async fn the_form_is_in_the_menu_and_in_both_languages() -> TestResult {
    let app = app().await?;
    let page = app.get("/import").await?;
    assert_eq!(page.status(), StatusCode::OK);
    assert!(page.body().contains("href=\"/import\""), "{}", page.body());
    assert!(page.body().contains("enctype=\"multipart/form-data\""));
    assert!(page.body().contains("Backup of another change monitor"));
    let uk = app.get_in("/import", "uk").await?;
    assert!(uk.body().contains("Перевіряти кожні"), "{}", uk.body());
    Ok(())
}

#[tokio::test]
async fn pasted_addresses_are_previewed_then_made_when_confirmed() -> TestResult {
    let app = app().await?;
    let body = multipart(
        &[
            (
                "urls",
                "https://a.example/pricing news\nhttps://b.example/\nftp://nope.example/\n",
            ),
            ("format", ""),
            ("every", "30m"),
        ],
        None,
    );
    let preview = send(&app, "/import", body).await?;
    assert_eq!(preview.status(), StatusCode::OK, "{}", preview.body());
    let page = preview.body();
    assert!(
        page.contains("Preview of the import") && page.contains("Read as: List of addresses"),
        "{page}"
    );
    assert!(
        page.contains("Found: 3") && page.contains("To be made: 2") && page.contains("Skipped: 1"),
        "{page}"
    );
    assert!(
        page.contains("only http and https addresses can be watched"),
        "{page}"
    );
    assert!(page.contains("Import 2 watches"), "{page}");
    // Nothing is made by looking.
    assert!(app.store.list_watches().await?.is_empty());

    let id = plan_id(page)?;
    let done = app
        .post_form("/import/apply", &format!("plan={id}"))
        .await?;
    assert_eq!(done.status(), StatusCode::OK, "{}", done.body());
    assert!(done.body().contains("Watches made: 2"), "{}", done.body());
    let watches = app.store.list_watches().await?;
    assert_eq!(watches.len(), 2);
    assert!(
        watches
            .iter()
            .all(|w| w.schedule == vigil_core::Schedule::Interval(1800))
    );
    assert_eq!(app.store.list_tags().await?.len(), 1);

    // The plan is used up: pressing the button again does nothing.
    let again = app
        .post_form("/import/apply", &format!("plan={id}"))
        .await?;
    assert_eq!(again.status(), StatusCode::GONE);
    assert!(
        again.body().contains("That preview has expired"),
        "{}",
        again.body()
    );
    assert_eq!(app.store.list_watches().await?.len(), 2);
    let none = app.post_form("/import/apply", "plan=nope").await?;
    assert_eq!(none.status(), StatusCode::GONE);
    Ok(())
}

#[tokio::test]
async fn a_file_is_read_whatever_its_format() -> TestResult {
    let app = app().await?;
    let opml = br#"<?xml version="1.0"?><opml version="2.0"><body>
        <outline text="Tech"><outline type="rss" text="Blog" xmlUrl="https://blog.example/feed"/></outline></body></opml>"#;
    let preview = send(
        &app,
        "/import",
        multipart(&[("format", ""), ("every", "")], Some(("feeds.opml", opml))),
    )
    .await?;
    assert_eq!(preview.status(), StatusCode::OK, "{}", preview.body());
    assert!(
        preview.body().contains("Read as: OPML") && preview.body().contains("Blog"),
        "{}",
        preview.body()
    );

    let id = plan_id(preview.body())?;
    app.post_form("/import/apply", &format!("plan={id}"))
        .await?;
    let made = app.store.list_watches().await?;
    assert_eq!(made.len(), 1);
    assert_eq!(made[0].processor, vigil_core::Processor::Feed);
    // A second preview of the same file says the address is watched already, and offers nothing.
    let again = send(&app, "/import", multipart(&[], Some(("feeds.opml", opml)))).await?;
    assert!(again.body().contains("exists already"), "{}", again.body());
    assert!(
        again.body().contains("There is nothing to import."),
        "{}",
        again.body()
    );
    Ok(())
}

#[tokio::test]
async fn a_form_that_cannot_work_is_shown_again_with_what_was_typed() -> TestResult {
    let app = app().await?;
    for (fields, file, status, text) in [
        (
            vec![("urls", "  ")],
            None,
            StatusCode::UNPROCESSABLE_ENTITY,
            "Choose a file or paste some addresses.",
        ),
        (
            vec![("urls", "https://a.example/"), ("format", "nope")],
            None,
            StatusCode::UNPROCESSABLE_ENTITY,
            "Choose one of the formats",
        ),
        (
            vec![("urls", "https://a.example/"), ("every", "soon")],
            None,
            StatusCode::UNPROCESSABLE_ENTITY,
            "Not an interval",
        ),
        (
            vec![("urls", "just some words")],
            None,
            StatusCode::UNPROCESSABLE_ENTITY,
            "not a format that can be imported",
        ),
        (
            vec![("format", "opml")],
            Some(("x.opml", &b"<rss/>"[..])),
            StatusCode::UNPROCESSABLE_ENTITY,
            "cannot read the opml file",
        ),
    ] {
        let page = send(&app, "/import", multipart(&fields, file)).await?;
        assert_eq!(page.status(), status, "{}", page.body());
        assert!(page.body().contains(text), "{text} not in {}", page.body());
    }
    let page = send(
        &app,
        "/import",
        multipart(&[("urls", "just some words")], None),
    )
    .await?;
    assert!(
        page.body().contains("just some words"),
        "what was typed is shown again"
    );
    assert!(app.store.list_watches().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_file_over_the_limit_is_refused_and_leaves_nothing_on_disk() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut web = WebConfig::new(Auth::Password, None);
    web.max_upload_bytes = 500;
    web.upload_dir = dir.path().to_owned();
    let app = app_with(web).await?;
    let big = "https://a.example/\n".repeat(100);
    let page = send(
        &app,
        "/import",
        multipart(&[], Some(("list.txt", big.as_bytes()))),
    )
    .await?;
    assert_eq!(
        page.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        page.body()
    );
    assert!(
        page.body().contains("The file is larger than 500 B."),
        "{}",
        page.body()
    );
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
    Ok(())
}

#[tokio::test]
async fn importing_needs_a_session_and_a_request_from_this_site() -> TestResult {
    let app = app().await?;
    let out = app.signed_out();
    let page = out.get("/import").await?;
    assert_eq!(page.status(), StatusCode::SEE_OTHER);
    let foreign = app
        .send(
            Request::post("/import/apply")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::ORIGIN, "http://evil.example")
                .body(Body::from("plan=x"))?,
        )
        .await?;
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    Ok(())
}
