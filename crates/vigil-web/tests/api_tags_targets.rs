//! `/api/v1/tags` and `/api/v1/notifications`.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::*;

const NIL: &str = "00000000-0000-7000-8000-000000000000";

async fn post(
    app: &App,
    token: &str,
    path: &str,
    body: Value,
) -> Result<(StatusCode, Value), Box<dyn std::error::Error>> {
    app.api(Method::POST, path, Some(token), Some(&body)).await
}

#[tokio::test]
async fn a_tag_is_made_read_listed_replaced_and_deleted() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let (_, phone) = post(
        &app,
        &token,
        "/api/v1/notifications",
        json!({"name": "phone", "url": "ntfys://ntfy.example/t"}),
    )
    .await?;
    let phone = phone["id"].as_str().ok_or("no id")?.to_owned();

    // Only a name: the defaults.
    let (status, plain) = post(&app, &token, "/api/v1/tags", json!({"name": "  news  "})).await?;
    assert_eq!(status, StatusCode::CREATED, "{plain}");
    assert_eq!(plain["name"], "news");
    assert_eq!(plain["color"], "gray");
    assert_eq!(plain["default_interval_secs"], Value::Null);
    assert_eq!(
        (plain["targets"].clone(), plain["watches"].clone()),
        (json!([]), json!(0))
    );

    let (status, shops) = post(
        &app,
        &token,
        "/api/v1/tags",
        json!({"name": "Shops", "color": "green", "default_interval_secs": 900, "targets": [phone, phone]}),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "{shops}");
    let id = shops["id"].as_str().ok_or("no id")?.to_owned();
    assert_eq!(shops["targets"], json!([phone]));
    assert_eq!(shops["default_interval_secs"], 900);

    // A watch with the tag is counted, and takes the tag's interval.
    let (_, watch) = post(
        &app,
        &token,
        "/api/v1/watch",
        json!({"url": "https://a.example/", "tags": [id], "paused": true}),
    )
    .await?;
    assert_eq!(watch["schedule"], json!({"interval": 900}));
    let (_, read) = app
        .api(
            Method::GET,
            &format!("/api/v1/tags/{id}"),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(read["watches"], 1);

    let (_, list) = app
        .api(Method::GET, "/api/v1/tags", Some(&token), None)
        .await?;
    let names: Vec<&str> = list["tags"]
        .as_array()
        .ok_or("no list")?
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert_eq!(names, ["news", "Shops"]);

    // Replacing resets what is left out; the tag keeps its watches.
    let (status, replaced) = app
        .api(
            Method::PUT,
            &format!("/api/v1/tags/{id}"),
            Some(&token),
            Some(&json!({"name": "Stores"})),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{replaced}");
    assert_eq!(
        (
            replaced["color"].clone(),
            replaced["default_interval_secs"].clone()
        ),
        (json!("gray"), Value::Null)
    );
    assert_eq!(
        (replaced["targets"].clone(), replaced["watches"].clone()),
        (json!([]), json!(1))
    );

    // Deleting takes it off its watches and leaves them.
    let response = app
        .signed_out()
        .send(
            axum::http::Request::delete(format!("/api/v1/tags/{id}"))
                .header("x-api-key", &token)
                .body(axum::body::Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let (_, kept) = app
        .api(
            Method::GET,
            &format!("/api/v1/watch/{}", watch["id"].as_str().ok_or("no id")?),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(kept["tags"], json!([]));
    let (status, _) = app
        .api(
            Method::GET,
            &format!("/api/v1/tags/{id}"),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn the_tag_list_gives_each_tag_its_own_targets_and_watch_count() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let mut targets = Vec::new();
    for (name, topic) in [("phone", "a"), ("desk", "b")] {
        let (_, target) = post(
            &app,
            &token,
            "/api/v1/notifications",
            json!({"name": name, "url": format!("ntfys://ntfy.example/{topic}")}),
        )
        .await?;
        targets.push(target["id"].as_str().ok_or("no id")?.to_owned());
    }
    let (phone, desk) = (&targets[0], &targets[1]);

    let mut ids = Vec::new();
    for (name, linked) in [
        ("a-none", json!([])),
        ("b-phone", json!([phone])),
        ("c-both", json!([phone, desk])),
    ] {
        let (status, tag) = post(
            &app,
            &token,
            "/api/v1/tags",
            json!({"name": name, "targets": linked}),
        )
        .await?;
        assert_eq!(status, StatusCode::CREATED, "{tag}");
        ids.push(tag["id"].as_str().ok_or("no id")?.to_owned());
    }
    // Two watches on the last tag, one on the middle one.
    for (url, tag) in [
        ("https://a.example/", &ids[2]),
        ("https://b.example/", &ids[2]),
        ("https://c.example/", &ids[1]),
    ] {
        let (status, watch) = post(
            &app,
            &token,
            "/api/v1/watch",
            json!({"url": url, "tags": [tag], "paused": true}),
        )
        .await?;
        assert_eq!(status, StatusCode::CREATED, "{watch}");
    }

    let (_, list) = app
        .api(Method::GET, "/api/v1/tags", Some(&token), None)
        .await?;
    let tags = list["tags"].as_array().ok_or("no list")?;
    assert_eq!(tags.len(), 3);
    let sorted = |value: &Value| -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let mut found: Vec<String> = value
            .as_array()
            .ok_or("no targets")?
            .iter()
            .filter_map(|t| t.as_str().map(str::to_owned))
            .collect();
        found.sort();
        Ok(found)
    };
    let mut both = vec![phone.clone(), desk.clone()];
    both.sort();
    assert_eq!(
        (tags[0]["name"].clone(), tags[0]["watches"].clone()),
        (json!("a-none"), json!(0))
    );
    assert_eq!(tags[0]["targets"], json!([]));
    assert_eq!(
        (tags[1]["name"].clone(), tags[1]["watches"].clone()),
        (json!("b-phone"), json!(1))
    );
    assert_eq!(tags[1]["targets"], json!([phone]));
    assert_eq!(
        (tags[2]["name"].clone(), tags[2]["watches"].clone()),
        (json!("c-both"), json!(2))
    );
    assert_eq!(sorted(&tags[2]["targets"])?, both);
    Ok(())
}

#[tokio::test]
async fn a_tag_request_that_cannot_work_names_the_field() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let long = "x".repeat(51);
    for (body, field) in [
        (json!({"name": ""}), "name"),
        (json!({"name": long}), "name"),
        (json!({"name": "a", "color": "chartreuse"}), "color"),
        (
            json!({"name": "a", "default_interval_secs": 0}),
            "default_interval_secs",
        ),
        (json!({"name": "a", "targets": [NIL]}), "targets"),
    ] {
        let (status, answer) = post(&app, &token, "/api/v1/tags", body.clone()).await?;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body} -> {answer}"
        );
        assert_eq!(answer["error"]["field"], field, "{answer}");
    }
    assert!(app.store.list_tags().await?.is_empty());

    // A name that is taken is a conflict, on a new tag and on a replacement.
    let (_, first) = post(&app, &token, "/api/v1/tags", json!({"name": "one"})).await?;
    let (_, second) = post(&app, &token, "/api/v1/tags", json!({"name": "two"})).await?;
    let (status, answer) = post(&app, &token, "/api/v1/tags", json!({"name": "one"})).await?;
    assert_eq!(
        (status, &answer["error"]["code"]),
        (StatusCode::CONFLICT, &json!("conflict"))
    );
    let (status, _) = app
        .api(
            Method::PUT,
            &format!("/api/v1/tags/{}", second["id"].as_str().ok_or("no id")?),
            Some(&token),
            Some(&json!({"name": "one"})),
        )
        .await?;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = app
        .api(
            Method::PUT,
            &format!("/api/v1/tags/{NIL}"),
            Some(&token),
            Some(&json!({"name": "x"})),
        )
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(first["name"], "one");
    Ok(())
}

#[tokio::test]
async fn a_target_is_written_with_its_secrets_and_read_without_them() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;

    let (status, made) = post(
        &app,
        &token,
        "/api/v1/notifications",
        json!({"name": "phone", "url": "ntfy://ntfy.example/S3CRETTOPIC", "default_template": "{{ watch_name }} changed"}),
    )
    .await?;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["kind"], "ntfy");
    assert_eq!(made["url"], "ntfy://ntfy.example/***");
    assert_eq!(made["sends_secrets_in_cleartext"], true);
    assert_eq!(made["enabled"], true);
    assert_eq!(made["watches"], 0);
    let id = made["id"].as_str().ok_or("no id")?.to_owned();

    // The secret is in the database and in no answer.
    assert_eq!(
        app.store.get_target(id.parse()?).await?.url,
        "ntfy://ntfy.example/S3CRETTOPIC"
    );
    for (method, path) in [
        (Method::GET, "/api/v1/notifications".to_owned()),
        (Method::GET, format!("/api/v1/notifications/{id}")),
    ] {
        let (_, answer) = app.api(method, &path, Some(&token), None).await?;
        assert!(!answer.to_string().contains("S3CRETTOPIC"), "{answer}");
    }

    // Replacing without an address keeps it; with one, changes it; `enabled` only when given.
    let (status, kept) = app
        .api(
            Method::PUT,
            &format!("/api/v1/notifications/{id}"),
            Some(&token),
            Some(&json!({"name": "mobile", "enabled": false})),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{kept}");
    assert_eq!(
        (kept["name"].clone(), kept["enabled"].clone()),
        (json!("mobile"), json!(false))
    );
    assert_eq!(kept["default_template"], Value::Null);
    assert_eq!(
        app.store.get_target(id.parse()?).await?.url,
        "ntfy://ntfy.example/S3CRETTOPIC"
    );
    let (_, still_off) = app
        .api(
            Method::PUT,
            &format!("/api/v1/notifications/{id}"),
            Some(&token),
            Some(&json!({"name": "mobile"})),
        )
        .await?;
    assert_eq!(still_off["enabled"], false);
    let (_, changed) = app
        .api(
            Method::PUT,
            &format!("/api/v1/notifications/{id}"),
            Some(&token),
            Some(&json!({"name": "mobile", "url": "ntfys://ntfy.example/OTHER", "enabled": true})),
        )
        .await?;
    assert_eq!(
        (
            changed["url"].clone(),
            changed["enabled"].clone(),
            changed["sends_secrets_in_cleartext"].clone()
        ),
        (json!("ntfys://ntfy.example/***"), json!(true), json!(false))
    );
    assert_eq!(
        app.store.get_target(id.parse()?).await?.url,
        "ntfys://ntfy.example/OTHER"
    );

    // A watch that uses it is counted.
    post(
        &app,
        &token,
        "/api/v1/watch",
        json!({"url": "https://a.example/", "targets": [id], "paused": true}),
    )
    .await?;
    let (_, counted) = app
        .api(
            Method::GET,
            &format!("/api/v1/notifications/{id}"),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(counted["watches"], 1);

    // Made disabled.
    let (_, off) = post(
        &app,
        &token,
        "/api/v1/notifications",
        json!({"name": "mail", "url": "ntfy://h/t", "enabled": false}),
    )
    .await?;
    assert_eq!(off["enabled"], false);

    let response = app
        .signed_out()
        .send(
            axum::http::Request::delete(format!("/api/v1/notifications/{id}"))
                .header("x-api-key", &token)
                .body(axum::body::Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let (status, _) = app
        .api(
            Method::GET,
            &format!("/api/v1/notifications/{id}"),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn a_target_request_that_cannot_work_never_repeats_the_address() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let long = "x".repeat(101);
    for (body, field) in [
        (json!({"name": ""}), "name"),
        (json!({"name": long, "url": "ntfy://h/t"}), "name"),
        (json!({"name": "a"}), "url"),
        (json!({"name": "a", "url": "  "}), "url"),
        (
            json!({"name": "a", "url": "carrier-pigeon://SECRETPART/x"}),
            "url",
        ),
        (
            json!({"name": "a", "url": "ntfy://h/t", "default_template": "{{ nope }"}),
            "default_template",
        ),
    ] {
        let (status, answer) = post(&app, &token, "/api/v1/notifications", body.clone()).await?;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body} -> {answer}"
        );
        assert_eq!(answer["error"]["field"], field, "{answer}");
        assert!(!answer.to_string().contains("SECRETPART"), "{answer}");
    }
    let (_, answer) = post(
        &app,
        &token,
        "/api/v1/notifications",
        json!({"name": "a", "url": "carrier-pigeon://x/y"}),
    )
    .await?;
    assert!(
        answer["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("not supported yet")),
        "{answer}"
    );
    assert!(app.store.list_targets().await?.is_empty());

    post(
        &app,
        &token,
        "/api/v1/notifications",
        json!({"name": "a", "url": "ntfy://h/t"}),
    )
    .await?;
    let (status, answer) = post(
        &app,
        &token,
        "/api/v1/notifications",
        json!({"name": "a", "url": "ntfy://h/u"}),
    )
    .await?;
    assert_eq!(
        (status, &answer["error"]["code"]),
        (StatusCode::CONFLICT, &json!("conflict"))
    );
    Ok(())
}

#[tokio::test]
async fn a_test_notification_says_whether_it_arrived_and_hides_the_topic() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let address = server.address();
    let url = format!("ntfy://{}:{}/S3CRETTOPIC", address.ip(), address.port());
    let (_, made) = post(
        &app,
        &token,
        "/api/v1/notifications",
        json!({"name": "phone", "url": url, "enabled": false}),
    )
    .await?;
    let path = format!(
        "/api/v1/notifications/{}/test",
        made["id"].as_str().ok_or("no id")?
    );

    // A disabled target can be tested. First the server accepts, then it refuses.
    let (status, ok) = app.api(Method::POST, &path, Some(&token), None).await?;
    assert_eq!(status, StatusCode::OK, "{ok}");
    assert_eq!(
        (ok["ok"].clone(), ok["error"].clone()),
        (json!(true), Value::Null)
    );
    let (status, failed) = app.api(Method::POST, &path, Some(&token), None).await?;
    assert_eq!(status, StatusCode::OK, "{failed}");
    assert_eq!(failed["ok"], false);
    assert!(failed["error"].is_string());
    assert!(!failed.to_string().contains("S3CRETTOPIC"), "{failed}");

    let (status, _) = app
        .api(
            Method::POST,
            &format!("/api/v1/notifications/{NIL}/test"),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn tags_and_targets_need_a_token() -> TestResult {
    let app = app().await?;
    for (method, path) in [
        (Method::GET, "/api/v1/tags".to_owned()),
        (Method::POST, "/api/v1/tags".to_owned()),
        (Method::GET, format!("/api/v1/tags/{NIL}")),
        (Method::PUT, format!("/api/v1/tags/{NIL}")),
        (Method::DELETE, format!("/api/v1/tags/{NIL}")),
        (Method::GET, "/api/v1/notifications".to_owned()),
        (Method::POST, "/api/v1/notifications".to_owned()),
        (Method::GET, format!("/api/v1/notifications/{NIL}")),
        (Method::PUT, format!("/api/v1/notifications/{NIL}")),
        (Method::DELETE, format!("/api/v1/notifications/{NIL}")),
        (Method::POST, format!("/api/v1/notifications/{NIL}/test")),
    ] {
        let (status, _) = app
            .api(method.clone(), &path, None, Some(&json!({"name": "x"})))
            .await?;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
    Ok(())
}
