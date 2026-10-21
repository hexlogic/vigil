//! `/api/v1/watch`: adding, reading, replacing, deleting and checking watches.

mod common;

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use serde_json::{Value, json};
use vigil_store::{NewTag, NewTarget};
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

async fn tag(
    app: &App,
    name: &str,
    default_secs: Option<u64>,
) -> Result<String, Box<dyn std::error::Error>> {
    let tag = app
        .store
        .create_tag(&NewTag {
            name: name.into(),
            color: "blue".into(),
            default_interval_secs: default_secs,
        })
        .await?;
    Ok(tag.id.to_string())
}

async fn target(app: &App, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let target = app
        .store
        .create_target(&NewTarget {
            name: name.into(),
            url: format!("ntfy://ntfy.example/{name}"),
            default_template: None,
        })
        .await?;
    Ok(target.id.to_string())
}

async fn wait_until_checked(
    app: &App,
    token: &str,
    id: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (_, watch) = app
            .api(
                Method::GET,
                &format!("/api/v1/watch/{id}"),
                Some(token),
                None,
            )
            .await?;
        if !watch["state"]["last_checked_at"].is_null() {
            return Ok(watch);
        }
        if Instant::now() > deadline {
            return Err("the first check never ran".into());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn a_watch_with_only_an_address_gets_the_defaults_and_is_checked_at_once() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let server = page("<p>hello</p>").await;
    let url = server.uri();

    let response = app
        .signed_out()
        .send(
            Request::post("/api/v1/watch")
                .header("x-api-key", &token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"url": url}).to_string()))?,
        )
        .await?;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "{}",
        response.body()
    );
    let watch: Value = serde_json::from_str(response.body())?;
    let id = watch["id"].as_str().ok_or("no id")?.to_owned();
    assert_eq!(
        header_of(&response, "location"),
        format!("/api/v1/watch/{id}")
    );

    assert_eq!(watch["name"], url);
    assert_eq!(watch["url"], url);
    assert_eq!(watch["paused"], false);
    assert_eq!(watch["schedule"], json!({"interval": 3600}));
    assert_eq!(watch["processor"], "text");
    assert_eq!(watch["pipeline"], json!([]));
    assert_eq!(
        watch["conditions"],
        json!({"trigger_text": [], "block_text": [], "notify_on": {"added": true, "removed": true, "changed": true}})
    );
    assert_eq!(watch["snapshot_retention"], 100);
    assert_eq!(watch["tags"], json!([]));
    assert_eq!(watch["targets"], json!([]));
    assert_eq!(
        watch["fetch"],
        json!({"timeout_secs": null, "user_agent": null, "headers": {}})
    );
    assert!(
        watch["created_at"]
            .as_str()
            .is_some_and(|t| t.ends_with('Z'))
    );

    let checked = wait_until_checked(&app, &token, &id).await?;
    assert_eq!(checked["state"]["consecutive_errors"], 0);
    assert!(checked["state"]["last_changed_at"].is_string());
    assert_eq!(checked["state"]["last_error"], Value::Null);
    Ok(())
}

#[tokio::test]
async fn everything_a_watch_has_can_be_set_and_is_read_back() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let server = page("<span id=price>$10</span>").await;
    let shops = tag(&app, "shops", Some(900)).await?;
    let phone = target(&app, "phone").await?;

    let body = json!({
        "url": server.uri(),
        "name": "  Widget  ",
        "schedule": {"interval": 300},
        "jitter_secs": 20,
        "fetch": {"timeout_secs": 15, "user_agent": "me", "headers": {"X-Test": "1"},
                  "method": "POST", "body": "q=1"},
        "processor": "text",
        "pipeline": [{"type": "css", "selector": "#price"}, {"type": "trim"}],
        "conditions": {"trigger_text": ["$"], "scope": "text", "notify_on": {"removed": false}},
        "snapshot_retention": 7,
        "tags": [shops, shops],
        "targets": [phone],
        "notification_template": "{{ watch_name }} changed"
    });
    let (status, made) = app
        .api(Method::POST, "/api/v1/watch", Some(&token), Some(&body))
        .await?;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let id = made["id"].as_str().ok_or("no id")?;

    let (status, read) = app
        .api(
            Method::GET,
            &format!("/api/v1/watch/{id}"),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["name"], "Widget");
    assert_eq!(read["schedule"], json!({"interval": 300}));
    assert_eq!(read["jitter_secs"], 20);
    assert_eq!(read["fetch"]["method"], "POST");
    assert_eq!(read["fetch"]["body"], "q=1");
    assert_eq!(read["fetch"]["headers"], json!({"X-Test": "1"}));
    assert_eq!(
        read["pipeline"][0],
        json!({"type": "css", "selector": "#price", "mode": "include"})
    );
    assert_eq!(read["conditions"]["scope"], "text");
    assert_eq!(read["conditions"]["notify_on"]["removed"], false);
    assert_eq!(read["snapshot_retention"], 7);
    assert_eq!(read["tags"], json!([shops]));
    assert_eq!(read["targets"], json!([phone]));
    assert_eq!(read["notification_template"], "{{ watch_name }} changed");

    // The same facts through the store, which is what the scheduler reads.
    let stored = app.store.get_watch(id.parse()?).await?;
    assert_eq!(stored.fetch.method, vigil_core::Method::Post);
    assert_eq!(stored.schedule, vigil_core::Schedule::Interval(300));
    Ok(())
}

