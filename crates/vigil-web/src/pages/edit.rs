//! Adding a watch and changing one.

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use vigil_core::WatchId;
use vigil_notify::Channel;
use vigil_store::{NewWatch, NotificationTarget, StoreError, Tag, WatchLinks};

use super::{not_found, parse_id, server_error, tag_class};
use crate::AppState;
use crate::form::{self, Built, Draft, Fields, Known, Problems, STEP_KINDS, StepOp};
use crate::page::{Ctx, Nav, render, render_with};

/// A tag or a target that can be chosen on the form.
struct Choice {
    id: String,
    name: String,
    /// Colour class of a tag; empty for a target.
    class: &'static str,
    /// What kind of thing it is, for a target ("ntfy", "email").
    kind: String,
    checked: bool,
}

/// One filter, ready to show.
struct StepView {
    index: usize,
    label: String,
    draft: form::StepDraft,
    error: Option<String>,
    first: bool,
    last: bool,
}

/// A filter kind that can be added.
struct KindOption {
    kind: &'static str,
    label: String,
}

#[derive(Template)]
#[template(path = "watch_form.html")]
struct FormPage {
    ctx: Ctx,
    title_key: &'static str,
    action: String,
    cancel_url: String,
    draft: Draft,
    problems: Problems,
    steps: Vec<StepView>,
    kinds: Vec<KindOption>,
    tags: Vec<Choice>,
    targets: Vec<Choice>,
    editing: bool,
    /// The watch being edited, for the preview to compare with; empty for a new one.
    watch_id: String,
}

#[derive(Template)]
#[template(path = "watch_steps.html")]
struct StepsFragment {
    ctx: Ctx,
    steps: Vec<StepView>,
}

fn kind_options(ctx: &Ctx) -> Vec<KindOption> {
    STEP_KINDS
        .iter()
        .map(|(kind, label)| KindOption {
            kind,
            label: ctx.t(label),
        })
        .collect()
}

fn step_views(ctx: &Ctx, draft: &Draft, problems: &Problems) -> Vec<StepView> {
    let count = draft.steps.len();
    draft
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| StepView {
            index,
            label: STEP_KINDS
                .iter()
                .find(|(kind, _)| *kind == step.kind)
                .map_or_else(|| step.kind.clone(), |(_, label)| ctx.t(label)),
            draft: step.clone(),
            error: problems.step(index).map(str::to_owned),
            first: index == 0,
            last: index + 1 == count,
        })
        .collect()
}

fn tag_choices(tags: &[Tag], draft: &Draft) -> Vec<Choice> {
    tags.iter()
        .map(|tag| Choice {
            id: tag.id.to_string(),
            name: tag.name.clone(),
            class: tag_class(&tag.color),
            kind: String::new(),
            checked: draft.tags.contains(&tag.id.to_string()),
        })
        .collect()
}

fn target_choices(targets: &[NotificationTarget], draft: &Draft) -> Vec<Choice> {
    targets
        .iter()
        .map(|target| Choice {
            id: target.id.to_string(),
            name: target.name.clone(),
            class: "",
            // Only the kind of channel, never the address: it holds credentials.
            kind: Channel::parse(&target.url)
                .map(|channel| channel.kind().to_owned())
                .unwrap_or_default(),
            checked: draft.targets.contains(&target.id.to_string()),
        })
        .collect()
}

/// Everything the form can choose from.
pub(super) struct Options {
    pub(super) tags: Vec<Tag>,
    pub(super) targets: Vec<NotificationTarget>,
}

pub(super) async fn options(state: &AppState) -> Result<Options, StoreError> {
    Ok(Options {
        tags: state.store.list_tags().await?,
        targets: state.store.list_targets().await?,
    })
}

fn page(
    ctx: Ctx,
    options: &Options,
    draft: Draft,
    problems: Problems,
    action: String,
    editing: Option<WatchId>,
) -> FormPage {
    FormPage {
        title_key: if editing.is_some() {
            "form.edit_title"
        } else {
            "form.add_title"
        },
        watch_id: editing.map(|id| id.to_string()).unwrap_or_default(),
        cancel_url: ctx.url("/"),
        steps: step_views(&ctx, &draft, &problems),
        kinds: kind_options(&ctx),
        tags: tag_choices(&options.tags, &draft),
        targets: target_choices(&options.targets, &draft),
        ctx: ctx.nav(Nav::Watches),
        action,
        draft,
        problems,
        editing: editing.is_some(),
    }
}

/// `GET /watch/new`.
pub(crate) async fn new_form(State(state): State<AppState>, ctx: Ctx) -> Response {
    let options = match options(&state).await {
        Ok(options) => options,
        Err(error) => return server_error(ctx, &error),
    };
    let draft = Draft::blank(form::new_draft_id());
    let action = ctx.url("/watch/new");
    render(&page(
        ctx,
        &options,
        draft,
        Problems::default(),
        action,
        None,
    ))
}

