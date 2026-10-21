//! The notification targets, and what can be done to one from its row.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use uuid::Uuid;
use vigil_notify::Channel;
use vigil_store::{NotificationTarget, StoreError};

use super::watches::is_htmx;
use super::{not_found, server_error};
use crate::AppState;
use crate::page::{Ctx, Nav, render};

/// How a test notification went, shown under the target it was sent to.
pub(crate) struct TestOutcome {
    pub(crate) ok: bool,
    pub(crate) message: String,
}

/// One target in the list.
pub(crate) struct TargetRow {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: String,
    /// The address with every secret replaced by `***`, or why it cannot be read.
    pub(crate) address: String,
    pub(crate) enabled: bool,
    pub(crate) watches: String,
    pub(crate) own_template: bool,
    /// The address sends a secret over plain HTTP.
    pub(crate) cleartext: bool,
    pub(crate) test: Option<TestOutcome>,
    pub(crate) row_url: String,
    pub(crate) test_url: String,
    pub(crate) toggle_url: String,
    pub(crate) edit_url: String,
    pub(crate) delete_url: String,
}

impl TargetRow {
    pub(crate) fn new(ctx: &Ctx, target: &NotificationTarget, watches: u32) -> Self {
        let t = ctx.translator();
        let (kind, address, cleartext) = match Channel::parse(&target.url) {
            Ok(channel) => (
                channel.kind().to_owned(),
                channel.redacted(),
                channel.sends_secrets_in_cleartext(),
            ),
            // The message never repeats the address.
            Err(error) => (
                String::new(),
                t.t1("targets.invalid", "reason", error),
                false,
            ),
        };
        let id = target.id.to_string();
        let at = |suffix: &str| ctx.url(&format!("/notifications/{id}{suffix}"));
        Self {
            name: target.name.clone(),
            kind,
            address,
            enabled: target.enabled,
            watches: t.plural("targets.watches", watches as usize),
            own_template: target.default_template.is_some(),
            cleartext,
            test: None,
            row_url: at("/row"),
            test_url: at("/test"),
            toggle_url: at(if target.enabled {
                "/disable"
            } else {
                "/enable"
            }),
            edit_url: at("/edit"),
            delete_url: at("/delete"),
            id,
        }
    }
}

#[derive(Template)]
#[template(path = "targets.html")]
struct Targets {
    ctx: Ctx,
    rows: Vec<TargetRow>,
    add_url: String,
}

#[derive(Template)]
#[template(path = "target_row.html")]
struct RowFragment {
    ctx: Ctx,
    row: TargetRow,
}

#[derive(Template)]
#[template(path = "target_delete.html")]
struct DeleteFragment {
    ctx: Ctx,
    row: TargetRow,
}

/// `GET /notifications`.
pub(crate) async fn targets(State(state): State<AppState>, ctx: Ctx) -> Response {
    let loaded = async {
        let targets = state.store.list_targets().await?;
        let counts = state.store.linked_watch_counts().await?;
        Ok::<_, StoreError>((targets, counts))
    }
    .await;
    match loaded {
        Ok((targets, counts)) => {
            let rows = targets
                .iter()
                .map(|target| {
                    TargetRow::new(&ctx, target, counts.get(&target.id).copied().unwrap_or(0))
                })
                .collect();
            render(&Targets {
                add_url: ctx.url("/notifications/new"),
                ctx: ctx.nav(Nav::Notifications),
                rows,
            })
        }
        Err(error) => server_error(ctx, &error),
    }
}

async fn row_of(state: &AppState, ctx: &Ctx, id: Uuid) -> Result<TargetRow, StoreError> {
    let target = state.store.get_target(id).await?;
    let counts: HashMap<_, _> = state.store.linked_watch_counts().await?;
    Ok(TargetRow::new(
        ctx,
        &target,
        counts.get(&id).copied().unwrap_or(0),
    ))
}

