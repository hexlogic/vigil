//! `/api/v1/watch/{id}/history` and `/diff`.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::json;
use vigil_core::Watch;
use vigil_store::{CheckSuccess, Content, RecordOutcome, SnapshotId, now_ms};

use common::*;

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

struct Versions {
    watch: Watch,
    first: SnapshotId,
    second: SnapshotId,
    third: SnapshotId,
}

async fn three_versions(app: &App) -> Result<Versions, Box<dyn std::error::Error>> {
    let watch = app.add_watch("shop", "https://shop.example/").await?;
    let first = record(app, &watch, "Widget\nPrice: $10\nStock: yes\n")
        .await?
        .ok_or("no baseline")?;
    let second = record(app, &watch, "Widget\nPrice: $12\nStock: yes\n")
        .await?
        .ok_or("no change")?;
    let third = record(app, &watch, "Widget\nPrice: $12\nStock: no\nNew arrival\n")
        .await?
        .ok_or("no change")?;
    Ok(Versions {
        watch,
        first,
        second,
        third,
    })
}

#[tokio::test]
async fn the_history_lists_the_versions_newest_first() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let v = three_versions(&app).await?;
    let path = format!("/api/v1/watch/{}/history", v.watch.id);

    let (status, history) = app.api(Method::GET, &path, Some(&token), None).await?;
    assert_eq!(status, StatusCode::OK, "{history}");
    let ids: Vec<&str> = history["snapshots"]
        .as_array()
        .ok_or("no list")?
        .iter()
        .filter_map(|s| s["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        [
            v.third.to_string(),
            v.second.to_string(),
            v.first.to_string()
        ]
    );
    let newest = &history["snapshots"][0];
    assert_eq!(newest["http_status"], 200);
    assert_eq!(newest["content_type"], "text/html");
    assert_eq!(newest["raw_size"], 2048);
    assert_eq!(newest["hash"].as_str().map(str::len), Some(64));
    assert!(
        newest["taken_at"]
            .as_str()
            .is_some_and(|t| t.ends_with('Z'))
    );
    assert!(newest.get("content").is_none(), "the list holds no text");

    let (_, limited) = app
        .api(Method::GET, &format!("{path}?limit=2"), Some(&token), None)
        .await?;
    assert_eq!(limited["snapshots"].as_array().map(Vec::len), Some(2));
    // 0 is read as 1, and a huge number as the most there is.
    let (_, zero) = app
        .api(Method::GET, &format!("{path}?limit=0"), Some(&token), None)
        .await?;
    assert_eq!(zero["snapshots"].as_array().map(Vec::len), Some(1));
    let (_, huge) = app
        .api(
            Method::GET,
            &format!("{path}?limit=99999"),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(huge["snapshots"].as_array().map(Vec::len), Some(3));
    Ok(())
}

