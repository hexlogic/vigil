//! A notification whose quick retries ran out on a failure that may pass is tried again later.

mod support;

use std::time::Duration;

use vigil_scheduler::CheckResult;
use vigil_store::DeliveryOutcome;

use support::*;

async fn pause() {
    tokio::time::sleep(Duration::from_millis(120)).await;
}

#[tokio::test]
async fn an_outage_longer_than_the_quick_retries_is_ridden_out() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    ntfy_answers(&s, 503).await;
    let CheckResult::Changed { .. } = change(&s, &watch).await? else {
        return Err("expected a change".into());
    };

    // The quick retries ran out; the notification is kept and the check was not affected.
    let reports = s.notifier.close_and_wait().await;
    assert_eq!(
        (reports.len(), reports[0].queued, reports[0].report.attempts),
        (1, true, 4)
    );
    let error = reports[0].report.result.clone().expect_err("failed");
    assert!(
        error.starts_with("HTTP 503") && error.contains("trying again in"),
        "{error}"
    );
    let waiting = s.store.pending_notifications().await?;
    assert_eq!((waiting.len(), waiting[0].rounds), (1, 1));
    assert!(waiting[0].next_attempt_at.is_some_and(|at| at > 0));
    assert_eq!(
        s.store.get_watch(watch.id).await?.state.consecutive_errors,
        0
    );

    // The service is back: the next sweep delivers it.
    ntfy_answers(&s, 200).await;
    pause().await;
    assert_eq!(s.notifier.retry_due().await, 1);
    let reports = s.notifier.close_and_wait().await;
    assert_eq!(
        (reports[0].report.result.clone(), reports[0].queued),
        (Ok(()), false)
    );
    assert_eq!(outbox_len(&s.store).await?, 0);

    let bodies = ntfy_bodies(&s).await?;
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies[0]["message"]
            .as_str()
            .is_some_and(|m| m.contains("Widget: $12")),
        "{:?}",
        bodies[0]
    );
    // Each round is in the log: the failed one says it will be tried again, the last one is sent.
    let log = s.store.list_deliveries(watch.id, 10).await?;
    assert_eq!(
        log.iter().map(|d| d.outcome).collect::<Vec<_>>(),
        [DeliveryOutcome::Sent, DeliveryOutcome::Failed]
    );
    assert!(
        log[1]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("trying again")),
        "{:?}",
        log[1].error
    );
    Ok(())
}

#[tokio::test]
async fn nothing_is_tried_before_its_time() -> TestResult {
    let s = setup_with_later(
        quick_retries(Duration::from_millis(10)),
        vec![Duration::from_secs(3600)],
    )
    .await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    ntfy_answers(&s, 503).await;
    change(&s, &watch).await?;
    s.notifier.close_and_wait().await;

    ntfy_answers(&s, 200).await;
    pause().await;
    assert_eq!(s.notifier.retry_due().await, 0);
    assert_eq!(outbox_len(&s.store).await?, 1);
    assert!(ntfy_bodies(&s).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_due_notification_is_taken_by_one_caller() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    ntfy_answers(&s, 503).await;
    change(&s, &watch).await?;
    s.notifier.close_and_wait().await;

    ntfy_answers(&s, 200).await;
    pause().await;
    let (a, b, c) = tokio::join!(
        s.notifier.retry_due(),
        s.notifier.retry_due(),
        s.notifier.retry_due()
    );
    assert_eq!(a + b + c, 1);
    s.notifier.close_and_wait().await;
    assert_eq!(
        ntfy_bodies(&s).await?.len(),
        1,
        "sent once, not once per caller"
    );
    Ok(())
}

#[tokio::test]
async fn later_rounds_make_one_attempt_each() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    ntfy_answers(&s, 503).await;
    change(&s, &watch).await?;
    s.notifier.close_and_wait().await;
    pause().await;
    assert_eq!(s.notifier.retry_due().await, 1);
    let reports = s.notifier.close_and_wait().await;

    // Four quick attempts, then one in the later round.
    assert_eq!((reports[0].report.attempts, reports[0].queued), (1, true));
    assert_eq!(
        s.ntfy.received_requests().await.unwrap_or_default().len(),
        5
    );
    assert_eq!(s.store.pending_notifications().await?[0].rounds, 2);
    Ok(())
}

#[tokio::test]
async fn a_waiting_notification_survives_a_restart() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    ntfy_answers(&s, 503).await;
    change(&s, &watch).await?;
    // Let the quick retries run out before the process "stops".
    wait_until("the notification to wait", async || {
        s.store
            .pending_notifications()
            .await
            .is_ok_and(|p| p.first().is_some_and(|n| n.next_attempt_at.is_some()))
    })
    .await?;
    s.notifier.shutdown(Duration::from_secs(5)).await;

    ntfy_answers(&s, 200).await;
    let restarted = restarted_notifier(&s)?;
    // Start-up resumes what was in progress, not what is waiting for its time.
    assert_eq!(restarted.resume().await, 0);
    pause().await;
    assert_eq!(restarted.retry_due().await, 1);
    assert_eq!(restarted.close_and_wait().await[0].report.result, Ok(()));
    assert_eq!(outbox_len(&s.store).await?, 0);
    Ok(())
}

#[tokio::test]
async fn a_permanent_refusal_is_not_kept() -> TestResult {
    let s = setup().await?;
    let watch = watch_with_target(&s, |_| {}).await?;
    ntfy_answers(&s, 403).await;
    change(&s, &watch).await?;
    let reports = s.notifier.close_and_wait().await;
    assert_eq!((reports[0].queued, reports[0].report.attempts), (false, 1));
    assert_eq!(outbox_len(&s.store).await?, 0);
    Ok(())
}

#[tokio::test]
async fn the_sweeper_retries_on_its_own_until_stopped() -> TestResult {
    let s = std::sync::Arc::new(setup().await?);
    let watch = watch_with_target(&s, |_| {}).await?;
    ntfy_answers(&s, 503).await;
    change(&s, &watch).await?;
    s.notifier.close_and_wait().await;
    ntfy_answers(&s, 200).await;

    let stop = tokio_util::sync::CancellationToken::new();
    let notifier = std::sync::Arc::clone(&s.notifier);
    let token = stop.clone();
    let sweeper =
        tokio::spawn(async move { notifier.run_retries(Duration::from_millis(40), token).await });
    wait_until("the retry", async || {
        outbox_len(&s.store).await.is_ok_and(|n| n == 0)
    })
    .await?;
    stop.cancel();
    sweeper.await?;
    s.notifier.close_and_wait().await;
    assert_eq!(ntfy_bodies(&s).await?.len(), 1);
    Ok(())
}
