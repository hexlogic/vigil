//! Fetcher tests against a local mock server.

use std::io::Write;
use std::time::Duration;

use flate2::Compression;
use flate2::write::GzEncoder;
use vigil_fetch::{FetchConfig, FetchError, FetchOutcome, FetchRequest, Fetcher};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn fetcher() -> Result<Fetcher, FetchError> {
    Fetcher::new(FetchConfig::default())
}

fn fetcher_with_limit(max_body_bytes: u64) -> Result<Fetcher, FetchError> {
    Fetcher::new(FetchConfig {
        max_body_bytes,
        ..FetchConfig::default()
    })
}

fn gzip(data: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

fn body_of(outcome: FetchOutcome) -> Result<vigil_fetch::FetchedBody, String> {
    match outcome {
        FetchOutcome::Body(body) => Ok(body),
        other => Err(format!("expected a body, got {other:?}")),
    }
}

#[tokio::test]
async fn returns_body_and_metadata() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"v1\"")
                .insert_header("last-modified", "Mon, 01 Jan 2024 00:00:00 GMT")
                .set_body_raw("<p>hello</p>", "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let url = format!("{}/page", server.uri());
    let body = body_of(fetcher()?.fetch(&FetchRequest::new(&url)).await?)?;
    assert_eq!(body.status, 200);
    assert_eq!(body.body, b"<p>hello</p>");
    assert_eq!(body.final_url, url);
    assert_eq!(
        body.content_type.as_deref(),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(body.etag.as_deref(), Some("\"v1\""));
    assert_eq!(
        body.last_modified.as_deref(),
        Some("Mon, 01 Jan 2024 00:00:00 GMT")
    );
    Ok(())
}

#[tokio::test]
async fn sends_user_agent_and_custom_headers() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("user-agent", "custom-agent/1"))
        .and(header("x-token", "secret"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let mut request = FetchRequest::new(server.uri());
    request.user_agent = Some("custom-agent/1".into());
    request.headers = vec![("X-Token".into(), "secret".into())];
    body_of(fetcher()?.fetch(&request).await?)?;
    Ok(())
}

#[tokio::test]
async fn default_user_agent_names_vigil() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    body_of(fetcher()?.fetch(&FetchRequest::new(server.uri())).await?)?;

    let requests = server.received_requests().await.unwrap_or_default();
    let agent = requests
        .first()
        .and_then(|r| r.headers.get("user-agent"))
        .and_then(|v| v.to_str().ok());
    assert!(agent.is_some_and(|a| a.starts_with("vigil/")));
    Ok(())
}

#[tokio::test]
async fn decompresses_gzip() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-encoding", "gzip")
                .set_body_bytes(gzip(b"compressed hello")?),
        )
        .mount(&server)
        .await;
    let body = body_of(fetcher()?.fetch(&FetchRequest::new(server.uri())).await?)?;
    assert_eq!(body.body, b"compressed hello");
    Ok(())
}

#[tokio::test]
async fn follows_redirects_and_reports_the_final_url() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(path("/old"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/new"))
        .mount(&server)
        .await;
    Mock::given(path("/new"))
        .respond_with(ResponseTemplate::new(200).set_body_string("moved"))
        .mount(&server)
        .await;

    let body = body_of(
        fetcher()?
            .fetch(&FetchRequest::new(format!("{}/old", server.uri())))
            .await?,
    )?;
    assert_eq!(body.body, b"moved");
    assert_eq!(body.final_url, format!("{}/new", server.uri()));
    Ok(())
}

#[tokio::test]
async fn redirect_loops_fail() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(path("/loop"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/loop"))
        .mount(&server)
        .await;
    let result = fetcher()?
        .fetch(&FetchRequest::new(format!("{}/loop", server.uri())))
        .await;
    assert!(matches!(result, Err(FetchError::Http(_))), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn sends_validators_and_accepts_304() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304).insert_header("etag", "\"v1\""))
        .expect(1)
        .mount(&server)
        .await;

    let mut request = FetchRequest::new(server.uri());
    request.etag = Some("\"v1\"".into());
    request.last_modified = Some("Mon, 01 Jan 2024 00:00:00 GMT".into());
    let outcome = fetcher()?.fetch(&request).await?;
    assert_eq!(
        outcome,
        FetchOutcome::NotModified {
            etag: Some("\"v1\"".into()),
            last_modified: None
        }
    );

    // wiremock's header matcher splits values on commas, so check the date header directly.
    let requests = server.received_requests().await.unwrap_or_default();
    let modified_since = requests
        .first()
        .and_then(|r| r.headers.get("if-modified-since"))
        .and_then(|v| v.to_str().ok());
    assert_eq!(modified_since, Some("Mon, 01 Jan 2024 00:00:00 GMT"));
    Ok(())
}

#[tokio::test]
async fn unsolicited_304_is_an_error() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    let result = fetcher()?.fetch(&FetchRequest::new(server.uri())).await;
    assert!(matches!(result, Err(FetchError::Status(304))), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn no_validators_are_sent_unless_provided() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    body_of(fetcher()?.fetch(&FetchRequest::new(server.uri())).await?)?;
    let requests = server.received_requests().await.unwrap_or_default();
    let sent = requests.first().is_some_and(|r| {
        r.headers.get("if-none-match").is_some() || r.headers.get("if-modified-since").is_some()
    });
    assert!(!sent);
    Ok(())
}

#[tokio::test]
async fn oversized_body_is_rejected_by_content_length() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 2_000]))
        .mount(&server)
        .await;
    let result = fetcher_with_limit(1_000)?
        .fetch(&FetchRequest::new(server.uri()))
        .await;
    assert!(
        matches!(result, Err(FetchError::TooLarge { limit: 1_000 })),
        "{result:?}"
    );
    Ok(())
}

