//! `POST /api/v1/import`.

mod common;

use std::io::Write;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use vigil_web::{Auth, WebConfig};

use common::*;

const LIST: &str =
    "https://a.example/pricing news, shops\nhttps://b.example/blog\nftp://nope.example/x\n";

async fn post(
    app: &App,
    token: &str,
    query: &str,
    body: Vec<u8>,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    let response = app
        .signed_out()
        .send(
            Request::post(format!("/api/v1/import{query}"))
                .header("x-api-key", token)
                .header("content-type", "application/octet-stream")
                .body(Body::from(body))?,
        )
        .await?;
    let status = response.status();
    let json = serde_json::from_str(response.body())
        .map_err(|e| format!("not JSON ({e}): {}", response.body()))?;
    Ok((status, json))
}

fn zip_of(files: &[(&str, &str)]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, text) in files {
        writer.start_file(*name, zip::write::SimpleFileOptions::default())?;
        writer.write_all(text.as_bytes())?;
    }
    Ok(writer.finish()?.into_inner())
}

#[tokio::test]
async fn a_dry_run_reports_and_changes_nothing() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let (status, answer) = post(&app, &token, "?dry_run=true", LIST.as_bytes().to_vec()).await?;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["dry_run"], true);
    assert_eq!(answer["made"], Value::Null);
    assert_eq!(answer["report"]["format"], "url-list");
    assert_eq!(answer["report"]["totals"]["created"], 2);
    assert_eq!(answer["report"]["totals"]["skipped"], 1);
    assert_eq!(answer["report"]["totals"]["tags_created"], 2);
    assert!(app.store.list_watches().await?.is_empty());
    assert!(app.store.list_tags().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn an_import_makes_the_watches_and_a_second_one_skips_them() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let (status, answer) = post(&app, &token, "?every=15m", LIST.as_bytes().to_vec()).await?;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["dry_run"], false);
    assert_eq!(answer["made"]["watches"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        (
            answer["made"]["tags"].clone(),
            answer["made"]["targets"].clone()
        ),
        (json!(2), json!(0))
    );

    let watches = app.store.list_watches().await?;
    assert_eq!(watches.len(), 2);
    assert!(
        watches
            .iter()
            .all(|w| w.schedule == vigil_core::Schedule::Interval(900))
    );
    let ids: Vec<String> = watches.iter().map(|w| w.id.to_string()).collect();
    for made in answer["made"]["watches"].as_array().ok_or("no list")? {
        assert!(ids.contains(&made.as_str().unwrap_or_default().to_owned()));
    }
    // The watches are there for the rest of the API too.
    let (_, listed) = app
        .api(axum::http::Method::GET, "/api/v1/watch", Some(&token), None)
        .await?;
    assert_eq!(listed["watches"].as_array().map(Vec::len), Some(2));

    let (_, again) = post(&app, &token, "", LIST.as_bytes().to_vec()).await?;
    assert_eq!(again["made"]["watches"], json!([]));
    assert_eq!(again["report"]["totals"]["skipped"], 3);
    assert_eq!(app.store.list_watches().await?.len(), 2);
    let (_, more) = post(
        &app,
        &token,
        "?allow_duplicates=true",
        LIST.as_bytes().to_vec(),
    )
    .await?;
    assert_eq!(more["made"]["watches"].as_array().map(Vec::len), Some(2));
    assert_eq!(app.store.list_watches().await?.len(), 4);
    Ok(())
}

#[tokio::test]
async fn a_zip_backup_is_read_from_the_upload() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let id = "11111111-2222-4333-8444-555555555555";
    let watch = json!({"uuid": id, "url": "https://shop.example/", "title": "Shop", "include_filters": [".price"],
                       "time_between_check_use_default": false, "time_between_check": {"minutes": 10}});
    let zip = zip_of(&[
        (&format!("{id}/watch.json"), &watch.to_string()),
        (&format!("{id}/history.txt"), "1,abc.txt"),
        ("abc.txt", "not read"),
    ])?;
    let (status, answer) = post(&app, &token, "", zip).await?;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["report"]["format"], "backup");
    let made = app.store.list_watches().await?;
    assert_eq!(made.len(), 1);
    assert_eq!(made[0].name, "Shop");
    assert_eq!(made[0].schedule, vigil_core::Schedule::Interval(600));
    assert_eq!(made[0].pipeline.len(), 1);
    Ok(())
}

#[tokio::test]
async fn what_cannot_be_read_is_a_json_error_that_names_the_problem() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    for (query, body, status, code, field) in [
        (
            "?format=nope",
            LIST,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            Some("format"),
        ),
        (
            "?every=soon",
            LIST,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            Some("every"),
        ),
        (
            "",
            "just some words\n",
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown_format",
            None,
        ),
        (
            "?format=opml",
            "<rss></rss>",
            StatusCode::UNPROCESSABLE_ENTITY,
            "unreadable_file",
            None,
        ),
        (
            "?format=backup",
            "{\"hello\": 1}",
            StatusCode::UNPROCESSABLE_ENTITY,
            "unreadable_file",
            None,
        ),
    ] {
        let (got, answer) = post(&app, &token, query, body.as_bytes().to_vec()).await?;
        assert_eq!(got, status, "{query} -> {answer}");
        assert_eq!(answer["error"]["code"], code, "{answer}");
        assert_eq!(answer["error"]["field"].as_str(), field, "{answer}");
    }
    let (status, _) = post(&app, &token, "", Vec::new()).await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(app.store.list_watches().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_file_over_the_limit_is_refused_and_nothing_is_left_on_disk() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut web = WebConfig::new(Auth::Password, None);
    web.max_upload_bytes = 1000;
    web.upload_dir = dir.path().to_owned();
    let app = app_with(web).await?;
    let token = app.api_token("t").await?;

    // One that says how large it is, and one that does not (it is counted as it arrives).
    let big = "https://a.example/\n".repeat(100);
    let (status, answer) = post(&app, &token, "", big.clone().into_bytes()).await?;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{answer}");
    assert_eq!(answer["error"]["code"], "too_large");
    let pieces = chunks(&big);
    let response = app
        .signed_out()
        .send(
            Request::post("/api/v1/import")
                .header("x-api-key", &token)
                .body(Body::from_stream(pieces))?,
        )
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        response.body()
    );
    assert_eq!(
        std::fs::read_dir(dir.path())?.count(),
        0,
        "an upload was left behind"
    );

    // One that fits is read, and its file is gone afterwards too.
    let (status, _) = post(&app, &token, "", b"https://a.example/\n".to_vec()).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
    Ok(())
}

/// A body with no declared length, in pieces.
fn chunks(text: &str) -> impl futures_util::Stream<Item = Result<Vec<u8>, std::io::Error>> + use<> {
    let parts: Vec<Result<Vec<u8>, std::io::Error>> = text
        .as_bytes()
        .chunks(200)
        .map(|c| Ok(c.to_vec()))
        .collect();
    futures_util::stream::iter(parts)
}

#[tokio::test]
async fn importing_needs_a_token_and_is_described() -> TestResult {
    let app = app().await?;
    let response = app
        .signed_out()
        .send(Request::post("/api/v1/import").body(Body::from(LIST))?)
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let (_, spec) = app
        .api(
            axum::http::Method::GET,
            "/api/docs/openapi.json",
            None,
            None,
        )
        .await?;
    assert!(
        spec["paths"]["/api/v1/import"]["post"].is_object(),
        "{spec}"
    );
    assert!(spec["components"]["schemas"]["Report"].is_object());
    Ok(())
}
