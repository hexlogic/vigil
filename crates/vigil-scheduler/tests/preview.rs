//! Previews: what a check would store, without storing it.

use vigil_core::{Schedule, SelectMode, Step, Watch};
use vigil_fetch::{FetchConfig, Fetcher};
use vigil_scheduler::{CheckResult, Checker};
use vigil_store::{NewWatch, Store};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Setup {
    _dir: tempfile::TempDir,
    store: Store,
    checker: Checker,
    server: MockServer,
}

async fn setup() -> Result<Setup, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    let checker = Checker::new(store.clone(), Fetcher::new(FetchConfig::default())?);
    Ok(Setup {
        _dir: dir,
        store,
        checker,
        server: MockServer::start().await,
    })
}

async fn serve(server: &MockServer, template: ResponseTemplate) {
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(template)
        .mount(server)
        .await;
}

fn page(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "text/html; charset=utf-8")
}

async fn add_watch(s: &Setup) -> Result<Watch, Box<dyn std::error::Error>> {
    Ok(s.store
        .create_watch(NewWatch::new(s.server.uri(), Schedule::Interval(3600)))
        .await?)
}

async fn snapshot_rows(s: &Setup) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM snapshots")
        .fetch_one(s.store.pool())
        .await
}

async fn log_rows(s: &Setup) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM check_log")
        .fetch_one(s.store.pool())
        .await
}

#[tokio::test]
async fn a_preview_shows_the_text_and_stores_nothing() -> TestResult {
    let s = setup().await?;
    let watch = add_watch(&s).await?;
    serve(&s.server, page("<p>Widget: $10</p>")).await;

    let report = s.checker.preview(&watch).await?;
    let preview = report.result?;
    assert_eq!(preview.text, "Widget: $10\n");
    assert_eq!(preview.http_status, 200);
    assert_eq!(preview.raw_size, 18);
    assert_eq!(
        preview.content_type.as_deref(),
        Some("text/html; charset=utf-8")
    );
    // No snapshot yet, so there is nothing to compare with.
    assert!(preview.latest.is_none() && preview.diff.is_none() && preview.verdict.is_none());

    assert_eq!(snapshot_rows(&s).await?, 0);
    assert_eq!(log_rows(&s).await?, 0);
    let stored = s.store.get_watch(watch.id).await?;
    assert_eq!(stored.state.last_checked_at, None);
    assert_eq!(stored.state.last_hash, None);
    Ok(())
}

#[tokio::test]
async fn a_preview_compares_with_the_latest_snapshot() -> TestResult {
    let s = setup().await?;
    let watch = add_watch(&s).await?;
    serve(&s.server, page("<p>Widget: $10</p><p>Gadget: $20</p>")).await;
    assert_eq!(
        s.checker.check(watch.id).await?.result,
        CheckResult::Baseline
    );

    // Same page: no difference and no verdict to give.
    let same = s.checker.preview(&watch).await?.result?;
    assert!(same.latest.is_some());
    assert!(same.diff.as_ref().is_some_and(|diff| diff.is_empty()));
    assert!(same.verdict.is_none());

    serve(&s.server, page("<p>Widget: $12</p><p>Gadget: $20</p>")).await;
    let changed = s.checker.preview(&watch).await?.result?;
    let diff = changed.diff.ok_or("no diff")?;
    assert_eq!(diff.removed().collect::<Vec<_>>(), ["Widget: $10"]);
    assert_eq!(diff.added().collect::<Vec<_>>(), ["Widget: $12"]);
    assert!(changed.verdict.ok_or("no verdict")?.notify);

    // Still only the baseline.
    assert_eq!(snapshot_rows(&s).await?, 1);
    assert_eq!(log_rows(&s).await?, 1);
    Ok(())
}

