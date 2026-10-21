//! End to end: a change found by a check is delivered to ntfy.

mod support;

use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use vigil_core::{Conditions, Schedule};
use vigil_scheduler::{CheckResult, Scheduler, Settings};
use vigil_store::{DeliveryOutcome, NewTag, NewWatch};
use wiremock::matchers::method;
use wiremock::{Mock, ResponseTemplate};

use support::*;

#[tokio::test]
async fn a_change_triggers_ntfy() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    let CheckResult::Changed {
        previous, current, ..
    } = change(&s, &watch).await?
    else {
        return Err("expected a change".into());
    };

    let reports = s.notifier.close_and_wait().await;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].target, "phone");
    assert_eq!(
        (reports[0].report.attempts, reports[0].report.result.clone()),
        (1, Ok(()))
    );

    let bodies = ntfy_bodies(&s).await?;
    assert_eq!(
        bodies.len(),
        1,
        "the baseline sends nothing, the change sends once"
    );
    assert_eq!(bodies[0]["topic"], "vigil-test");
    assert_eq!(bodies[0]["title"], format!("{} changed", watch.name));
    let message = bodies[0]["message"].as_str().ok_or("no message")?;
    assert!(
        message.contains("Widget: $12") && message.contains("Widget: $10"),
        "{message}"
    );
    assert_eq!(
        bodies[0]["click"],
        format!(
            "http://vigil.test/watch/{}/diff?from={previous}&to={current}",
            watch.id
        )
    );

    let log = s.store.list_deliveries(watch.id, 10).await?;
    assert_eq!(log.len(), 1);
    assert_eq!(
        (log[0].outcome, log[0].attempts, log[0].snapshot),
        (DeliveryOutcome::Sent, 1, Some(current))
    );
    assert_eq!(log[0].error, None);
    Ok(())
}

#[tokio::test]
async fn the_scheduler_notifies_on_its_own() -> TestResult {
    let s = setup().await?;
    let mut new = NewWatch::new(s.page.uri(), Schedule::Interval(1));
    new.name = "shop".into();
    let watch = s.store.create_watch(new).await?;
    let target = add_target(&s, "phone", "vigil-test").await?;
    s.store.link_target(watch.id, target.id).await?;

    // The first request sees version 1, every later one version 2.
    let html = |body: &'static str| ResponseTemplate::new(200).set_body_raw(body, "text/html");
    Mock::given(method("GET"))
        .respond_with(html(PAGE_V1))
        .up_to_n_times(1)
        .mount(&s.page)
        .await;
    Mock::given(method("GET"))
        .respond_with(html(PAGE_V2))
        .mount(&s.page)
        .await;

    let settings = Settings {
        resync: Duration::from_millis(200),
        shutdown_grace: Duration::from_secs(5),
        ..Settings::default()
    };
    let scheduler = Scheduler::new(s.checker.clone(), settings);
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let task = tokio::spawn(async move { scheduler.run(token).await });

    wait_until("a notification", async || {
        s.ntfy
            .received_requests()
            .await
            .is_some_and(|r| !r.is_empty())
    })
    .await?;
    cancel.cancel();
    task.await?;
    s.notifier.shutdown(Duration::from_secs(5)).await;

    let bodies = ntfy_bodies(&s).await?;
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["title"], "shop changed");
    Ok(())
}

#[tokio::test]
async fn nothing_is_sent_when_the_conditions_hold_the_change_back() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |new| {
        new.conditions = Conditions {
            trigger_text: vec!["no such text".into()],
            ..Conditions::default()
        };
    })
    .await?;
    let CheckResult::Changed { verdict, .. } = change(&s, &watch).await? else {
        return Err("expected a change".into());
    };
    assert!(!verdict.notify);
    s.notifier.close_and_wait().await;
    assert!(ntfy_bodies(&s).await?.is_empty());
    assert!(s.store.list_deliveries(watch.id, 10).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn disabled_and_unlinked_targets_are_skipped() -> TestResult {
    let s = setup().await?;
    let watch = s
        .store
        .create_watch(NewWatch::new(s.page.uri(), Schedule::Interval(3600)))
        .await?;
    let on = add_target(&s, "on", "enabled-and-linked").await?;
    let off = add_target(&s, "off", "disabled").await?;
    add_target(&s, "unlinked", "not-linked").await?;
    s.store.link_target(watch.id, on.id).await?;
    s.store.link_target(watch.id, off.id).await?;
    s.store.set_target_enabled(off.id, false).await?;

    change(&s, &watch).await?;
    s.notifier.close_and_wait().await;
    let topics: Vec<Value> = ntfy_bodies(&s)
        .await?
        .into_iter()
        .map(|b| b["topic"].clone())
        .collect();
    assert_eq!(topics, ["enabled-and-linked"]);
    Ok(())
}

#[tokio::test]
async fn a_test_notification_is_sent_and_logged_without_a_watch() -> TestResult {
    let s = setup().await?;
    let target = add_target(&s, "phone", "vigil-test").await?;
    let report = s.notifier.test(&target).await;
    assert_eq!(report.target, "phone");
    assert_eq!(report.report.result, Ok(()));

    let bodies = ntfy_bodies(&s).await?;
    assert_eq!(bodies[0]["title"], "Vigil test notification");
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_log WHERE watch_id IS NULL AND outcome = 'sent'",
    )
    .fetch_one(s.store.pool())
    .await?;
    assert_eq!(rows, 1);
    Ok(())
}

#[tokio::test]
async fn the_default_targets_of_a_tag_are_notified_too() -> TestResult {
    let s = setup().await?;
    let watch = s
        .store
        .create_watch(NewWatch::new(s.page.uri(), Schedule::Interval(3600)))
        .await?;
    let tag = s
        .store
        .create_tag(&NewTag {
            name: "prices".into(),
            color: "blue".into(),
            default_interval_secs: None,
        })
        .await?;
    let from_tag = add_target(&s, "from-tag", "via-tag").await?;
    let both = add_target(&s, "both", "direct-and-tag").await?;
    s.store.set_watch_tags(watch.id, &[tag.id]).await?;
    s.store
        .set_tag_targets(tag.id, &[from_tag.id, both.id])
        .await?;
    s.store.link_target(watch.id, both.id).await?;

    change(&s, &watch).await?;
    s.notifier.close_and_wait().await;
    let mut topics: Vec<String> = ntfy_bodies(&s)
        .await?
        .into_iter()
        .filter_map(|b| b["topic"].as_str().map(str::to_owned))
        .collect();
    topics.sort();
    assert_eq!(
        topics,
        ["direct-and-tag", "via-tag"],
        "a target reached twice is notified once"
    );
    Ok(())
}
