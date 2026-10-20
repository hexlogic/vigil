//! A delivery that goes wrong never turns into a failed check, and templates, bad targets and
//! shutdown behave.

mod support;

use std::time::{Duration, Instant};

use vigil_core::Schedule;
use vigil_scheduler::CheckResult;
use vigil_store::{DeliveryOutcome, NewTarget, NewWatch};
use wiremock::matchers::method;
use wiremock::{Mock, ResponseTemplate};

use support::*;

#[tokio::test]
async fn a_failing_channel_never_fails_the_check() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    s.ntfy.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&s.ntfy)
        .await;

    let result = change(&s, &watch).await?;
    assert!(matches!(result, CheckResult::Changed { .. }), "{result:?}");
    let reports = s.notifier.close_and_wait().await;
    assert_eq!(reports[0].report.attempts, 4);

    assert_eq!(
        s.store.get_watch(watch.id).await?.state.consecutive_errors,
        0
    );
    let log: Vec<String> =
        sqlx::query_scalar("SELECT outcome FROM check_log WHERE watch_id = ? ORDER BY id")
            .bind(watch.id.to_string())
            .fetch_all(s.store.pool())
            .await?;
    assert_eq!(log, ["changed", "changed"]);

    let deliveries = s.store.list_deliveries(watch.id, 10).await?;
    assert_eq!(
        (deliveries[0].outcome, deliveries[0].attempts),
        (DeliveryOutcome::Failed, 4)
    );
    assert!(
        deliveries[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("HTTP 500")),
        "{:?}",
        deliveries[0].error
    );
    Ok(())
}

#[tokio::test]
async fn a_broken_watch_template_falls_back_to_the_built_in_one() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    // Passes the store; fails when rendered, as a template can once the values differ.
    let bomb = "{% for i in range(100000) %}{% for j in range(100000) %}x{% endfor %}{% endfor %}";
    s.store
        .set_notification_template(watch.id, Some(bomb))
        .await?;

    change(&s, &watch).await?;
    s.notifier.close_and_wait().await;
    let bodies = ntfy_bodies(&s).await?;
    assert!(
        bodies[0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("Widget: $12")),
        "{:?}",
        bodies[0]
    );
    let log = s.store.list_deliveries(watch.id, 10).await?;
    assert_eq!(log[0].outcome, DeliveryOutcome::Sent);
    assert!(
        log[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("custom template")),
        "{:?}",
        log[0].error
    );
    Ok(())
}

#[tokio::test]
async fn a_watch_template_wins_over_the_targets() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    s.store
        .set_notification_template(watch.id, Some("watch: {{ watch_name }}"))
        .await?;
    sqlx::query("UPDATE notification_targets SET default_template = 'target: {{ watch_name }}'")
        .execute(s.store.pool())
        .await?;

    change(&s, &watch).await?;
    s.notifier.close_and_wait().await;
    assert_eq!(
        ntfy_bodies(&s).await?[0]["message"],
        format!("watch: {}", watch.name)
    );

    s.store.set_notification_template(watch.id, None).await?;
    s.store.get_watch(watch.id).await?;
    serve(&s.page, PAGE_V1).await;
    s.checker.check(watch.id).await?;
    s.notifier.close_and_wait().await;
    assert_eq!(
        ntfy_bodies(&s).await?[1]["message"],
        format!("target: {}", watch.name)
    );
    Ok(())
}

#[tokio::test]
async fn a_slow_channel_does_not_hold_up_the_check() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    s.ntfy.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
        .mount(&s.ntfy)
        .await;

    serve(&s.page, PAGE_V1).await;
    s.checker.check(watch.id).await?;
    serve(&s.page, PAGE_V2).await;
    let started = Instant::now();
    s.checker.check(watch.id).await?;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the check waited for the delivery: {:?}",
        started.elapsed()
    );

    let reports = s.notifier.close_and_wait().await;
    assert_eq!(reports[0].report.result, Ok(()));
    Ok(())
}

#[tokio::test]
async fn a_target_with_a_bad_stored_url_fails_cleanly() -> TestResult {
    let s = setup().await?;
    let watch = s
        .store
        .create_watch(NewWatch::new(s.page.uri(), Schedule::Interval(3600)))
        .await?;
    let new = NewTarget {
        name: "broken".into(),
        url: "S3CRET-without-a-scheme".into(),
        default_template: None,
    };
    let target = s.store.create_target(&new).await?;
    s.store.link_target(watch.id, target.id).await?;

    change(&s, &watch).await?;
    let reports = s.notifier.close_and_wait().await;
    assert_eq!(reports[0].report.attempts, 0);
    let log = s.store.list_deliveries(watch.id, 10).await?;
    assert_eq!(
        (log[0].outcome, log[0].attempts),
        (DeliveryOutcome::Failed, 0)
    );
    let error = log[0].error.clone().unwrap_or_default();
    assert!(
        error.contains("not valid") && !error.contains("S3CRET"),
        "{error}"
    );
    Ok(())
}

#[tokio::test]
async fn shutdown_stops_waiting_to_retry() -> TestResult {
    let s = setup_with(quick_retries(Duration::from_secs(60))).await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    s.ntfy.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&s.ntfy)
        .await;

    change(&s, &watch).await?;
    wait_until("the first attempt", async || {
        s.ntfy
            .received_requests()
            .await
            .is_some_and(|r| !r.is_empty())
    })
    .await?;
    let started = Instant::now();
    s.notifier.shutdown(Duration::from_secs(10)).await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );

    let log = s.store.list_deliveries(watch.id, 10).await?;
    assert_eq!(log[0].outcome, DeliveryOutcome::Failed);
    assert!(
        log[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("shutting down")),
        "{:?}",
        log[0].error
    );
    Ok(())
}