#[tokio::test]
async fn a_preview_applies_unsaved_settings() -> TestResult {
    let s = setup().await?;
    let watch = add_watch(&s).await?;
    serve(&s.server, page("<p>Menu</p><p class=price>$10</p>")).await;
    s.checker.check(watch.id).await?;

    let mut edited = watch.clone();
    edited.pipeline = vec![Step::Css {
        selector: ".price".into(),
        mode: SelectMode::Include,
        required: false,
    }];
    edited.conditions.block_text = vec!["menu".into()];
    let preview = s.checker.preview(&edited).await?.result?;
    assert_eq!(preview.text, "$10\n");
    let diff = preview.diff.ok_or("no diff")?;
    assert_eq!(
        diff.removed()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>(),
        ["Menu"]
    );
    // The removed line is not where blocks look, so the change still notifies.
    assert!(preview.verdict.ok_or("no verdict")?.notify);

    // The stored watch is untouched.
    assert!(s.store.get_watch(watch.id).await?.pipeline.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_preview_sends_no_validators() -> TestResult {
    let s = setup().await?;
    let watch = add_watch(&s).await?;
    sqlx::query(
        "UPDATE watches SET etag = '\"v1\"', last_modified = 'Mon, 05 Oct 2026 10:00:00 GMT'",
    )
    .execute(s.store.pool())
    .await?;
    let watch = s.store.get_watch(watch.id).await?;
    assert!(watch.state.etag.is_some());
    serve(&s.server, page("<p>x</p>")).await;

    s.checker.preview(&watch).await?.result?;
    let requests = s.server.received_requests().await.ok_or("no requests")?;
    assert_eq!(requests.len(), 1);
    let headers = &requests[0].headers;
    assert!(!headers.contains_key("if-none-match") && !headers.contains_key("if-modified-since"));

    // A real check does send them.
    s.checker.check(watch.id).await?;
    let requests = s.server.received_requests().await.ok_or("no requests")?;
    assert!(requests[1].headers.contains_key("if-none-match"));
    Ok(())
}

#[tokio::test]
async fn a_preview_reports_problems_instead_of_recording_them() -> TestResult {
    let s = setup().await?;
    let watch = add_watch(&s).await?;

    serve(&s.server, ResponseTemplate::new(500)).await;
    let message = s
        .checker
        .preview(&watch)
        .await?
        .result
        .err()
        .ok_or("expected an error")?
        .message;
    assert!(message.contains("500"), "{message}");

    // A bare 304 is the fetcher's own error. A conditional header of the watch's own makes it
    // legitimate, and then there is still no body to preview.
    serve(&s.server, ResponseTemplate::new(304)).await;
    let message = s
        .checker
        .preview(&watch)
        .await?
        .result
        .err()
        .ok_or("expected an error")?
        .message;
    assert!(message.contains("304"), "{message}");
    let mut conditional = watch.clone();
    conditional
        .fetch
        .headers
        .insert("If-None-Match".into(), "\"x\"".into());
    let message = s
        .checker
        .preview(&conditional)
        .await?
        .result
        .err()
        .ok_or("expected an error")?
        .message;
    assert!(
        message.starts_with("the server answered 304 Not Modified"),
        "{message}"
    );

    serve(&s.server, page("<p>x</p>")).await;
    let mut broken = watch.clone();
    broken.pipeline = vec![Step::Css {
        selector: ".missing".into(),
        mode: SelectMode::Include,
        required: true,
    }];
    let message = s
        .checker
        .preview(&broken)
        .await?
        .result
        .err()
        .ok_or("expected an error")?
        .message;
    assert_eq!(message, "step 1 (css): matched nothing");

    let mut bad_conditions = watch.clone();
    bad_conditions.conditions.trigger_text = vec!["/(/".into()];
    let message = s
        .checker
        .preview(&bad_conditions)
        .await?
        .result
        .err()
        .ok_or("expected an error")?
        .message;
    assert!(
        message.starts_with("conditions: trigger_text[0]: invalid regex"),
        "{message}"
    );

    // None of it counts against the watch.
    let stored = s.store.get_watch(watch.id).await?;
    assert_eq!(stored.state.consecutive_errors, 0);
    assert_eq!(stored.state.last_error, None);
    assert_eq!(log_rows(&s).await?, 0);
    Ok(())
}

/// The processing slot of a form being edited. The limit of one processing thread per slot is
/// shared by the whole process, so tests that run side by side each use a slot of their own.
fn draft_slot(n: u32) -> Result<vigil_core::WatchId, Box<dyn std::error::Error>> {
    Ok(format!("00000000-0000-7000-8000-{n:012}").parse()?)
}

fn css(selector: &str) -> Step {
    Step::Css {
        selector: selector.into(),
        mode: SelectMode::Include,
        required: true,
    }
}

#[tokio::test]
async fn a_fetched_page_can_be_previewed_again_with_other_settings() -> TestResult {
    let s = setup().await?;
    let watch = add_watch(&s).await?;
    serve(
        &s.server,
        page("<h1>Shop</h1><p class=\"price\">$10</p><p class=\"stock\">in stock</p>"),
    )
    .await;

    let page = s.checker.fetch_for_preview(&watch).await?;
    assert_eq!(page.status(), 200);
    assert_eq!(page.content_type(), Some("text/html; charset=utf-8"));

    let mut price = watch.clone();
    price.pipeline = vec![css(".price")];
    let first = s
        .checker
        .preview_page(&price, &page, draft_slot(177)?)
        .await?;
    assert_eq!(first.result?.text, "$10\n");

    let mut stock = watch.clone();
    stock.pipeline = vec![css(".stock")];
    let second = s
        .checker
        .preview_page(&stock, &page, draft_slot(177)?)
        .await?;
    let second = second.result?;
    assert_eq!(second.text, "in stock\n");
    assert_eq!(second.raw_size, page.size());

    let requests = s.server.received_requests().await.unwrap_or_default();
    assert_eq!(requests.len(), 1, "the page was fetched once");
    assert_eq!(snapshot_rows(&s).await?, 0);
    assert_eq!(log_rows(&s).await?, 0);
    Ok(())
}

#[tokio::test]
async fn a_preview_names_the_step_that_failed() -> TestResult {
    let s = setup().await?;
    let watch = add_watch(&s).await?;
    serve(&s.server, page("<p class=\"a\">x</p>")).await;

    let page = s.checker.fetch_for_preview(&watch).await?;
    let mut broken = watch.clone();
    broken.pipeline = vec![css(".a"), css(".missing")];
    let error = s
        .checker
        .preview_page(&broken, &page, draft_slot(178)?)
        .await?
        .result
        .err()
        .ok_or("expected an error")?;
    assert_eq!(error.step, Some(2), "{error:?}");
    assert_eq!(error.message, "step 2 (css): matched nothing");

    // A problem that is not in a step has no step.
    let mut bad_conditions = watch.clone();
    bad_conditions.conditions.block_text = vec!["/(/".into()];
    let error = s
        .checker
        .preview_page(&bad_conditions, &page, draft_slot(178)?)
        .await?
        .result
        .err()
        .ok_or("expected an error")?;
    assert_eq!(error.step, None);
    Ok(())
}

#[tokio::test]
async fn validating_a_watch_finds_a_bad_step_without_fetching() -> TestResult {
    let s = setup().await?;
    let watch = add_watch(&s).await?;
    serve(&s.server, page("<p>x</p>")).await;

    s.checker.validate_watch(&watch, draft_slot(179)?).await?;

    let mut broken = watch.clone();
    broken.pipeline = vec![
        css("p"),
        Step::RegexExtract {
            pattern: "(".into(),
            group: 0,
            required: false,
        },
    ];
    let error = s
        .checker
        .validate_watch(&broken, draft_slot(179)?)
        .await
        .err()
        .ok_or("expected an error")?;
    assert_eq!(error.step, Some(2));

    let mut bad_conditions = watch.clone();
    bad_conditions.conditions.trigger_text = vec!["/(/".into()];
    let error = s
        .checker
        .validate_watch(&bad_conditions, draft_slot(179)?)
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(error.message.starts_with("conditions:"), "{error}");

    let requests = s.server.received_requests().await.unwrap_or_default();
    assert!(requests.is_empty(), "nothing was fetched");
    Ok(())
}
