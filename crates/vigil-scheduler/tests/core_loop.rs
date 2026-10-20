//! The Phase 1 acceptance test: a real checker, store and fetcher against a mock web server.

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use vigil_core::{Conditions, Schedule, SelectMode, Step, VerdictReason, Watch};
use vigil_fetch::{FetchConfig, Fetcher};
use vigil_scheduler::{Backend, CheckResult, Checker, Scheduler, Settings};
use vigil_store::{NewWatch, Store, StoreError};
use wiremock::matchers::{header, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const PAGE_V1: &str =
    "<html><body><h1>Price list</h1><p>Widget: $10</p><p>Gadget: $20</p></body></html>";
const PAGE_V2: &str =
    "<html><body><h1>Price list</h1><p>Widget: $12</p><p>Gadget: $20</p></body></html>";

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

async fn serve_page(server: &MockServer, body: &str) {
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/html; charset=utf-8"))
        .mount(server)
        .await;
}

async fn serve_json(server: &MockServer, body: &str) {
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
        .mount(server)
        .await;
}

async fn serve_xml(server: &MockServer, body: &str) {
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/rss+xml"))
        .mount(server)
        .await;
}

async fn add_watch(setup: &Setup, interval: u64) -> Result<Watch, StoreError> {
    let schedule = Schedule::Interval(interval);
    setup
        .store
        .create_watch(NewWatch::new(setup.server.uri(), schedule))
        .await
}

async fn outcomes(store: &Store, watch: &Watch) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT outcome FROM check_log WHERE watch_id = ? ORDER BY id")
        .bind(watch.id.to_string())
        .fetch_all(store.pool())
        .await
}

#[tokio::test]
async fn page_v1_then_v2_yields_two_snapshots_and_a_correct_diff() -> TestResult {
    let s = setup().await?;
    let watch = add_watch(&s, 3600).await?;

    // Page v1: the first check stores the baseline.
    serve_page(&s.server, PAGE_V1).await;
    let first = s.checker.check(watch.id).await?;
    assert_eq!(first.result, CheckResult::Baseline);
    assert_eq!(s.store.list_snapshots(watch.id, 10).await?.len(), 1);

    // The same page again is not a change and stores nothing.
    let again = s.checker.check(watch.id).await?;
    assert_eq!(again.result, CheckResult::Unchanged);
    assert_eq!(s.store.list_snapshots(watch.id, 10).await?.len(), 1);

    // Page v2: one price changed.
    s.server.reset().await;
    serve_page(&s.server, PAGE_V2).await;
    let third = s.checker.check(watch.id).await?;
    let CheckResult::Changed {
        previous,
        current,
        diff,
        verdict,
    } = third.result
    else {
        return Err(format!("expected a change, got {:?}", third.result).into());
    };

    // Exactly two snapshots, newest first, holding the two page versions.
    let snapshots = s.store.list_snapshots(watch.id, 10).await?;
    assert_eq!(
        snapshots.iter().map(|snap| snap.id).collect::<Vec<_>>(),
        [current, previous]
    );
    let old_text = s.store.snapshot_content(previous).await?;
    let new_text = s.store.snapshot_content(current).await?;
    assert!(
        old_text.contains("Widget: $10") && !old_text.contains("$12"),
        "{old_text}"
    );
    assert!(
        new_text.contains("Widget: $12") && !new_text.contains("$10"),
        "{new_text}"
    );

    // Without conditions every change is worth a notification.
    assert!(verdict.notify, "{verdict:?}");

    // The diff names exactly the changed line.
    assert_eq!(diff.removed().collect::<Vec<_>>(), ["Widget: $10"]);
    assert_eq!(diff.added().collect::<Vec<_>>(), ["Widget: $12"]);
    let unified = diff.unified(1);
    assert!(
        unified.contains("-Widget: $10\n+Widget: $12\n"),
        "{unified}"
    );

    // And the watch state and log agree.
    assert_eq!(
        outcomes(&s.store, &watch).await?,
        ["changed", "unchanged", "changed"]
    );
    let state = s.store.get_watch(watch.id).await?.state;
    assert_eq!(state.consecutive_errors, 0);
    assert!(state.last_changed_at.is_some());
    Ok(())
}

