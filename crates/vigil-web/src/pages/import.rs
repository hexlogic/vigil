//! Importing watches from the browser: choose a file, read the preview, confirm.

use std::time::{Duration, Instant};

use askama::Template;
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::Response;
use vigil_import::report::{ItemReport, Note, NoteKind, Status};
use vigil_import::{Existing, Format, ImportError, Options, plan, plan_reader};
use vigil_scheduler::{IntervalError, parse_interval};

use super::server_error;
use crate::AppState;
use crate::fmt;
use crate::form::{Fields, new_draft_id};
use crate::page::{Ctx, Nav, render, render_with};
use crate::upload::{Upload, UploadError};

/// The interval of sources that have none, when the form does not say.
const DEFAULT_EVERY: &str = "1h";

struct FormatOption {
    value: &'static str,
    label: String,
    selected: bool,
}

#[derive(Template)]
#[template(path = "import.html")]
struct ImportPage {
    ctx: Ctx,
    action: String,
    error: Option<String>,
    urls: String,
    every: String,
    formats: Vec<FormatOption>,
    allow_duplicates: bool,
    limit: String,
}

struct NoteView {
    kind: &'static str,
    label: String,
    option: String,
    message: String,
}

struct ItemView {
    source: String,
    status_class: &'static str,
    status_label: String,
    notes: Vec<NoteView>,
}

#[derive(Template)]
#[template(path = "import_report.html")]
struct ReportPage {
    ctx: Ctx,
    format: String,
    summary: Vec<String>,
    notes: Vec<NoteView>,
    items: Vec<ItemView>,
    plan: String,
    apply_url: String,
    back_url: String,
    apply_label: String,
    can_apply: bool,
}

#[derive(Template)]
#[template(path = "import_done.html")]
struct DonePage {
    ctx: Ctx,
    made: String,
    watches_url: String,
}

fn formats(ctx: &Ctx, selected: &str) -> Vec<FormatOption> {
    let auto = FormatOption {
        value: "",
        label: ctx.t("import.format_auto"),
        selected: selected.is_empty(),
    };
    std::iter::once(auto)
        .chain(Format::ALL.into_iter().map(|format| FormatOption {
            value: format.as_str(),
            label: ctx.t(&format!("import.format.{}", format.as_str())),
            selected: format.as_str() == selected,
        }))
        .collect()
}

fn form_page(state: &AppState, ctx: Ctx, error: Option<String>, typed: &Typed) -> ImportPage {
    ImportPage {
        action: ctx.url("/import"),
        error,
        urls: typed.urls.clone(),
        every: typed.every.clone(),
        formats: formats(&ctx, &typed.format),
        allow_duplicates: typed.allow_duplicates,
        limit: fmt::bytes(state.web.max_upload_bytes),
        ctx: ctx.nav(Nav::Import),
    }
}

/// What was typed in the form.
#[derive(Default)]
struct Typed {
    urls: String,
    format: String,
    every: String,
    allow_duplicates: bool,
}

/// `GET /import`.
pub(crate) async fn import_form(State(state): State<AppState>, ctx: Ctx) -> Response {
    let typed = Typed {
        every: DEFAULT_EVERY.to_owned(),
        ..Typed::default()
    };
    render(&form_page(&state, ctx, None, &typed))
}

fn notes_of(ctx: &Ctx, notes: &[Note]) -> Vec<NoteView> {
    notes
        .iter()
        .map(|note| {
            let (kind, key) = match note.kind {
                NoteKind::Skipped => ("error", "import.note.skipped"),
                NoteKind::Approximated => ("muted", "import.note.approximated"),
                NoteKind::Info => ("muted", "import.note.info"),
            };
            NoteView {
                kind,
                label: ctx.t(key),
                option: note.option.clone(),
                message: note.message.clone(),
            }
        })
        .collect()
}

fn item_of(ctx: &Ctx, item: &ItemReport) -> ItemView {
    let (class, key) = match item.status {
        Status::Created => ("badge badge-ok", "import.status.created"),
        Status::Partial => ("badge", "import.status.partial"),
        Status::Skipped => ("badge", "import.status.skipped"),
    };
    ItemView {
        source: item.source.clone(),
        status_class: class,
        status_label: ctx.t(key),
        notes: notes_of(ctx, &item.notes),
    }
}

