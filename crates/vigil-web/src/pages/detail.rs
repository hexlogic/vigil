//! One watch: its settings, its recent checks and the notifications sent for it.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, State};
use axum::response::Response;
use vigil_store::{CheckOutcome, DeliveryOutcome, StoreError, now_ms};

use super::watches::Row;
use super::{not_found, parse_id, server_error};
use crate::AppState;
use crate::fmt::{self, TimeView};
use crate::page::{Ctx, Nav, render};

/// Checks and deliveries shown.
const RECENT: u32 = 15;

struct CheckView {
    when: TimeView,
    outcome: String,
    class: &'static str,
    duration: String,
    error: Option<String>,
}

struct DeliveryView {
    when: TimeView,
    target: String,
    outcome: String,
    class: &'static str,
    attempts: u32,
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "watch_show.html")]
struct Show {
    ctx: Ctx,
    row: Row,
    processor: String,
    filters: String,
    jitter: String,
    retention: String,
    targets: Vec<String>,
    custom_template: bool,
    snapshots: usize,
    history_url: String,
    latest_diff_url: Option<String>,
    back: String,
    checks: Vec<CheckView>,
    deliveries: Vec<DeliveryView>,
}

/// `GET /watch/{id}`.
pub(crate) async fn show(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> Response {
    let Some(id) = parse_id(&id) else {
        return not_found(ctx).await;
    };
    match load(&state, &ctx, id).await {
        Ok(page) => render(&page),
        Err(StoreError::NotFound(_)) => not_found(ctx).await,
        Err(error) => server_error(ctx, &error),
    }
}

async fn load(state: &AppState, ctx: &Ctx, id: vigil_core::WatchId) -> Result<Show, StoreError> {
    let t = ctx.translator();
    let now = now_ms();
    let watch = state.store.get_watch(id).await?;
    let tags = state.store.tags_for_watch(id).await?;
    let linked = state.store.targets_for_watch(id, false).await?;
    let snapshots = state.store.list_snapshots(id, 1000).await?;
    let checks = state.store.list_checks(id, RECENT).await?;
    let deliveries = state.store.list_deliveries(id, RECENT).await?;
    let names: HashMap<_, _> = state
        .store
        .list_targets()
        .await?
        .into_iter()
        .map(|target| (target.id, target.name))
        .collect();

    let tag_refs: Vec<&vigil_store::Tag> = tags.iter().collect();
    let row = Row::new(ctx, &watch, &tag_refs, now);
    let latest_diff_url = match snapshots.as_slice() {
        [newest, previous, ..] => Some(ctx.url(&format!(
            "/watch/{id}/diff?from={}&to={}",
            previous.id, newest.id
        ))),
        _ => None,
    };
    Ok(Show {
        processor: t.t(match watch.processor {
            vigil_core::Processor::Text => "form.processor_text",
            vigil_core::Processor::Json => "form.processor_json",
            vigil_core::Processor::Feed => "form.processor_feed",
        }),
        filters: t.plural("detail.filters", watch.pipeline.len()),
        jitter: if watch.jitter_secs == 0 {
            t.t("detail.jitter_default")
        } else {
            format!("{}s", watch.jitter_secs)
        },
        retention: t.plural("detail.versions", watch.snapshot_retention as usize),
        targets: linked.into_iter().map(|target| target.name).collect(),
        custom_template: watch.notification_template.is_some(),
        snapshots: snapshots.len(),
        history_url: ctx.url(&format!("/watch/{id}/history")),
        latest_diff_url,
        back: ctx.url(&format!("/watch/{id}")),
        checks: checks
            .into_iter()
            .map(|check| {
                let (outcome, class) = match check.outcome {
                    CheckOutcome::Unchanged => ("detail.check.unchanged", ""),
                    CheckOutcome::Changed => ("detail.check.changed", "badge-ok"),
                    CheckOutcome::Error => ("detail.check.error", "badge-error"),
                };
                CheckView {
                    when: fmt::when(t, Some(check.at), now),
                    outcome: t.t(outcome),
                    class,
                    duration: format!("{} ms", check.duration_ms),
                    error: check.error,
                }
            })
            .collect(),
        deliveries: deliveries
            .into_iter()
            .map(|delivery| {
                let (outcome, class) = match delivery.outcome {
                    DeliveryOutcome::Sent => ("detail.delivery.sent", "badge-ok"),
                    DeliveryOutcome::Failed => ("detail.delivery.failed", "badge-error"),
                };
                DeliveryView {
                    when: fmt::when(t, Some(delivery.at), now),
                    target: names
                        .get(&delivery.target)
                        .cloned()
                        .unwrap_or_else(|| t.t("detail.delivery.removed_target")),
                    outcome: t.t(outcome),
                    class,
                    attempts: delivery.attempts,
                    error: delivery.error,
                }
            })
            .collect(),
        row,
        ctx: ctx.clone().nav(Nav::Watches),
    })
}
