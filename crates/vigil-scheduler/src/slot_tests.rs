//! A preview that is still processing must never make a scheduled check fail.

use vigil_core::{Schedule, WatchId};
use vigil_fetch::{FetchConfig, Fetcher};
use vigil_store::{NewWatch, Store};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::render::SLOTS;
use crate::{CheckResult, Checker};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn a_busy_preview_slot_does_not_make_a_check_fail() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(&dir.path().join("vigil.db")).await?;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("<p>hello</p>", "text/html"))
        .mount(&server)
        .await;
    let checker = Checker::new(store.clone(), Fetcher::new(FetchConfig::default())?);
    let watch = store
        .create_watch(NewWatch::new(server.uri(), Schedule::Interval(60)))
        .await?;
    let draft: WatchId = "00000000-0000-7000-8000-0000000000aa".parse()?;

    // The processing thread of an earlier preview of this form is still alive.
    let _earlier_preview = SLOTS.acquire(draft)?;

    // The next preview of the form is refused, as one thread per key is the rule...
    let page = checker.fetch_for_preview(&watch).await?;
    let error = checker
        .preview_page(&watch, &page, draft)
        .await?
        .result
        .err()
        .ok_or("the preview should have been refused")?;
    assert!(error.message.contains("still running"), "{error}");
    assert!(
        checker
            .validate_watch(&watch, draft)
            .await
            .is_err_and(|e| e.message.contains("still running"))
    );

    // ...but the scheduled check of the same watch is not.
    let report = checker.check(watch.id).await?;
    assert!(matches!(report.result, CheckResult::Baseline), "{report:?}");
    let stored = store.get_watch(watch.id).await?;
    assert_eq!(stored.state.consecutive_errors, 0);
    assert_eq!(stored.state.last_error, None);
    Ok(())
}
