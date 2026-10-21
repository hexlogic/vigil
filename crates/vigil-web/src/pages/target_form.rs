//! Adding a notification target and changing one.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use uuid::Uuid;
use vigil_notify::{Channel, validate_template};
use vigil_store::{NewTarget, NotificationTarget, StoreError};

use super::{not_found, server_error};
use crate::AppState;
use crate::form::Fields;
use crate::page::{Ctx, Nav, render, render_with};

/// Longest name of a target.
pub(crate) const NAME_LEN: usize = 100;

/// What was typed.
#[derive(Default)]
struct Draft {
    name: String,
    url: String,
    template: String,
}

/// What is wrong with it.
#[derive(Default)]
struct Problems {
    name: Option<String>,
    url: Option<String>,
    template: Option<String>,
}

#[derive(Template)]
#[template(path = "target_form.html")]
struct FormPage {
    ctx: Ctx,
    title_key: &'static str,
    action: String,
    cancel_url: String,
    draft: Draft,
    problems: Problems,
    editing: bool,
    /// The current address with its secrets hidden, shown where the field is empty.
    current: String,
}

fn page(
    ctx: Ctx,
    draft: Draft,
    problems: Problems,
    action: String,
    current: Option<&NotificationTarget>,
) -> FormPage {
    let t = ctx.translator().clone();
    FormPage {
        title_key: if current.is_some() {
            "targets.edit_title"
        } else {
            "targets.add_title"
        },
        cancel_url: ctx.url("/notifications"),
        editing: current.is_some(),
        current: current
            .map(|target| {
                Channel::parse(&target.url)
                    .map(|channel| channel.redacted())
                    .unwrap_or_else(|_| t.t("targets.invalid_short"))
            })
            .unwrap_or_default(),
        ctx: ctx.nav(Nav::Notifications),
        draft,
        problems,
        action,
    }
}

/// `GET /notifications/new`.
pub(crate) async fn new_form(ctx: Ctx) -> Response {
    let action = ctx.url("/notifications/new");
    render(&page(
        ctx,
        Draft::default(),
        Problems::default(),
        action,
        None,
    ))
}

/// `GET /notifications/{id}/edit`.
pub(crate) async fn edit_form(
    State(state): State<AppState>,
    ctx: Ctx,
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
    // The address is not put in the form: it holds credentials. An empty field keeps it.
    let draft = Draft {
        name: target.name.clone(),
        url: String::new(),
        template: target.default_template.clone().unwrap_or_default(),
    };
    let action = ctx.url(&format!("/notifications/{id}/edit"));
    render(&page(
        ctx,
        draft,
        Problems::default(),
        action,
        Some(&target),
    ))
}

/// Check what was typed. `keep` is the target being edited: an empty address then keeps its.
fn check(
    ctx: &Ctx,
    draft: &Draft,
    keep: Option<&NotificationTarget>,
) -> Result<NewTarget, Problems> {
    let t = ctx.translator();
    let mut problems = Problems::default();
    let name = draft.name.trim();
    if name.is_empty() {
        problems.name = Some(t.t("targets.err.name_required"));
    } else if name.chars().count() > NAME_LEN {
        problems.name = Some(t.t1("targets.err.name_long", "n", NAME_LEN));
    }
    let url = match (draft.url.trim(), keep) {
        ("", Some(target)) => target.url.clone(),
        ("", None) => {
            problems.url = Some(t.t("targets.err.url_required"));
            String::new()
        }
        (url, _) => {
            // The message names what is wrong and never repeats the address.
            if let Err(error) = Channel::parse(url) {
                problems.url = Some(error.to_string());
            }
            url.to_owned()
        }
    };
    let template = match draft.template.trim() {
        "" => None,
        text => {
            if let Err(error) = validate_template(text) {
                problems.template = Some(error.to_string());
            }
            Some(text.to_owned())
        }
    };
    if problems.name.is_some() || problems.url.is_some() || problems.template.is_some() {
        return Err(problems);
    }
    Ok(NewTarget {
        name: name.to_owned(),
        url,
        default_template: template,
    })
}

fn draft_of(body: &str) -> Draft {
    let fields = Fields::parse(body);
    Draft {
        name: fields.get("name").to_owned(),
        url: fields.get("url").to_owned(),
        template: fields.get("template").to_owned(),
    }
}

/// A name that is taken is the name field's problem.
fn taken(ctx: &Ctx) -> Problems {
    Problems {
        name: Some(ctx.t("targets.err.name_taken")),
        ..Problems::default()
    }
}

/// `POST /notifications/new`.
pub(crate) async fn create(State(state): State<AppState>, ctx: Ctx, body: String) -> Response {
    let draft = draft_of(&body);
    let action = ctx.url("/notifications/new");
    let new = match check(&ctx, &draft, None) {
        Ok(new) => new,
        Err(problems) => {
            return render_with(
                StatusCode::UNPROCESSABLE_ENTITY,
                &page(ctx, draft, problems, action, None),
            );
        }
    };
    match state.store.create_target(&new).await {
        Ok(_) => Redirect::to(&ctx.url("/notifications")).into_response(),
        Err(StoreError::Conflict(_)) => {
            let problems = taken(&ctx);
            render_with(
                StatusCode::UNPROCESSABLE_ENTITY,
                &page(ctx, draft, problems, action, None),
            )
        }
        Err(error) => server_error(ctx, &error),
    }
}

/// `POST /notifications/{id}/edit`.
pub(crate) async fn update(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    body: String,
) -> Response {
    let Ok(uuid) = id.parse::<Uuid>() else {
        return not_found(ctx).await;
    };
    let current = match state.store.get_target(uuid).await {
        Ok(target) => target,
        Err(StoreError::NotFound(_)) => return not_found(ctx).await,
        Err(error) => return server_error(ctx, &error),
    };
    let draft = draft_of(&body);
    let action = ctx.url(&format!("/notifications/{id}/edit"));
    let new = match check(&ctx, &draft, Some(&current)) {
        Ok(new) => new,
        Err(problems) => {
            return render_with(
                StatusCode::UNPROCESSABLE_ENTITY,
                &page(ctx, draft, problems, action, Some(&current)),
            );
        }
    };
    match state.store.update_target(uuid, &new).await {
        Ok(_) => Redirect::to(&ctx.url("/notifications")).into_response(),
        Err(StoreError::Conflict(_)) => {
            let problems = taken(&ctx);
            render_with(
                StatusCode::UNPROCESSABLE_ENTITY,
                &page(ctx, draft, problems, action, Some(&current)),
            )
        }
        Err(StoreError::NotFound(_)) => not_found(ctx).await,
        Err(error) => server_error(ctx, &error),
    }
}
