//! Adding a tag and changing one.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use uuid::Uuid;
use vigil_scheduler::{IntervalError, parse_interval};
use vigil_store::{NewTag, NotificationTarget, StoreError, Tag};

use super::{TAG_COLORS, not_found, server_error};
use crate::AppState;
use crate::fmt;
use crate::form::Fields;
use crate::page::{Ctx, Nav, render, render_with};

/// Longest name of a tag.
const NAME_LEN: usize = 50;

/// What was typed.
struct Draft {
    name: String,
    color: String,
    every: String,
    targets: Vec<String>,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            name: String::new(),
            color: "gray".to_owned(),
            every: String::new(),
            targets: Vec::new(),
        }
    }
}

#[derive(Default)]
struct Problems {
    name: Option<String>,
    every: Option<String>,
    targets: Option<String>,
}

struct Swatch {
    value: &'static str,
    label: String,
    checked: bool,
}

struct TargetChoice {
    id: String,
    name: String,
    checked: bool,
}

#[derive(Template)]
#[template(path = "tag_form.html")]
struct FormPage {
    ctx: Ctx,
    title_key: &'static str,
    action: String,
    cancel_url: String,
    draft: Draft,
    problems: Problems,
    swatches: Vec<Swatch>,
    targets: Vec<TargetChoice>,
    editing: bool,
}

fn page(
    ctx: Ctx,
    draft: Draft,
    problems: Problems,
    action: String,
    editing: bool,
    targets: &[NotificationTarget],
) -> FormPage {
    FormPage {
        title_key: if editing {
            "tags.edit_title"
        } else {
            "tags.add_title"
        },
        cancel_url: ctx.url("/tags"),
        swatches: TAG_COLORS
            .iter()
            .map(|&value| Swatch {
                value,
                label: ctx.t(&format!("tags.color.{value}")),
                checked: draft.color == value,
            })
            .collect(),
        targets: targets
            .iter()
            .map(|target| TargetChoice {
                id: target.id.to_string(),
                name: target.name.clone(),
                checked: draft.targets.contains(&target.id.to_string()),
            })
            .collect(),
        ctx: ctx.nav(Nav::Tags),
        draft,
        problems,
        action,
        editing,
    }
}

/// `GET /tags/new`.
pub(crate) async fn new_form(State(state): State<AppState>, ctx: Ctx) -> Response {
    let targets = match state.store.list_targets().await {
        Ok(targets) => targets,
        Err(error) => return server_error(ctx, &error),
    };
    let action = ctx.url("/tags/new");
    render(&page(
        ctx,
        Draft::default(),
        Problems::default(),
        action,
        false,
        &targets,
    ))
}