/// The fields of the form, and the file or pasted text it holds.
struct Received {
    typed: Typed,
    file: Option<crate::upload::Spooled>,
}

enum Problem {
    Message(String),
    TooLarge,
    Server(String),
}

async fn receive(state: &AppState, mut multipart: Multipart) -> Result<Received, Problem> {
    let mut typed = Typed::default();
    let mut file = None;
    loop {
        let mut field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => {
                // A body over the limit, or one that was cut off.
                return Err(if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    Problem::TooLarge
                } else {
                    Problem::Message(error.body_text())
                });
            }
        };
        match field.name().unwrap_or_default() {
            "file" => {
                let mut upload = Upload::new(&state.web.upload_dir, state.web.max_upload_bytes)
                    .map_err(|e| Problem::Server(e.to_string()))?;
                loop {
                    match field.chunk().await {
                        Ok(Some(chunk)) => match upload.push(&chunk).await {
                            Ok(()) => {}
                            Err(UploadError::TooLarge { .. }) => return Err(Problem::TooLarge),
                            Err(UploadError::Io(error)) => {
                                return Err(Problem::Server(error.to_string()));
                            }
                        },
                        Ok(None) => break,
                        Err(error) => {
                            return Err(if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                                Problem::TooLarge
                            } else {
                                Problem::Message(error.body_text())
                            });
                        }
                    }
                }
                let spooled = upload
                    .finish()
                    .await
                    .map_err(|e| Problem::Server(e.to_string()))?;
                // A browser sends an empty file part when none was chosen.
                if spooled.len > 0 {
                    file = Some(spooled);
                }
            }
            name => {
                let name = name.to_owned();
                let text = field
                    .text()
                    .await
                    .map_err(|e| Problem::Message(e.body_text()))?;
                match name.as_str() {
                    "urls" => typed.urls = text,
                    "format" => typed.format = text.trim().to_owned(),
                    "every" => typed.every = text.trim().to_owned(),
                    "allow_duplicates" => typed.allow_duplicates = true,
                    _ => {}
                }
            }
        }
    }
    Ok(Received { typed, file })
}