#[tokio::test]
async fn a_retention_of_one_still_reports_changes() -> TestResult {
    let s = setup().await?;
    let mut new = NewWatch::new(s.server.uri(), Schedule::Interval(3600));
    new.snapshot_retention = 1;
    let watch = s.store.create_watch(new).await?;

    serve_page(&s.server, PAGE_V1).await;
    assert_eq!(
        s.checker.check(watch.id).await?.result,
        CheckResult::Baseline
    );
    s.server.reset().await;
    serve_page(&s.server, PAGE_V2).await;

    let CheckResult::Changed { diff, current, .. } = s.checker.check(watch.id).await?.result else {
        return Err("expected a change".into());
    };
    assert_eq!(diff.removed().collect::<Vec<_>>(), ["Widget: $10"]);
    assert_eq!(diff.added().collect::<Vec<_>>(), ["Widget: $12"]);
    let snapshots = s.store.list_snapshots(watch.id, 10).await?;
    assert_eq!(
        snapshots.iter().map(|snap| snap.id).collect::<Vec<_>>(),
        [current]
    );
    assert_eq!(outcomes(&s.store, &watch).await?, ["changed", "changed"]);
    Ok(())
}

#[tokio::test]
async fn a_304_counts_as_unchanged_without_a_new_snapshot() -> TestResult {
    let s = setup().await?;
    let watch = add_watch(&s, 3600).await?;
    Mock::given(method("GET"))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304).insert_header("etag", "\"v1\""))
        .with_priority(1)
        .mount(&s.server)
        .await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"v1\"")
                .set_body_raw(PAGE_V1, "text/html"),
        )
        .with_priority(5)
        .mount(&s.server)
        .await;

    assert_eq!(
        s.checker.check(watch.id).await?.result,
        CheckResult::Baseline
    );
    assert_eq!(
        s.checker.check(watch.id).await?.result,
        CheckResult::Unchanged
    );

    let requests = s.server.received_requests().await.unwrap_or_default();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].headers.get("if-none-match").is_none());
    assert!(requests[1].headers.get("if-none-match").is_some());
    assert_eq!(s.store.list_snapshots(watch.id, 10).await?.len(), 1);
    assert_eq!(
        s.store.get_watch(watch.id).await?.state.etag.as_deref(),
        Some("\"v1\"")
    );
    assert_eq!(outcomes(&s.store, &watch).await?, ["changed", "unchanged"]);
    Ok(())
}

#[tokio::test]
async fn failures_are_recorded_and_cleared_by_recovery() -> TestResult {
    let s = setup().await?;
    let watch = add_watch(&s, 3600).await?;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&s.server)
        .await;

    let report = s.checker.check(watch.id).await?;
    let CheckResult::Failed {
        message,
        consecutive_errors,
    } = report.result
    else {
        return Err(format!("expected a failure, got {:?}", report.result).into());
    };
    assert_eq!(consecutive_errors, 1);
    assert_eq!(message, "server answered http 500");
    let state = s.store.get_watch(watch.id).await?.state;
    assert_eq!(
        state.last_error.as_deref(),
        Some("server answered http 500")
    );
    assert!(s.store.list_snapshots(watch.id, 10).await?.is_empty());

    s.server.reset().await;
    serve_page(&s.server, PAGE_V1).await;
    assert_eq!(
        s.checker.check(watch.id).await?.result,
        CheckResult::Baseline
    );
    let state = s.store.get_watch(watch.id).await?.state;
    assert_eq!((state.consecutive_errors, state.last_error), (0, None));
    assert_eq!(outcomes(&s.store, &watch).await?, ["error", "changed"]);
    Ok(())
}