#[tokio::test]
async fn the_interval_defaults_to_the_shortest_default_of_the_tags() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let slow = tag(&app, "slow", Some(7200)).await?;
    let fast = tag(&app, "fast", Some(600)).await?;
    let body = json!({"url": "https://a.example/", "tags": [slow, fast], "paused": true});
    let (status, made) = app
        .api(Method::POST, "/api/v1/watch", Some(&token), Some(&body))
        .await?;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["schedule"], json!({"interval": 600}));
    assert_eq!(made["paused"], true);
    Ok(())
}

#[tokio::test]
async fn the_list_is_by_name_and_can_be_narrowed_to_a_tag() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let news = tag(&app, "news", None).await?;
    for (name, tags) in [
        ("beta", vec![news.clone()]),
        ("Alpha", vec![]),
        ("gamma", vec![news.clone()]),
    ] {
        let body = json!({"url": "https://a.example/", "name": name, "tags": tags, "paused": true});
        let (status, _) = app
            .api(Method::POST, "/api/v1/watch", Some(&token), Some(&body))
            .await?;
        assert_eq!(status, StatusCode::CREATED);
    }
    let names = |list: &Value| -> Vec<String> {
        list["watches"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|w| w["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (_, all) = app
        .api(Method::GET, "/api/v1/watch", Some(&token), None)
        .await?;
    assert_eq!(names(&all), ["Alpha", "beta", "gamma"]);
    let (_, some) = app
        .api(
            Method::GET,
            &format!("/api/v1/watch?tag={news}"),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(names(&some), ["beta", "gamma"]);
    assert_eq!(some["watches"][0]["tags"], json!([news]));
    let (_, none) = app
        .api(
            Method::GET,
            "/api/v1/watch?tag=00000000-0000-7000-8000-000000000000",
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(names(&none), Vec::<String>::new());
    // Not an id at all.
    let (status, body) = app
        .api(Method::GET, "/api/v1/watch?tag=nope", Some(&token), None)
        .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "bad_request");
    Ok(())
}

#[tokio::test]
async fn a_request_that_cannot_work_is_refused_with_the_field_it_is_about() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let ghost = "00000000-0000-7000-8000-000000000000";
    let cases = [
        (json!({"url": ""}), "url"),
        (json!({"url": "file:///etc/passwd"}), "url"),
        (json!({"url": "not a url"}), "url"),
        (
            json!({"url": "https://a.example/", "tags": [ghost]}),
            "tags",
        ),
        (
            json!({"url": "https://a.example/", "targets": [ghost]}),
            "targets",
        ),
        (
            json!({"url": "https://a.example/", "schedule": {"interval": 0}}),
            "schedule",
        ),
        (
            json!({"url": "https://a.example/", "schedule": {"interval": 99999999999u64}}),
            "schedule",
        ),
        (
            json!({"url": "https://a.example/", "snapshot_retention": 0}),
            "snapshot_retention",
        ),
        (
            json!({"url": "https://a.example/", "fetch": {"timeout_secs": 0}}),
            "fetch.timeout_secs",
        ),
        (
            json!({"url": "https://a.example/", "fetch": {"headers": {"X": "a\nb"}}}),
            "fetch.headers",
        ),
        (
            json!({"url": "https://a.example/", "fetch": {"body": "x"}}),
            "fetch.body",
        ),
        (
            json!({"url": "https://a.example/", "pipeline": [{"type": "css", "selector": "a["}]}),
            "pipeline",
        ),
        (
            json!({"url": "https://a.example/", "pipeline": [{"type": "jq", "program": ".a"}]}),
            "pipeline",
        ),
        (
            json!({"url": "https://a.example/", "conditions": {"trigger_text": ["/(/"]}}),
            "conditions",
        ),
        (
            json!({"url": "https://a.example/", "notification_template": "{{ nope }"}),
            "notification_template",
        ),
    ];
    for (body, field) in cases {
        let (status, answer) = app
            .api(Method::POST, "/api/v1/watch", Some(&token), Some(&body))
            .await?;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body} -> {answer}"
        );
        assert_eq!(answer["error"]["code"], "invalid", "{body}");
        assert_eq!(answer["error"]["field"], field, "{body} -> {answer}");
    }
    // A pipeline error says which step.
    let body = json!({"url": "https://a.example/", "pipeline": [{"type": "trim"}, {"type": "css", "selector": "a["}]});
    let (_, answer) = app
        .api(Method::POST, "/api/v1/watch", Some(&token), Some(&body))
        .await?;
    assert!(
        answer["error"]["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("step 2 (css)")),
        "{answer}"
    );
    // Nothing was made by any of them.
    assert!(app.store.list_watches().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_body_that_cannot_be_read_is_a_json_error_too() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let send = |content_type: Option<&str>, body: String| {
        let mut request = Request::post("/api/v1/watch").header("x-api-key", &token);
        if let Some(content_type) = content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        request.body(Body::from(body))
    };
    let app = app.signed_out();
    let cases = [
        (
            Some("application/json"),
            "{not json".to_owned(),
            StatusCode::BAD_REQUEST,
            "bad_request",
        ),
        (
            Some("application/json"),
            r#"{"url": 5}"#.to_owned(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
        ),
        (
            Some("application/json"),
            r#"{"url": "https://a.example/", "nope": 1}"#.to_owned(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
        ),
        (
            Some("application/json"),
            r#"{"url": "https://a.example/", "pipeline": [{"type": "magic"}]}"#.to_owned(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
        ),
        (
            Some("text/plain"),
            r#"{"url": "https://a.example/"}"#.to_owned(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
        ),
        (
            None,
            r#"{"url": "https://a.example/"}"#.to_owned(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
        ),
        (
            Some("application/json"),
            "x".repeat(1024 * 1024 + 1),
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
        ),
    ];
    for (content_type, body, status, code) in cases {
        let response = app.send(send(content_type, body.clone())?).await?;
        assert_eq!(
            response.status(),
            status,
            "{content_type:?} {:.60}",
            response.body()
        );
        let answer: Value = serde_json::from_str(response.body())?;
        assert_eq!(answer["error"]["code"], code, "{answer}");
    }
    assert!(app.store.list_watches().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn replacing_a_watch_replaces_all_of_it_and_resets_nothing_it_did_not_change() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let tag_id = tag(&app, "shops", None).await?;
    let phone = target(&app, "phone").await?;
    let body = json!({
        "url": "https://a.example/", "name": "one", "tags": [tag_id], "targets": [phone],
        "notification_template": "x", "paused": true,
        "pipeline": [{"type": "trim"}], "jitter_secs": 5,
    });
    let (_, made) = app
        .api(Method::POST, "/api/v1/watch", Some(&token), Some(&body))
        .await?;
    let id = made["id"].as_str().ok_or("no id")?.to_owned();
    let path = format!("/api/v1/watch/{id}");
    let uuid = id.parse()?;

    // The same settings again: the stored settings are not rewritten.
    let before = app.store.get_watch(uuid).await?.updated_at;
    let (status, same) = app
        .api(Method::PUT, &path, Some(&token), Some(&body))
        .await?;
    assert_eq!(status, StatusCode::OK, "{same}");
    assert_eq!(app.store.get_watch(uuid).await?.updated_at, before);

    // Only an address: everything else goes back to its default, links and pause included.
    let (status, replaced) = app
        .api(
            Method::PUT,
            &path,
            Some(&token),
            Some(&json!({"url": "https://b.example/"})),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{replaced}");
    assert_eq!(replaced["id"], id);
    assert_eq!(replaced["url"], "https://b.example/");
    assert_eq!(replaced["name"], "https://b.example/");
    assert_eq!(replaced["paused"], false);
    assert_eq!(replaced["pipeline"], json!([]));
    assert_eq!(replaced["jitter_secs"], 0);
    assert_eq!(replaced["tags"], json!([]));
    assert_eq!(replaced["targets"], json!([]));
    assert_eq!(replaced["notification_template"], Value::Null);
    assert!(app.store.get_watch(uuid).await?.updated_at > before);

    // A bad replacement changes nothing; an unknown watch is not found.
    let (status, _) = app
        .api(
            Method::PUT,
            &path,
            Some(&token),
            Some(&json!({"url": "file:///x"})),
        )
        .await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(app.store.get_watch(uuid).await?.url, "https://b.example/");
    let ghost = "/api/v1/watch/00000000-0000-7000-8000-000000000000";
    let (status, answer) = app
        .api(
            Method::PUT,
            ghost,
            Some(&token),
            Some(&json!({"url": "https://a.example/"})),
        )
        .await?;
    assert_eq!(
        (status, &answer["error"]["code"]),
        (StatusCode::NOT_FOUND, &json!("not_found"))
    );
    Ok(())
}

#[tokio::test]
async fn a_watch_can_be_deleted_once() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let (_, made) = app
        .api(
            Method::POST,
            "/api/v1/watch",
            Some(&token),
            Some(&json!({"url": "https://a.example/", "paused": true})),
        )
        .await?;
    let path = format!("/api/v1/watch/{}", made["id"].as_str().ok_or("no id")?);
    let response = app
        .signed_out()
        .send(
            Request::delete(&path)
                .header("x-api-key", &token)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(response.body(), "");
    for method in [Method::GET, Method::DELETE] {
        let (status, _) = app.api(method, &path, Some(&token), None).await?;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, _) = app
        .api(Method::GET, "/api/v1/watch/not-an-id", Some(&token), None)
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn a_check_can_be_asked_for_and_waited_for() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let server = page("<p>v1</p>").await;
    // Paused: it is not checked on a schedule, but can be checked on request.
    let (_, made) = app
        .api(
            Method::POST,
            "/api/v1/watch",
            Some(&token),
            Some(&json!({"url": server.uri(), "paused": true})),
        )
        .await?;
    let id = made["id"].as_str().ok_or("no id")?.to_owned();
    assert_eq!(made["state"]["last_checked_at"], Value::Null);

    let (status, waited) = app
        .api(
            Method::POST,
            &format!("/api/v1/watch/{id}/check?wait=true"),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{waited}");
    assert_eq!(waited["queued"], false);
    assert_eq!(waited["outcome"], "done");
    assert!(
        waited["watch"]["state"]["last_checked_at"].is_string(),
        "{waited}"
    );
    assert_eq!(waited["watch"]["paused"], true);

    let (status, queued) = app
        .api(
            Method::POST,
            &format!("/api/v1/watch/{id}/check"),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::ACCEPTED, "{queued}");
    assert_eq!(queued["queued"], true);

    let (status, _) = app
        .api(
            Method::POST,
            "/api/v1/watch/00000000-0000-7000-8000-000000000000/check",
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn a_failing_page_is_a_check_that_ran_with_an_error_on_the_watch() -> TestResult {
    let app = app().await?;
    let token = app.api_token("t").await?;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let (_, made) = app
        .api(
            Method::POST,
            "/api/v1/watch",
            Some(&token),
            Some(&json!({"url": server.uri(), "paused": true})),
        )
        .await?;
    let (_, waited) = app
        .api(
            Method::POST,
            &format!(
                "/api/v1/watch/{}/check?wait=true",
                made["id"].as_str().ok_or("no id")?
            ),
            Some(&token),
            None,
        )
        .await?;
    assert_eq!(waited["outcome"], "done");
    assert_eq!(waited["watch"]["state"]["consecutive_errors"], 1);
    assert!(
        waited["watch"]["state"]["last_error"]
            .as_str()
            .is_some_and(|e| e.contains("500")),
        "{waited}"
    );
    Ok(())
}

#[tokio::test]
async fn every_watch_endpoint_needs_a_token() -> TestResult {
    let app = app().await?;
    let id = "00000000-0000-7000-8000-000000000000";
    for (method, path) in [
        (Method::GET, "/api/v1/watch".to_owned()),
        (Method::POST, "/api/v1/watch".to_owned()),
        (Method::GET, format!("/api/v1/watch/{id}")),
        (Method::PUT, format!("/api/v1/watch/{id}")),
        (Method::DELETE, format!("/api/v1/watch/{id}")),
        (Method::POST, format!("/api/v1/watch/{id}/check")),
    ] {
        let (status, body) = app
            .api(
                method.clone(),
                &path,
                None,
                Some(&json!({"url": "https://a.example/"})),
            )
            .await?;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
        assert_eq!(body["error"]["code"], "unauthorized");
    }
    Ok(())
}

#[tokio::test]
async fn the_description_holds_the_watch_schemas() -> TestResult {
    let app = app().await?;
    let (_, spec) = app
        .api(Method::GET, "/api/docs/openapi.json", None, None)
        .await?;
    let schemas = &spec["components"]["schemas"];
    for name in [
        "WatchInput",
        "WatchView",
        "Step",
        "Conditions",
        "FetchSettings",
        "Schedule",
        "ErrorBody",
        "CheckResponse",
    ] {
        assert!(
            schemas[name].is_object(),
            "schema {name} is missing: {:?}",
            schemas.as_object().map(|o| o.keys().collect::<Vec<_>>())
        );
    }
    // The steps keep their `type` tag and the new options are described.
    let step = schemas["Step"].to_string();
    assert!(
        step.contains("\"jq\"") && step.contains("\"raw\"") && step.contains("\"lowercase\""),
        "{step}"
    );
    let method_schema = schemas["Method"].to_string();
    assert!(method_schema.contains("\"PATCH\""), "{method_schema}");
    assert!(spec["paths"]["/api/v1/watch/{id}"]["put"].is_object());
    Ok(())
}