async fn row_response(state: &AppState, ctx: Ctx, id: &str, test: Option<TestOutcome>) -> Response {
    let Ok(id) = id.parse::<Uuid>() else {
        return not_found(ctx).await;
    };
    match row_of(state, &ctx, id).await {
        Ok(mut row) => {
            row.test = test;
            render(&RowFragment {
                ctx: ctx.nav(Nav::Notifications),
                row,
            })
        }
        Err(StoreError::NotFound(_)) => not_found(ctx).await,
        Err(error) => server_error(ctx, &error),
    }
}

/// `GET /notifications/{id}/row`.
pub(crate) async fn target_row(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> Response {
    row_response(&state, ctx, &id, None).await
}

/// `POST /notifications/{id}/test`: send a test message now, and show how it went.
pub(crate) async fn test_target(
    State(state): State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Ok(uuid) = id.parse::<Uuid>() else {
        return not_found(ctx).await;
    };
    let target = match state.store.get_target(uuid).await {
        Ok(target) => target,
        Err(StoreError::NotFound(_)) => return not_found(ctx).await,
        Err(error) => return server_error(ctx, &error),
    };
    // One attempt: a person is waiting for the answer.
    let report = state.notifier.test_once(&target).await;
    if !is_htmx(&headers) {
        return Redirect::to(&ctx.url("/notifications")).into_response();
    }
    let t = ctx.translator();
    let outcome = match report.report.result {
        Ok(()) => TestOutcome {
            ok: true,
            message: report.report.note.map_or_else(
                || t.t("targets.test_sent"),
                |note| t.t1("targets.test_sent_note", "note", note),
            ),
        },
        Err(message) => TestOutcome {
            ok: false,
            message: t.t1("targets.test_failed", "reason", message),
        },
    };
    row_response(&state, ctx, &id, Some(outcome)).await
}

/// `POST /notifications/{id}/enable`.
pub(crate) async fn enable_target(
    state: State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    id: Path<String>,
) -> Response {
    set_enabled(state, ctx, headers, id, true).await
}

/// `POST /notifications/{id}/disable`.
pub(crate) async fn disable_target(
    state: State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    id: Path<String>,
) -> Response {
    set_enabled(state, ctx, headers, id, false).await
}

async fn set_enabled(
    State(state): State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    Path(id): Path<String>,
    enabled: bool,
) -> Response {
    let Ok(uuid) = id.parse::<Uuid>() else {
        return not_found(ctx).await;
    };
    match state.store.set_target_enabled(uuid, enabled).await {
        Ok(()) => {}
        Err(StoreError::NotFound(_)) => return not_found(ctx).await,
        Err(error) => return server_error(ctx, &error),
    }
    if !is_htmx(&headers) {
        return Redirect::to(&ctx.url("/notifications")).into_response();
    }
    row_response(&state, ctx, &id, None).await
}

/// `GET /notifications/{id}/delete`: ask first, in place of the row.
pub(crate) async fn confirm_delete_target(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> Response {
    let Ok(uuid) = id.parse::<Uuid>() else {
        return not_found(ctx).await;
    };
    match row_of(&state, &ctx, uuid).await {
        Ok(row) => render(&DeleteFragment { ctx, row }),
        Err(StoreError::NotFound(_)) => not_found(ctx).await,
        Err(error) => server_error(ctx, &error),
    }
}

/// `POST /notifications/{id}/delete`.
pub(crate) async fn delete_target(
    State(state): State<AppState>,
    ctx: Ctx,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let Ok(uuid) = id.parse::<Uuid>() else {
        return not_found(ctx).await;
    };
    match state.store.delete_target(uuid).await {
        // Already gone is what was asked for.
        Ok(()) | Err(StoreError::NotFound(_)) => {}
        Err(error) => return server_error(ctx, &error),
    }
    if is_htmx(&headers) {
        StatusCode::OK.into_response()
    } else {
        Redirect::to(&ctx.url("/notifications")).into_response()
    }
}