#[tokio::test]
async fn an_unreachable_host_is_a_recorded_failure_with_its_cause() -> TestResult {
    let s = setup().await?;
    let watch = s
        .store
        .create_watch(NewWatch::new(
            "http://127.0.0.1:1/",
            Schedule::Interval(3600),
        ))
        .await?;
    let report = s.checker.check(watch.id).await?;
    let CheckResult::Failed { message, .. } = report.result else {
        return Err(format!("expected a failure, got {:?}", report.result).into());
    };
    assert!(message.starts_with("request failed: "), "{message}");
    assert!(message.contains("127.0.0.1"), "{message}");
    Ok(())
}

#[tokio::test]
async fn checking_a_missing_watch_is_a_store_error() -> TestResult {
    let s = setup().await?;
    let watch = add_watch(&s, 3600).await?;
    s.store.delete_watch(watch.id).await?;
    assert!(matches!(
        s.checker.check(watch.id).await,
        Err(StoreError::NotFound(_))
    ));
    Ok(())
}

const FEED_V1: &str = "<rss version=\"2.0\"><channel><title>News</title>\
    <item><title>Older</title><link>https://n.example/1</link>\
    <pubDate>Mon, 05 Oct 2026 10:00:00 GMT</pubDate></item></channel></rss>";
const FEED_V2: &str = "<rss version=\"2.0\"><channel><title>News (renamed)</title>\
    <item><title>Older</title><link>https://n.example/1</link>\
    <pubDate>Mon, 05 Oct 2026 10:00:00 GMT</pubDate></item>\
    <item><title>Newer</title><link>https://n.example/2</link>\
    <pubDate>Tue, 06 Oct 2026 10:00:00 GMT</pubDate></item></channel></rss>";

#[tokio::test]
async fn feed_watches_report_new_items_and_ignore_feed_metadata() -> TestResult {
    let s = setup().await?;
    let mut new = NewWatch::new(s.server.uri(), Schedule::Interval(3600));
    new.processor = vigil_core::Processor::Feed;
    let watch = s.store.create_watch(new).await?;

    serve_xml(&s.server, FEED_V1).await;
    assert_eq!(
        s.checker.check(watch.id).await?.result,
        CheckResult::Baseline
    );

    // A new item shows up as exactly the added block; the renamed channel is not a change.
    serve_xml(&s.server, FEED_V2).await;
    let CheckResult::Changed { current, diff, .. } = s.checker.check(watch.id).await?.result else {
        return Err("expected a change".into());
    };
    assert_eq!(
        s.store.snapshot_content(current).await?,
        "Newer\nhttps://n.example/2\n2026-10-06T10:00:00Z\n\n\
         Older\nhttps://n.example/1\n2026-10-05T10:00:00Z\n"
    );
    assert_eq!(diff.removed().count(), 0);
    assert!(diff.added().any(|line| line == "Newer"));
    Ok(())
}

#[tokio::test]
async fn a_feed_watch_on_an_html_page_fails_the_check_clearly() -> TestResult {
    let s = setup().await?;
    serve_page(&s.server, PAGE_V1).await;
    let mut new = NewWatch::new(s.server.uri(), Schedule::Interval(3600));
    new.processor = vigil_core::Processor::Feed;
    let watch = s.store.create_watch(new).await?;
    let CheckResult::Failed { message, .. } = s.checker.check(watch.id).await?.result else {
        return Err("expected a failure".into());
    };
    assert!(
        message.starts_with("the response is not a valid feed: "),
        "{message}"
    );
    Ok(())
}