/// `POST /import`: read the file, show what would be made.
pub(crate) async fn import_preview(
    State(state): State<AppState>,
    ctx: Ctx,
    multipart: Multipart,
) -> Response {
    let t = ctx.translator().clone();
    let again = |ctx: Ctx, status: StatusCode, message: String, typed: &Typed| {
        render_with(status, &form_page(&state, ctx, Some(message), typed))
    };
    let received = match receive(&state, multipart).await {
        Ok(received) => received,
        Err(Problem::TooLarge) => {
            return again(
                ctx.clone(),
                StatusCode::PAYLOAD_TOO_LARGE,
                t.t1(
                    "import.err.too_large",
                    "limit",
                    fmt::bytes(state.web.max_upload_bytes),
                ),
                &Typed {
                    every: DEFAULT_EVERY.to_owned(),
                    ..Typed::default()
                },
            );
        }
        Err(Problem::Message(message)) => {
            return again(ctx, StatusCode::BAD_REQUEST, message, &Typed::default());
        }
        Err(Problem::Server(message)) => return server_error(ctx, &message),
    };
    let Received { typed, file } = received;
    let invalid = |message: String| (StatusCode::UNPROCESSABLE_ENTITY, message);

    let format = match typed.format.as_str() {
        "" => None,
        text => match text.parse::<Format>() {
            Ok(format) => Some(format),
            Err(_) => {
                let (status, message) = invalid(t.t("import.err.format"));
                return again(ctx, status, message, &typed);
            }
        },
    };
    let every = match typed.every.as_str() {
        "" => Duration::from_secs(3600),
        text => match parse_interval(text) {
            Ok(every) => every,
            Err(error) => {
                let (status, message) = invalid(t.t(match error {
                    IntervalError::Invalid(_) => "form.err.every_invalid",
                    IntervalError::TooShort => "form.err.every_short",
                    IntervalError::TooLong => "form.err.every_long",
                }));
                return again(ctx, status, message, &typed);
            }
        },
    };
    if file.is_none() && typed.urls.trim().is_empty() {
        let (status, message) = invalid(t.t("import.err.nothing"));
        return again(ctx, status, message, &typed);
    }

    let existing = match async {
        Ok::<_, vigil_store::StoreError>(Existing::new(
            &state.store.list_tags().await?,
            &state.store.list_targets().await?,
            &state.store.list_watches().await?,
        ))
    }
    .await
    {
        Ok(existing) => existing,
        Err(error) => return server_error(ctx, &error),
    };
    let options = Options {
        every,
        allow_duplicates: typed.allow_duplicates,
    };
    let pasted = typed.urls.clone().into_bytes();
    let planned = tokio::task::spawn_blocking(move || match file {
        Some(file) => file
            .reader()
            .map_err(|error| ImportError::Invalid {
                format: Format::UrlList,
                reason: error.to_string(),
            })
            .and_then(|reader| plan_reader(reader, file.len, format, &options, &existing)),
        None => plan(&pasted, format, &options, &existing),
    })
    .await;
    let planned = match planned {
        Ok(planned) => planned,
        Err(error) => return server_error(ctx, &error),
    };
    let planned = match planned {
        Ok(planned) => planned,
        Err(error) => {
            let status = match error {
                ImportError::TooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
                _ => StatusCode::UNPROCESSABLE_ENTITY,
            };
            return again(ctx, status, error.to_string(), &typed);
        }
    };

    let totals = planned.report.totals;
    let page = ReportPage {
        format: t.t(&format!("import.format.{}", planned.report.format.as_str())),
        summary: vec![
            t.t1("import.found", "n", totals.found),
            t.t1("import.will_make", "n", totals.created),
            t.t1("import.skipped_count", "n", totals.skipped),
            t.t2(
                "import.tags_targets",
                "tags",
                totals.tags_created,
                "targets",
                totals.targets_created,
            ),
        ],
        notes: notes_of(&ctx, &planned.report.notes),
        items: planned
            .report
            .items
            .iter()
            .map(|item| item_of(&ctx, item))
            .collect(),
        apply_label: ctx.plural("import.apply", totals.created),
        can_apply: totals.created > 0,
        apply_url: ctx.url("/import/apply"),
        back_url: ctx.url("/import"),
        plan: new_draft_id(),
        ctx: ctx.nav(Nav::Import),
    };
    state
        .import_plans
        .put(page.plan.clone(), planned, Instant::now());
    render(&page)
}

/// `POST /import/apply`: make what the preview showed.
pub(crate) async fn import_apply(
    State(state): State<AppState>,
    ctx: Ctx,
    body: String,
) -> Response {
    let fields = Fields::parse(&body);
    let Some(plan) = state.import_plans.take(fields.get("plan"), Instant::now()) else {
        // The preview is gone (it waited too long, or the button was pressed twice).
        let typed = Typed {
            every: DEFAULT_EVERY.to_owned(),
            ..Typed::default()
        };
        let message = ctx.t("import.expired");
        return render_with(
            StatusCode::GONE,
            &form_page(&state, ctx, Some(message), &typed),
        );
    };
    let outcome = match state.store.apply_import(&plan.batch).await {
        Ok(outcome) => outcome,
        Err(error) => return server_error(ctx, &error),
    };
    if let Err(error) = state.scheduler.reload().await {
        tracing::warn!(%error, "the scheduler could not be told about the imported watches");
    }
    let made = outcome.watches.iter().flatten().count();
    let t = ctx.translator().clone();
    render(&DonePage {
        made: t.t2(
            "import.done",
            "watches",
            made,
            "more",
            t.t2(
                "import.done_more",
                "tags",
                outcome.tags_created,
                "targets",
                outcome.targets_created,
            ),
        ),
        watches_url: ctx.url("/"),
        ctx: ctx.nav(Nav::Import),
    })
}