/// `POST /watch/new`.
pub(crate) async fn create(State(state): State<AppState>, ctx: Ctx, body: String) -> Response {
    let options = match options(&state).await {
        Ok(options) => options,
        Err(error) => return server_error(ctx, &error),
    };
    let draft = Draft::from_fields(&Fields::parse(&body));
    let built = match check(&state, &ctx, &options, &draft).await {
        Ok(built) => built,
        Err(problems) => {
            let action = ctx.url("/watch/new");
            return render_with(
                StatusCode::UNPROCESSABLE_ENTITY,
                &page(ctx, &options, draft, *problems, action, None),
            );
        }
    };
    let watch = match state
        .store
        .create_watch_with_links(&built.new, false, &links_of(&built))
        .await
    {
        Ok(watch) => watch,
        Err(error) => return server_error(ctx, &error),
    };
    if let Err(error) = state.scheduler.reload().await {
        tracing::warn!(%error, "the scheduler could not be told about the new watch");
    }
    // The first check starts at once, so that the list soon shows something. Nobody waits for
    // it: the ticket is dropped and the check runs anyway.
    if let Err(error) = state.scheduler.check_now(watch.id).await {
        tracing::warn!(%error, "the first check could not be asked for");
    }
    Redirect::to(&ctx.url("/")).into_response()
}

/// `GET /watch/{id}/edit`.
pub(crate) async fn edit_form(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
) -> Response {
    let Some(id) = parse_id(&id) else {
        return not_found(ctx).await;
    };
    let loaded = async {
        let watch = state.store.get_watch(id).await?;
        let tags = state.store.tags_for_watch(id).await?;
        let linked = state.store.targets_for_watch(id, false).await?;
        Ok::<_, StoreError>((watch, tags, linked, options(&state).await?))
    }
    .await;
    let (watch, tags, linked, options) = match loaded {
        Ok(loaded) => loaded,
        Err(StoreError::NotFound(_)) => return not_found(ctx).await,
        Err(error) => return server_error(ctx, &error),
    };
    let draft = Draft::from_watch(form::new_draft_id(), &watch, &tags, &linked);
    let action = ctx.url(&format!("/watch/{id}/edit"));
    render(&page(
        ctx,
        &options,
        draft,
        Problems::default(),
        action,
        Some(id),
    ))
}

/// `POST /watch/{id}/edit`.
pub(crate) async fn update(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<String>,
    body: String,
) -> Response {
    let Some(id) = parse_id(&id) else {
        return not_found(ctx).await;
    };
    let current = match state.store.get_watch(id).await {
        Ok(watch) => watch,
        Err(StoreError::NotFound(_)) => return not_found(ctx).await,
        Err(error) => return server_error(ctx, &error),
    };
    let options = match options(&state).await {
        Ok(options) => options,
        Err(error) => return server_error(ctx, &error),
    };
    let draft = Draft::from_fields(&Fields::parse(&body));
    let built = match check(&state, &ctx, &options, &draft).await {
        Ok(built) => built,
        Err(problems) => {
            let action = ctx.url(&format!("/watch/{id}/edit"));
            return render_with(
                StatusCode::UNPROCESSABLE_ENTITY,
                &page(ctx, &options, draft, *problems, action, Some(id)),
            );
        }
    };
    // Like `vigil watch edit`: the settings of the check are written only when they changed,
    // because writing them forgets the page's ETag and the failure streak and moves the
    // schedule. Tags, targets and the template are not part of that. All of it is saved or
    // none.
    let settings = (NewWatch::from(&current) != built.new).then_some(&built.new);
    if let Err(error) = state
        .store
        .replace_watch(id, settings, None, &links_of(&built))
        .await
    {
        return server_error(ctx, &error);
    }
    if let Err(error) = state.scheduler.reload().await {
        tracing::warn!(%error, "the scheduler could not be told about the change");
    }
    Redirect::to(&ctx.url("/")).into_response()
}

async fn check(
    state: &AppState,
    ctx: &Ctx,
    options: &Options,
    draft: &Draft,
) -> Result<Built, Box<Problems>> {
    let known = Known {
        tags: &options.tags,
        targets: &options.targets,
    };
    // The form's processing slot is used one request at a time: a Save that comes while a
    // preview of the same form is still working waits for it.
    let _turn = state.slots.hold(form::slot_of(&draft.id)).await;
    form::build(draft, ctx.translator(), &known, &state.checker).await
}

/// The tags, targets and template the form says.
fn links_of(built: &Built) -> WatchLinks<'_> {
    WatchLinks {
        tags: &built.tags,
        targets: &built.targets,
        template: built.template.as_deref(),
    }
}

/// What a button next to the filters asked for.
#[derive(Debug, Deserialize)]
pub(crate) struct StepQuery {
    op: String,
    #[serde(default)]
    i: usize,
}

/// `POST /watch/steps`: the list of filters after a button was pressed, from the form as it
/// is now. Nothing is saved.
pub(crate) async fn steps(ctx: Ctx, Query(query): Query<StepQuery>, body: String) -> Response {
    let fields = Fields::parse(&body);
    let mut draft = Draft::from_fields(&fields);
    let op = match query.op.as_str() {
        "add" => StepOp::Add(fields.get("new_step_type").to_owned()),
        "remove" => StepOp::Remove(query.i),
        "up" => StepOp::Up(query.i),
        "down" => StepOp::Down(query.i),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    draft.apply(&op);
    let steps = step_views(&ctx, &draft, &Problems::default());
    let mut response = render(&StepsFragment { ctx, steps });
    // The preview listens for this, since the filters changed without an input event.
    response
        .headers_mut()
        .insert("hx-trigger", HeaderValue::from_static("stepschanged"));
    response
}