#[tokio::test]
async fn json_watches_store_canonical_json_and_ignore_key_order() -> TestResult {
    let s = setup().await?;
    let mut new = NewWatch::new(s.server.uri(), Schedule::Interval(3600));
    new.processor = vigil_core::Processor::Json;
    let watch = s.store.create_watch(new).await?;

    serve_json(&s.server, r#"{"price": 10, "name": "Widget"}"#).await;
    assert_eq!(
        s.checker.check(watch.id).await?.result,
        CheckResult::Baseline
    );
    let snapshots = s.store.list_snapshots(watch.id, 10).await?;
    assert_eq!(
        s.store.snapshot_content(snapshots[0].id).await?,
        "{\n  \"name\": \"Widget\",\n  \"price\": 10\n}\n"
    );

    // The same data in another key order and layout is not a change.
    serve_json(&s.server, "{\"name\":\"Widget\",\n\"price\":10}").await;
    assert_eq!(
        s.checker.check(watch.id).await?.result,
        CheckResult::Unchanged
    );

    // A changed value is, and the diff names only that line.
    serve_json(&s.server, r#"{"name": "Widget", "price": 12}"#).await;
    let CheckResult::Changed { diff, .. } = s.checker.check(watch.id).await?.result else {
        return Err("expected a change".into());
    };
    assert_eq!(diff.removed().collect::<Vec<_>>(), ["  \"price\": 10"]);
    assert_eq!(diff.added().collect::<Vec<_>>(), ["  \"price\": 12"]);
    Ok(())
}

#[tokio::test]
async fn a_json_watch_on_an_html_page_fails_the_check_clearly() -> TestResult {
    let s = setup().await?;
    serve_page(&s.server, PAGE_V1).await;
    let mut new = NewWatch::new(s.server.uri(), Schedule::Interval(3600));
    new.processor = vigil_core::Processor::Json;
    let watch = s.store.create_watch(new).await?;
    let CheckResult::Failed { message, .. } = s.checker.check(watch.id).await?.result else {
        return Err("expected a failure".into());
    };
    assert!(
        message.starts_with("the response is not valid JSON: "),
        "{message}"
    );
    Ok(())
}

#[tokio::test]
async fn only_unpaused_watches_are_scheduled() -> TestResult {
    let s = setup().await?;
    let active = add_watch(&s, 60).await?;
    let paused = add_watch(&s, 60).await?;
    sqlx::query("UPDATE watches SET paused = 1 WHERE id = ?")
        .bind(paused.id.to_string())
        .execute(s.store.pool())
        .await?;

    let jobs = s.checker.load().await?;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, active.id);
    assert_eq!(jobs[0].interval, Duration::from_secs(60));
    assert_eq!(jobs[0].host, vigil_fetch::host_key(&s.server.uri()));
    Ok(())
}

#[tokio::test]
async fn the_scheduler_checks_watches_on_its_own() -> TestResult {
    let s = setup().await?;
    let watch = add_watch(&s, 1).await?;
    serve_page(&s.server, PAGE_V1).await;

    let settings = Settings {
        resync: Duration::from_millis(200),
        shutdown_grace: Duration::from_secs(5),
        ..Settings::default()
    };
    let scheduler = Scheduler::new(s.checker.clone(), settings);
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let task = tokio::spawn(async move { scheduler.run(token).await });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if outcomes(&s.store, &watch).await?.len() >= 2 {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            return Err("the scheduler did not run two checks in time".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cancel.cancel();
    task.await?;

    let logged = outcomes(&s.store, &watch).await?;
    assert_eq!(&logged[..2], ["changed", "unchanged"]);
    assert_eq!(s.store.list_snapshots(watch.id, 10).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn intervals_outside_the_supported_range_are_clamped() -> TestResult {
    let s = setup().await?;
    let instant = add_watch(&s, 0).await?;
    let forever = add_watch(&s, u64::MAX).await?;

    let jobs = s.checker.load().await?;
    let interval_of = |id| jobs.iter().find(|job| job.id == id).map(|job| job.interval);
    assert_eq!(interval_of(instant.id), Some(Duration::from_secs(1)));
    assert_eq!(
        interval_of(forever.id),
        Some(vigil_scheduler::backoff::MAX_INTERVAL)
    );
    Ok(())
}

/// A page with a price and a banner that changes on every visit.
fn shop(banner: &str, price: &str) -> String {
    format!(
        "<html><body><div class=banner>{banner}</div>\
         <p>Widget</p><span id=price>{price}</span></body></html>"
    )
}

async fn add_with(s: &Setup, configure: impl FnOnce(&mut NewWatch)) -> Result<Watch, StoreError> {
    let mut new = NewWatch::new(s.server.uri(), Schedule::Interval(3600));
    configure(&mut new);
    s.store.create_watch(new).await
}

fn css(selector: &str) -> Step {
    Step::Css {
        selector: selector.into(),
        mode: SelectMode::Include,
        required: false,
    }
}

#[tokio::test]
async fn a_css_pipeline_ignores_changes_outside_the_selection() -> TestResult {
    let s = setup().await?;
    let watch = add_with(&s, |new| new.pipeline = vec![css("#price")]).await?;

    serve_page(&s.server, &shop("Welcome", "$10")).await;
    assert_eq!(
        s.checker.check(watch.id).await?.result,
        CheckResult::Baseline
    );

    s.server.reset().await;
    serve_page(&s.server, &shop("Autumn sale!", "$10")).await;
    assert_eq!(
        s.checker.check(watch.id).await?.result,
        CheckResult::Unchanged
    );

    s.server.reset().await;
    serve_page(&s.server, &shop("Autumn sale!", "$12")).await;
    let CheckResult::Changed { diff, .. } = s.checker.check(watch.id).await?.result else {
        return Err("expected a change".into());
    };
    assert_eq!(diff.removed().collect::<Vec<_>>(), ["$10"]);
    assert_eq!(diff.added().collect::<Vec<_>>(), ["$12"]);
    Ok(())
}

#[tokio::test]
async fn something_that_appears_later_is_a_change_not_a_baseline() -> TestResult {
    let s = setup().await?;
    let watch = add_with(&s, |new| new.pipeline = vec![css(".sale")]).await?;

    // Nothing selected is a normal, empty result, so the watch is not in error and keeps its
    // normal schedule while it waits.
    serve_page(&s.server, "<p>Regular prices</p>").await;
    assert_eq!(
        s.checker.check(watch.id).await?.result,
        CheckResult::Baseline
    );
    assert_eq!(
        s.checker.check(watch.id).await?.result,
        CheckResult::Unchanged
    );
    assert_eq!(
        s.store.get_watch(watch.id).await?.state.consecutive_errors,
        0
    );

    s.server.reset().await;
    serve_page(
        &s.server,
        "<p>Regular prices</p><p class=sale>Half price today</p>",
    )
    .await;
    let CheckResult::Changed { diff, verdict, .. } = s.checker.check(watch.id).await?.result else {
        return Err("expected a change".into());
    };
    assert_eq!(diff.added().collect::<Vec<_>>(), ["Half price today"]);
    assert!(verdict.notify);
    Ok(())
}

#[tokio::test]
async fn a_required_selection_that_finds_nothing_fails_the_check() -> TestResult {
    let s = setup().await?;
    let required = Step::Css {
        selector: ".sale".into(),
        mode: SelectMode::Include,
        required: true,
    };
    let watch = add_with(&s, |new| new.pipeline = vec![required]).await?;
    serve_page(&s.server, "<p>Regular prices</p>").await;
    let CheckResult::Failed {
        message,
        consecutive_errors,
    } = s.checker.check(watch.id).await?.result
    else {
        return Err("expected a failure".into());
    };
    assert_eq!(message, "step 1 (css): matched nothing");
    assert_eq!(consecutive_errors, 1);
    assert!(s.store.list_snapshots(watch.id, 10).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn an_invalid_pipeline_fails_the_check_naming_the_step() -> TestResult {
    let s = setup().await?;
    let broken = Step::RegexReplace {
        pattern: "(".into(),
        replacement: "".into(),
    };
    let watch = add_with(&s, |new| new.pipeline = vec![css("p"), broken]).await?;
    serve_page(&s.server, PAGE_V1).await;
    let CheckResult::Failed { message, .. } = s.checker.check(watch.id).await?.result else {
        return Err("expected a failure".into());
    };
    assert!(
        message.starts_with("step 2 (regex_replace): invalid regex \"(\": "),
        "{message}"
    );
    assert!(s.store.list_snapshots(watch.id, 10).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn invalid_conditions_fail_the_check_before_anything_is_fetched() -> TestResult {
    let s = setup().await?;
    let watch = add_with(&s, |new| {
        new.conditions = Conditions {
            trigger_text: vec!["/(/".into()],
            ..Conditions::default()
        };
    })
    .await?;
    serve_page(&s.server, PAGE_V1).await;
    let CheckResult::Failed { message, .. } = s.checker.check(watch.id).await?.result else {
        return Err("expected a failure".into());
    };
    assert!(
        message.starts_with("conditions: trigger_text[0]: invalid regex"),
        "{message}"
    );
    assert!(
        s.server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn trigger_text_decides_whether_a_change_notifies() -> TestResult {
    let s = setup().await?;
    let watch = add_with(&s, |new| {
        new.conditions.trigger_text = vec!["gadget: $15".into()];
    })
    .await?;
    serve_page(&s.server, PAGE_V1).await;
    s.checker.check(watch.id).await?;

    // The widget price changes: stored, but not what the watch is waiting for.
    s.server.reset().await;
    serve_page(&s.server, PAGE_V2).await;
    let CheckResult::Changed { verdict, .. } = s.checker.check(watch.id).await?.result else {
        return Err("expected a change".into());
    };
    assert!(!verdict.notify);
    assert_eq!(verdict.reason, VerdictReason::NoTriggerMatched);

    s.server.reset().await;
    serve_page(&s.server, &PAGE_V2.replace("Gadget: $20", "Gadget: $15")).await;
    let CheckResult::Changed { verdict, .. } = s.checker.check(watch.id).await?.result else {
        return Err("expected a change".into());
    };
    assert!(verdict.notify);
    assert_eq!(verdict.triggered_text, ["Gadget: $15"]);
    Ok(())
}

#[tokio::test]
async fn a_blocked_change_is_still_stored_and_logged() -> TestResult {
    let s = setup().await?;
    let watch = add_with(&s, |new| new.conditions.block_text = vec!["$12".into()]).await?;
    serve_page(&s.server, PAGE_V1).await;
    s.checker.check(watch.id).await?;
    s.server.reset().await;
    serve_page(&s.server, PAGE_V2).await;
    let CheckResult::Changed { verdict, .. } = s.checker.check(watch.id).await?.result else {
        return Err("expected a change".into());
    };
    assert!(!verdict.notify);
    assert_eq!(verdict.reason, VerdictReason::Blocked("$12".into()));
    assert_eq!(s.store.list_snapshots(watch.id, 10).await?.len(), 2);
    assert_eq!(outcomes(&s.store, &watch).await?, ["changed", "changed"]);
    Ok(())
}

#[tokio::test]
async fn an_edited_pipeline_runs_even_when_the_server_answers_304() -> TestResult {
    let s = setup().await?;
    // The server answers 304 to a request that carries the validator, 200 to one without.
    Mock::given(method("GET"))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304).insert_header("etag", "\"v1\""))
        .with_priority(1)
        .mount(&s.server)
        .await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"v1\"")
                .set_body_raw(shop("Welcome", "$10"), "text/html"),
        )
        .with_priority(2)
        .mount(&s.server)
        .await;

    let watch = add_with(&s, |_| {}).await?;
    assert_eq!(
        s.checker.check(watch.id).await?.result,
        CheckResult::Baseline
    );
    assert_eq!(
        s.checker.check(watch.id).await?.result,
        CheckResult::Unchanged
    );

    let mut settings = NewWatch::from(&s.store.get_watch(watch.id).await?);
    settings.pipeline = vec![css("#price")];
    s.store.update_watch(watch.id, &settings).await?;

    let CheckResult::Changed { diff, .. } = s.checker.check(watch.id).await?.result else {
        return Err("the new pipeline did not run".into());
    };
    // The page text shrank to the price: the banner and the product name are gone.
    let removed: Vec<_> = diff.removed().filter(|line| !line.is_empty()).collect();
    assert_eq!(removed, ["Welcome", "Widget"]);
    assert_eq!(diff.added_count(), 0);
    Ok(())
}