#[tokio::test]
async fn decompressed_size_counts_against_the_limit() -> TestResult {
    // 200 KB of zeros gzips to a few hundred bytes, so only the streaming check can catch it.
    let compressed = gzip(&vec![0u8; 200_000])?;
    assert!(compressed.len() < 1_000);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-encoding", "gzip")
                .set_body_bytes(compressed),
        )
        .mount(&server)
        .await;
    let result = fetcher_with_limit(10_000)?
        .fetch(&FetchRequest::new(server.uri()))
        .await;
    assert!(
        matches!(result, Err(FetchError::TooLarge { limit: 10_000 })),
        "{result:?}"
    );
    Ok(())
}

#[tokio::test]
async fn body_exactly_at_the_limit_is_accepted() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 1_000]))
        .mount(&server)
        .await;
    let body = body_of(
        fetcher_with_limit(1_000)?
            .fetch(&FetchRequest::new(server.uri()))
            .await?,
    )?;
    assert_eq!(body.body.len(), 1_000);
    Ok(())
}

#[tokio::test]
async fn slow_responses_time_out() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(800)))
        .mount(&server)
        .await;
    let mut request = FetchRequest::new(server.uri());
    request.timeout = Some(Duration::from_millis(100));
    let result = fetcher()?.fetch(&request).await;
    assert!(matches!(result, Err(FetchError::Timeout)), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn error_statuses_are_reported() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(path("/boom"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(path("/gone"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let fetcher = fetcher()?;
    for (route, code) in [("/boom", 500), ("/gone", 404)] {
        let result = fetcher
            .fetch(&FetchRequest::new(format!("{}{route}", server.uri())))
            .await;
        assert!(
            matches!(result, Err(FetchError::Status(c)) if c == code),
            "{route}: {result:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn only_http_and_https_are_fetched() -> TestResult {
    let fetcher = fetcher()?;
    for url in [
        "file:///etc/passwd",
        "ftp://example.com/x",
        "data:text/plain,hi",
    ] {
        let result = fetcher.fetch(&FetchRequest::new(url)).await;
        assert!(
            matches!(result, Err(FetchError::UnsupportedScheme(_))),
            "{url}: {result:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn garbage_urls_and_headers_are_rejected_before_sending() -> TestResult {
    let fetcher = fetcher()?;
    let result = fetcher.fetch(&FetchRequest::new("not a url")).await;
    assert!(
        matches!(result, Err(FetchError::InvalidUrl(_))),
        "{result:?}"
    );

    let mut request = FetchRequest::new("http://127.0.0.1:9/");
    request.headers = vec![("bad header".into(), "x".into())];
    let result = fetcher.fetch(&request).await;
    assert!(
        matches!(result, Err(FetchError::InvalidHeader(_))),
        "{result:?}"
    );

    let mut request = FetchRequest::new("http://127.0.0.1:9/");
    request.headers = vec![("x-ok".into(), "line\nbreak".into())];
    let result = fetcher.fetch(&request).await;
    assert!(
        matches!(result, Err(FetchError::InvalidHeader(_))),
        "{result:?}"
    );
    Ok(())
}
