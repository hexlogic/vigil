//! The API's front door: tokens, the open mode, errors in JSON and the description.

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use serde_json::{Value, json};
use vigil_web::{Auth, WebConfig};

use common::*;

const VERSION: &str = "/api/v1/version";

#[tokio::test]
async fn a_call_needs_a_token_made_here() -> TestResult {
    let app = app().await?;

    let (status, body) = app.api(Method::GET, VERSION, None, None).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");

    for wrong in [
        "nope",
        &format!("vigil_{}", "0".repeat(64)),
        &format!("other_{}", "a".repeat(64)),
    ] {
        let (status, _) = app.api(Method::GET, VERSION, Some(wrong), None).await?;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{wrong}");
    }

    let token = app.api_token("script").await?;
    let (status, body) = app.api(Method::GET, VERSION, Some(&token), None).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["access"], "token");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    Ok(())
}

#[tokio::test]
async fn a_signed_in_browser_is_not_a_token() -> TestResult {
    let app = app().await?;
    assert!(app.cookie.is_some());
    // The same browser, with its session cookie and from this very site, is still refused.
    let response = app
        .send(
            Request::get(VERSION)
                .header(header::ORIGIN, "http://localhost")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn a_revoked_token_stops_working_and_use_is_noted() -> TestResult {
    let app = app().await?;
    let token = app.api_token("ci").await?;
    assert_eq!(app.store.list_api_tokens().await?[0].last_used_at, None);

    let (status, _) = app.api(Method::GET, VERSION, Some(&token), None).await?;
    assert_eq!(status, StatusCode::OK);
    assert!(app.store.list_api_tokens().await?[0].last_used_at.is_some());

    let id = app.store.resolve_api_token("ci").await?.id;
    app.store.delete_api_token(id).await?;
    let (status, _) = app.api(Method::GET, VERSION, Some(&token), None).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn with_sign_in_off_no_token_is_needed_but_the_request_must_be_local_and_ours() -> TestResult
{
    let app = app_with(WebConfig::new(Auth::Disabled, None)).await?;

    let (status, body) = app.api(Method::GET, VERSION, None, None).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["access"], "open");

    let send = |headers: &[(&str, &str)]| {
        let mut request = Request::get(VERSION);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.body(Body::empty())
    };
    // A name that is not an address: DNS rebinding.
    let response = app.send(send(&[("host", "attacker.example")])?).await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let rebinding: Value = serde_json::from_str(response.body())?;
    assert_eq!(rebinding["error"]["code"], "forbidden");

    // A page of another site, the way a browser says so.
    for headers in [
        [("origin", "http://evil.example")],
        [("sec-fetch-site", "cross-site")],
    ] {
        let response = app.send(send(&headers)?).await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{headers:?}");
    }

    // A text/plain POST is a request a page can make without a CORS preflight.
    let response = app
        .send(
            Request::post("/api/v1/version")
                .header(header::CONTENT_TYPE, "text/plain")
                .header(header::ORIGIN, "http://evil.example")
                .body(Body::from("{}"))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // The site's own pages, and tools that send neither header, are fine.
    let response = app
        .send(send(&[
            ("origin", "http://localhost"),
            ("sec-fetch-site", "same-origin"),
        ])?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn anything_that_is_not_an_endpoint_is_a_json_error() -> TestResult {
    let app = app().await?;
    let token = app.api_token("script").await?;

    let (status, body) = app
        .api(Method::GET, "/api/v1/nothing", Some(&token), None)
        .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "no_such_endpoint");
    // The answer does not depend on having a token: it says nothing about the data.
    let (status, body) = app.api(Method::GET, "/api/other", None, None).await?;
    assert_eq!(
        (status, &body["error"]["code"]),
        (StatusCode::NOT_FOUND, &json!("no_such_endpoint"))
    );

    // A wrong method names the ones that work.
    let response = app
        .signed_out()
        .send(
            Request::delete(VERSION)
                .header("x-api-key", &token)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert!(header_of(&response, "allow").contains("GET"));
    let body: Value = serde_json::from_str(response.body())?;
    assert_eq!(body["error"]["code"], "method_not_allowed");

    // The UI keeps its own 404 page.
    let page = app.get("/no/such/page").await?;
    assert_eq!(page.status(), StatusCode::NOT_FOUND);
    assert!(page.body().contains("<html"), "{}", page.body());
    Ok(())
}

#[tokio::test]
async fn the_description_is_open_and_lists_what_the_router_serves() -> TestResult {
    let app = app().await?;

    let (status, pointer) = app.api(Method::GET, "/api/docs", None, None).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pointer["openapi"], "/api/docs/openapi.json");

    let (status, spec) = app
        .api(Method::GET, "/api/docs/openapi.json", None, None)
        .await?;
    assert_eq!(status, StatusCode::OK);
    assert!(
        spec["openapi"]
            .as_str()
            .is_some_and(|v| v.starts_with("3.")),
        "{spec}"
    );
    assert_eq!(spec["info"]["title"], "Vigil API");
    assert_eq!(
        spec["components"]["securitySchemes"]["api_key"],
        json!({
            "type": "apiKey",
            "in": "header",
            "name": "x-api-key",
            "description": "An API token made on the settings page or with `vigil token add`. Not needed while sign-in is off."
        })
    );

    // Every path of the description is a route that exists. A route that is documented but
    // not served would answer with the "no such endpoint" 404.
    let token = app.api_token("script").await?;
    let paths = spec["paths"]
        .as_object()
        .ok_or("the description has no paths")?;
    assert!(paths.contains_key(VERSION));
    for (path, item) in paths {
        assert!(path.starts_with("/api/v1/"), "{path}");
        for method in item.as_object().ok_or("a path is not an object")?.keys() {
            let method: Method = method.to_uppercase().parse()?;
            let concrete = path.replace("{id}", "00000000-0000-7000-8000-000000000000");
            let (_, body) = app
                .api(method.clone(), &concrete, Some(&token), None)
                .await?;
            assert_ne!(
                body["error"]["code"], "no_such_endpoint",
                "{method} {path} is documented but not served"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn under_a_base_path_the_api_and_its_description_move_with_the_ui() -> TestResult {
    let app = app_with(WebConfig::new(
        Auth::Password,
        Some("https://example.com/vigil"),
    ))
    .await?;
    let token = app.api_token("script").await?;

    let (status, _) = app
        .api(Method::GET, "/vigil/api/v1/version", Some(&token), None)
        .await?;
    assert_eq!(status, StatusCode::OK);
    // Outside the prefix there is nothing, as for the UI.
    let outside = app
        .signed_out()
        .send(
            Request::get(VERSION)
                .header("x-api-key", &token)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(outside.status(), StatusCode::NOT_FOUND);

    let (_, pointer) = app.api(Method::GET, "/vigil/api/docs", None, None).await?;
    assert_eq!(pointer["openapi"], "/vigil/api/docs/openapi.json");
    let (_, spec) = app
        .api(Method::GET, "/vigil/api/docs/openapi.json", None, None)
        .await?;
    assert_eq!(spec["servers"][0]["url"], "/vigil");
    Ok(())
}
