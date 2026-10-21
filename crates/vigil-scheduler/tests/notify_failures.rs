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
async fn a_notification_cut_short_by_shutdown_is_sent_after_a_restart() -> TestResult {
    let s = setup_with(quick_retries(Duration::from_secs(60))).await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    ntfy_answers(&s, 500).await;

    let CheckResult::Changed { current, .. } = change(&s, &watch).await? else {
        return Err("expected a change".into());
    };
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

    // Neither sent nor failed for good: it waits in the outbox.
    assert!(s.store.list_deliveries(watch.id, 10).await?.is_empty());
    assert_eq!(outbox_len(&s.store).await?, 1);

    // The next start finds it, with the same message and link, and sends it.
    ntfy_answers(&s, 200).await;
    let restarted = restarted_notifier(&s)?;
    assert_eq!(restarted.resume().await, 1);
    let reports = restarted.close_and_wait().await;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].report.result, Ok(()));

    let bodies = ntfy_bodies(&s).await?;
    assert_eq!(bodies.len(), 1);
    let message = bodies[0]["message"].as_str().ok_or("no message")?;
    assert!(
        message.contains("Widget: $12") && message.contains("Widget: $10"),
        "{message}"
    );
    assert!(
        bodies[0]["click"]
            .as_str()
            .is_some_and(|c| c.contains(&format!("&to={current}"))),
        "{:?}",
        bodies[0]["click"]
    );
    let log = s.store.list_deliveries(watch.id, 10).await?;
    assert_eq!((log.len(), log[0].outcome), (1, DeliveryOutcome::Sent));
    assert_eq!(outbox_len(&s.store).await?, 0);
    Ok(())
}

#[tokio::test]
async fn a_finished_delivery_leaves_the_outbox() -> TestResult {
    // Sent.
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    change(&s, &watch).await?;
    s.notifier.close_and_wait().await;
    assert_eq!(outbox_len(&s.store).await?, 0);

    // Refused for good.
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    ntfy_answers(&s, 401).await;
    change(&s, &watch).await?;
    s.notifier.close_and_wait().await;
    assert_eq!(outbox_len(&s.store).await?, 0);
    let log = s.store.list_deliveries(watch.id, 10).await?;
    assert_eq!(log[0].outcome, DeliveryOutcome::Failed);

    // Out of quick retries on a failure that may pass: it stays for a later try
    // (see notify_later.rs).
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    ntfy_answers(&s, 503).await;
    change(&s, &watch).await?;
    s.notifier.close_and_wait().await;
    assert_eq!(outbox_len(&s.store).await?, 1);
    Ok(())
}