/// `GET /tags/{id}/edit`.
pub(crate) async fn edit_form(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> Response {
    let Ok(uuid) = id.parse::<Uuid>() else {
        return not_found(ctx).await;
    };
    let loaded = async {
        let tag = state.store.get_tag(uuid).await?;
        let chosen = state.store.tag_targets(uuid).await?;
        let targets = state.store.list_targets().await?;
        Ok::<_, StoreError>((tag, chosen, targets))
    }
    .await;
    let (tag, chosen, targets) = match loaded {
        Ok(loaded) => loaded,
        Err(StoreError::NotFound(_)) => return not_found(ctx).await,
        Err(error) => return server_error(ctx, &error),
    };
    let draft = Draft {
        name: tag.name,
        color: tag.color,
        every: tag
            .default_interval_secs
            .map(fmt::every)
            .unwrap_or_default(),
        targets: chosen.iter().map(Uuid::to_string).collect(),
    };
    let action = ctx.url(&format!("/tags/{id}/edit"));
    render(&page(
        ctx,
        draft,
        Problems::default(),
        action,
        true,
        &targets,
    ))
}

fn draft_of(body: &str) -> Draft {
    let fields = Fields::parse(body);
    Draft {
        name: fields.get("name").to_owned(),
        color: fields.get("color").to_owned(),
        every: fields.get("every").to_owned(),
        targets: fields.all("target"),
    }
}

/// Check what was typed.
fn check(
    ctx: &Ctx,
    draft: &Draft,
    known: &[NotificationTarget],
) -> Result<(NewTag, Vec<Uuid>), Problems> {
    let t = ctx.translator();
    let mut problems = Problems::default();
    let name = draft.name.trim();
    if name.is_empty() {
        problems.name = Some(t.t("tags.err.name_required"));
    } else if name.chars().count() > NAME_LEN {
        problems.name = Some(t.t1("tags.err.name_long", "n", NAME_LEN));
    }
    let every = match draft.every.trim() {
        "" => None,
        text => match parse_interval(text) {
            Ok(every) => Some(every.as_secs()),
            Err(error) => {
                problems.every = Some(t.t(match error {
                    IntervalError::Invalid(_) => "form.err.every_invalid",
                    IntervalError::TooShort => "form.err.every_short",
                    IntervalError::TooLong => "form.err.every_long",
                }));
                None
            }
        },
    };
    let mut targets = Vec::new();
    for text in &draft.targets {
        match text.parse::<Uuid>() {
            Ok(id) if known.iter().any(|target| target.id == id) => {
                if !targets.contains(&id) {
                    targets.push(id);
                }
            }
            _ => problems.targets = Some(t.t("form.err.unknown_choice")),
        }
    }
    if problems.name.is_some() || problems.every.is_some() || problems.targets.is_some() {
        return Err(problems);
    }
    // A colour outside the palette is the default one: the page policy allows no free choice.
    let color = TAG_COLORS
        .iter()
        .find(|&&known| known == draft.color)
        .map_or("gray", |&color| color);
    Ok((
        NewTag {
            name: name.to_owned(),
            color: color.to_owned(),
            default_interval_secs: every,
        },
        targets,
    ))
}

fn taken(ctx: &Ctx) -> Problems {
    Problems {
        name: Some(ctx.t("tags.err.name_taken")),
        ..Problems::default()
    }
}

/// `POST /tags/new`.
pub(crate) async fn create(State(state): State<AppState>, ctx: Ctx, body: String) -> Response {
    let targets = match state.store.list_targets().await {
        Ok(targets) => targets,
        Err(error) => return server_error(ctx, &error),
    };
    let draft = draft_of(&body);
    let action = ctx.url("/tags/new");
    let (new, chosen) = match check(&ctx, &draft, &targets) {
        Ok(checked) => checked,
        Err(problems) => {
            return render_with(
                StatusCode::UNPROCESSABLE_ENTITY,
                &page(ctx, draft, problems, action, false, &targets),
            );
        }
    };
    let saved = async {
        let tag: Tag = state.store.create_tag(&new).await?;
        state.store.set_tag_targets(tag.id, &chosen).await
    }
    .await;
    match saved {
        Ok(()) => Redirect::to(&ctx.url("/tags")).into_response(),
        Err(StoreError::Conflict(_)) => {
            let problems = taken(&ctx);
            render_with(
                StatusCode::UNPROCESSABLE_ENTITY,
                &page(ctx, draft, problems, action, false, &targets),
            )
        }
        Err(error) => server_error(ctx, &error),
    }
}

/// `POST /tags/{id}/edit`.
pub(crate) async fn update(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    body: String,
) -> Response {
    let Ok(uuid) = id.parse::<Uuid>() else {
        return not_found(ctx).await;
    };
    let targets = match state.store.list_targets().await {
        Ok(targets) => targets,
        Err(error) => return server_error(ctx, &error),
    };
    let draft = draft_of(&body);
    let action = ctx.url(&format!("/tags/{id}/edit"));
    let (new, chosen) = match check(&ctx, &draft, &targets) {
        Ok(checked) => checked,
        Err(problems) => {
            return render_with(
                StatusCode::UNPROCESSABLE_ENTITY,
                &page(ctx, draft, problems, action, true, &targets),
            );
        }
    };
    let saved = async {
        state.store.update_tag(uuid, &new).await?;
        state.store.set_tag_targets(uuid, &chosen).await
    }
    .await;
    match saved {
        Ok(()) => Redirect::to(&ctx.url("/tags")).into_response(),
        Err(StoreError::Conflict(_)) => {
            let problems = taken(&ctx);
            render_with(
                StatusCode::UNPROCESSABLE_ENTITY,
                &page(ctx, draft, problems, action, true, &targets),
            )
        }
        Err(StoreError::NotFound(_)) => not_found(ctx).await,
        Err(error) => server_error(ctx, &error),
    }
}