#[tokio::test]
async fn one_version_comes_with_its_text_and_only_for_its_own_watch() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let v = three_versions(&app).await?;
    let other = app.add_watch("other", "https://other.example/").await?;
    let theirs = record(&app, &other, "secret\n")
        .await?
        .ok_or("no baseline")?;

    let (status, one) = app
        .api(
            Method::GET,
            &format!("/api/v1/watch/{}/history/{}", v.watch.id, v.second),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(one["id"], v.second.to_string());
    assert_eq!(one["content"], "Widget\nPrice: $12\nStock: yes\n");

    // Another watch's version, a made-up one and a malformed one are all just not found.
    for id in [
        theirs.to_string(),
        "00000000-0000-7000-8000-000000000000".to_owned(),
        "nope".to_owned(),
    ] {
        let (status, answer) = app
            .api(
                Method::GET,
                &format!("/api/v1/watch/{}/history/{id}", v.watch.id),
                Some(&token),
                None,
            )
            .await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{id}");
        assert_eq!(answer["error"]["code"], "not_found");
        assert!(!answer.to_string().contains("secret"));
    }
    let (status, _) = app
        .api(
            Method::GET,
            "/api/v1/watch/00000000-0000-7000-8000-000000000000/history",
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn the_diff_defaults_to_the_newest_two_and_can_name_any_two() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let v = three_versions(&app).await?;
    let path = format!("/api/v1/watch/{}/diff", v.watch.id);

    let (status, diff) = app.api(Method::GET, &path, Some(&token), None).await?;
    assert_eq!(status, StatusCode::OK, "{diff}");
    assert_eq!(diff["from"]["id"], v.second.to_string());
    assert_eq!(diff["to"]["id"], v.third.to_string());
    assert_eq!(diff["added"], json!(["Stock: no", "New arrival"]));
    assert_eq!(diff["removed"], json!(["Stock: yes"]));
    assert_eq!(
        (diff["added_count"].clone(), diff["removed_count"].clone()),
        (json!(2), json!(1))
    );
    assert_eq!(diff["truncated"], false);
    let unified = diff["unified"].as_str().ok_or("no unified diff")?;
    assert!(unified.starts_with("@@ -"), "{unified}");
    assert!(
        unified.contains("-Stock: yes\n") && unified.contains("+New arrival\n"),
        "{unified}"
    );

    // Any two, in either order of age: from is the older one the caller names.
    let (_, wide) = app
        .api(
            Method::GET,
            &format!("{path}?from={}&to={}", v.first, v.third),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(wide["removed"], json!(["Price: $10", "Stock: yes"]));
    // Only `to`: the version before it.
    let (_, to_only) = app
        .api(
            Method::GET,
            &format!("{path}?to={}", v.second),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(to_only["from"]["id"], v.first.to_string());
    // The same version twice: nothing differs.
    let (_, same) = app
        .api(
            Method::GET,
            &format!("{path}?from={}&to={}", v.third, v.third),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(
        (same["added_count"].clone(), same["unified"].clone()),
        (json!(0), json!(""))
    );
    Ok(())
}

#[tokio::test]
async fn a_diff_needs_two_versions_that_belong_to_the_watch() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let watch = app.add_watch("shop", "https://shop.example/").await?;
    let path = format!("/api/v1/watch/{}/diff", watch.id);

    // No versions, then one: nothing to compare with.
    for _ in 0..2 {
        let (status, answer) = app.api(Method::GET, &path, Some(&token), None).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
        assert_eq!(answer["error"]["code"], "nothing_to_compare");
        record(&app, &watch, "one\n").await?;
    }

    let other = app.add_watch("other", "https://other.example/").await?;
    let theirs = record(&app, &other, "secret\n")
        .await?
        .ok_or("no baseline")?;
    let (status, answer) = app
        .api(
            Method::GET,
            &format!("{path}?to={theirs}"),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!answer.to_string().contains("secret"));
    let (status, _) = app
        .api(
            Method::GET,
            &format!("{path}?to=not-an-id"),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn a_huge_difference_is_cut_but_counted() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let watch = app.add_watch("big", "https://big.example/").await?;
    record(&app, &watch, "start\n").await?;
    let lines: String = (0..6000).map(|n| format!("line {n}\n")).collect();
    record(&app, &watch, &lines).await?;
    let (status, diff) = app
        .api(
            Method::GET,
            &format!("/api/v1/watch/{}/diff", watch.id),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(diff["added_count"], 6000);
    assert_eq!(diff["added"].as_array().map(Vec::len), Some(5000));
    assert_eq!(diff["truncated"], true);
    Ok(())
}

#[tokio::test]
async fn history_needs_a_token() -> TestResult {
    let app = app().await?;
    let v = three_versions(&app).await?;
    for path in [
        format!("/api/v1/watch/{}/history", v.watch.id),
        format!("/api/v1/watch/{}/history/{}", v.watch.id, v.first),
        format!("/api/v1/watch/{}/diff", v.watch.id),
    ] {
        let (status, _) = app.api(Method::GET, &path, None, None).await?;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
    Ok(())
}

/// Make a watch that keeps `count` versions and record that many, one per second of an
/// invented clock, so the newest is `count - 1`. Returns the ids, oldest first.
async fn long_history(
    app: &App,
    count: usize,
) -> Result<(Watch, Vec<SnapshotId>), Box<dyn std::error::Error>> {
    let watch = app.add_watch("long", "https://long.example/").await?;
    let mut settings = vigil_store::NewWatch::from(&watch);
    settings.snapshot_retention = u32::try_from(count)? + 10;
    app.store.update_watch(watch.id, &settings).await?;
    let mut ids = Vec::new();
    for n in 0..count {
        let outcome = app
            .store
            .record_success(
                watch.id,
                &CheckSuccess {
                    at: 1_000 + i64::try_from(n)? * 1_000,
                    duration_ms: 1,
                    http_status: Some(200),
                    content_type: Some("text/plain".into()),
                    etag: None,
                    last_modified: None,
                    content: Some(Content {
                        text: format!("version {n}\n"),
                        raw_size: 10,
                    }),
                    settings_version: None,
                },
            )
            .await?;
        match outcome {
            RecordOutcome::Baseline { snapshot } => ids.push(snapshot),
            RecordOutcome::Changed { current, .. } => ids.push(current),
            RecordOutcome::Unchanged => return Err("no new version".into()),
        }
    }
    Ok((watch, ids))
}

#[tokio::test]
async fn a_diff_finds_the_version_before_one_that_is_far_back_in_a_long_history() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let (watch, ids) = long_history(&app, 520).await?;
    let path = format!("/api/v1/watch/{}/diff", watch.id);

    // The third oldest is 518 versions from the newest, beyond what was once searched.
    let (status, diff) = app
        .api(
            Method::GET,
            &format!("{path}?to={}", ids[2]),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{diff}");
    assert_eq!(diff["from"]["id"], ids[1].to_string());
    assert_eq!(diff["to"]["id"], ids[2].to_string());
    assert_eq!(diff["added"], json!(["version 2"]));

    // The oldest has nothing before it.
    let (status, answer) = app
        .api(
            Method::GET,
            &format!("{path}?to={}", ids[0]),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["error"]["code"], "nothing_to_compare");
    Ok(())
}

#[tokio::test]
async fn a_diff_from_the_newest_version_alone_has_nothing_to_compare_with() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let v = three_versions(&app).await?;
    let path = format!("/api/v1/watch/{}/diff", v.watch.id);

    let (status, answer) = app
        .api(
            Method::GET,
            &format!("{path}?from={}", v.third),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["error"]["code"], "nothing_to_compare");

    // An older `from` alone is compared with the newest.
    let (status, diff) = app
        .api(
            Method::GET,
            &format!("{path}?from={}", v.first),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{diff}");
    assert_eq!(diff["to"]["id"], v.third.to_string());
    Ok(())
}
