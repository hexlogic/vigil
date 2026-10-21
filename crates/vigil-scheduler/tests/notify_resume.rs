//! Notifications left in the outbox by a crash or a shutdown are found at the next start.

mod support;

use vigil_notify::Event;
use vigil_scheduler::CheckResult;
use vigil_store::{DeliveryOutcome, NewPending};

use support::*;

/// Put a notification in the outbox the way a process that died mid-delivery leaves it.
async fn leave_behind(
    s: &Setup,
    watch: &vigil_core::Watch,
) -> Result<i64, Box<dyn std::error::Error>> {
    let target = s.store.targets_for_watch(watch.id, false).await?.remove(0);
    let mut event = Event::sample();
    event.watch_id = watch.id.to_string();
    event.watch_name = watch.name.clone();
    let pending = NewPending {
        watch: watch.id,
        target: target.id,
        snapshot: uuid_of_a_snapshot(s, watch).await?,
        payload: event.to_json()?,
    };
    Ok(s.store.enqueue_notification(&pending).await?)
}

async fn uuid_of_a_snapshot(
    s: &Setup,
    watch: &vigil_core::Watch,
) -> Result<vigil_store::SnapshotId, Box<dyn std::error::Error>> {
    serve(&s.page, PAGE_V1).await;
    match s.checker.check(watch.id).await?.result {
        CheckResult::Baseline => {}
        other => return Err(format!("expected a baseline, got {other:?}").into()),
    }
    let snapshots = s.store.list_snapshots(watch.id, 1).await?;
    Ok(snapshots.first().ok_or("no snapshot")?.id)
}

#[tokio::test]
async fn what_a_crash_left_behind_is_sent_at_the_next_start() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    leave_behind(&s, &watch).await?;
    assert_eq!(outbox_len(&s.store).await?, 1);

    let restarted = restarted_notifier(&s)?;
    assert_eq!(restarted.resume().await, 1);
    let reports = restarted.close_and_wait().await;
    assert_eq!(reports[0].report.result, Ok(()));

    let bodies = ntfy_bodies(&s).await?;
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies[0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("Widget: $12")),
        "{:?}",
        bodies[0]
    );
    assert_eq!(outbox_len(&s.store).await?, 0);
    assert_eq!(
        s.store.list_deliveries(watch.id, 10).await?[0].outcome,
        DeliveryOutcome::Sent
    );
    Ok(())
}

#[tokio::test]
async fn the_watchs_template_still_applies_after_a_restart() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    s.store
        .set_notification_template(watch.id, Some("moved: {{ current_snapshot }}"))
        .await?;
    leave_behind(&s, &watch).await?;

    let restarted = restarted_notifier(&s)?;
    restarted.resume().await;
    restarted.close_and_wait().await;
    // The page text is not kept in the outbox; it is read back from the snapshot.
    let message = ntfy_bodies(&s).await?[0]["message"]
        .as_str()
        .map(str::to_owned);
    assert!(
        message
            .as_deref()
            .is_some_and(|m| m.starts_with("moved: # Price list") && m.contains("Widget: $10")),
        "{message:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_notification_a_day_old_is_not_sent() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    let id = leave_behind(&s, &watch).await?;
    let two_days_ago = vigil_store::now_ms() - 2 * 24 * 60 * 60 * 1000;
    sqlx::query("UPDATE notification_outbox SET created_at = ? WHERE id = ?")
        .bind(two_days_ago)
        .bind(id)
        .execute(s.store.pool())
        .await?;

    let restarted = restarted_notifier(&s)?;
    assert_eq!(restarted.resume().await, 0);
    restarted.close_and_wait().await;
    assert!(ntfy_bodies(&s).await?.is_empty());
    assert_eq!(outbox_len(&s.store).await?, 0);
    let log = s.store.list_deliveries(watch.id, 10).await?;
    assert_eq!(log[0].outcome, DeliveryOutcome::Failed);
    assert!(
        log[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("too old")),
        "{:?}",
        log[0].error
    );
    Ok(())
}

#[tokio::test]
async fn a_disabled_or_removed_target_drops_its_saved_notifications() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    leave_behind(&s, &watch).await?;
    let target = s.store.targets_for_watch(watch.id, false).await?.remove(0);
    s.store.set_target_enabled(target.id, false).await?;

    let restarted = restarted_notifier(&s)?;
    assert_eq!(restarted.resume().await, 0);
    restarted.close_and_wait().await;
    assert!(ntfy_bodies(&s).await?.is_empty());
    assert_eq!(outbox_len(&s.store).await?, 0);
    Ok(())
}

#[tokio::test]
async fn resuming_with_nothing_saved_does_nothing() -> TestResult {
    let s = setup().await?;
    let restarted = restarted_notifier(&s)?;
    assert_eq!(restarted.resume().await, 0);
    assert!(restarted.close_and_wait().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_failure_that_would_outlast_a_day_is_final() -> TestResult {
    let s = setup_with_later(
        quick_retries(std::time::Duration::from_millis(10)),
        vec![std::time::Duration::from_secs(2 * 3600)],
    )
    .await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    let id = leave_behind(&s, &watch).await?;
    // Saved 23 hours ago: a try two hours from now would be past the limit.
    let long_ago = vigil_store::now_ms() - 23 * 60 * 60 * 1000;
    sqlx::query("UPDATE notification_outbox SET created_at = ? WHERE id = ?")
        .bind(long_ago)
        .bind(id)
        .execute(s.store.pool())
        .await?;
    ntfy_answers(&s, 503).await;

    let restarted =
        restarted_notifier_with_later(&s, vec![std::time::Duration::from_secs(2 * 3600)])?;
    assert_eq!(restarted.resume().await, 1);
    let reports = restarted.close_and_wait().await;
    assert!(!reports[0].queued);
    assert_eq!(outbox_len(&s.store).await?, 0);
    let log = s.store.list_deliveries(watch.id, 10).await?;
    assert_eq!(log[0].outcome, DeliveryOutcome::Failed);
    assert!(
        !log[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("trying again"),
        "{:?}",
        log[0].error
    );
    Ok(())
}

#[tokio::test]
async fn a_waiting_notification_that_is_over_a_day_old_is_dropped_when_due() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    let id = leave_behind(&s, &watch).await?;
    let now = vigil_store::now_ms();
    sqlx::query("UPDATE notification_outbox SET created_at = ?, next_attempt_at = ?, rounds = 3 WHERE id = ?")
        .bind(now - 25 * 60 * 60 * 1000)
        .bind(now - 1000)
        .bind(id)
        .execute(s.store.pool())
        .await?;

    assert_eq!(s.notifier.retry_due().await, 0);
    assert_eq!(outbox_len(&s.store).await?, 0);
    assert!(ntfy_bodies(&s).await?.is_empty());
    let log = s.store.list_deliveries(watch.id, 10).await?;
    assert!(
        log[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("too old")),
        "{:?}",
        log[0].error
    );
    Ok(())
}
