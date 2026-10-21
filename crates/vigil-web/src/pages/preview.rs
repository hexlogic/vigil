//! The live preview beside the watch form: what a check would store with the settings as
//! they are typed.

use std::time::{Duration, Instant};

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Response;
use serde::Deserialize;
use vigil_core::DiffView;
use vigil_store::now_ms;

use super::edit::options;
use super::{parse_id, server_error};
use crate::AppState;
use crate::diff_html::{self, Inline};
use crate::fmt;
use crate::form::{self, Draft, Fields, Known};
use crate::page::{Ctx, render};

/// Characters of the result shown; the rest is counted.
const SHOWN_CHARS: usize = 20_000;

/// Lines of context around a change in the comparison.
const CONTEXT: usize = 2;

/// What the preview shows of a page that was fetched and processed.
struct Outcome {
    /// "HTTP 200 · text/html · 12.3 KiB".
    facts: String,
    /// "fetched in 120 ms, processed in 3 ms", or that the page was reused.
    timing: String,
    text: String,
    /// "Showing the first 20000 of 90000 characters."
    shown: Option<String>,
    lines: String,
    compare: Compare,
}

/// How the result compares with what the watch stored last.
struct Compare {
    /// A sentence when there is nothing to show a difference of.
    note: Option<String>,
    diff: Option<Inline>,
    /// Whether the watch's conditions would notify, with the reason.
    verdict: Option<(bool, String)>,
}

#[derive(Template)]
#[template(path = "preview.html")]
struct Panel {
    ctx: Ctx,
    /// Nothing to preview yet.
    idle: bool,
    /// What to fix before there can be a preview.
    problems: Vec<String>,
    /// The page could not be fetched.
    fetch_error: Option<String>,
    /// The page was fetched but processing it failed.
    failure: Option<String>,
    outcome: Option<Outcome>,
    refetch_url: String,
}

impl Panel {
    fn new(ctx: Ctx) -> Self {
        let refetch_url = ctx.url("/watch/preview?refetch=1");
        Self {
            ctx,
            idle: false,
            problems: Vec::new(),
            fetch_error: None,
            failure: None,
            outcome: None,
            refetch_url,
        }
    }
}

/// Query of the preview.
#[derive(Debug, Deserialize)]
pub(crate) struct PreviewQuery {
    /// Fetch the page again even if the last fetch of this form is still good.
    #[serde(default)]
    refetch: u8,
}

/// `POST /watch/preview`: the preview of the form as it is now. Nothing is saved.
pub(crate) async fn preview(
    State(state): State<AppState>,
    ctx: Ctx,
    Query(query): Query<PreviewQuery>,
    body: String,
) -> Response {
    let fields = Fields::parse(&body);
    let draft = Draft::from_fields(&fields);
    let mut panel = Panel::new(ctx.clone());
    if draft.url.trim().is_empty() {
        panel.idle = true;
        return render(&panel);
    }
    let slot = form::slot_of(&draft.id);
    let t = ctx.translator();

    // What the form describes. The slot's turn is held for the work on the processing thread
    // and not for the fetch, which can take half a minute.
    let options = match options(&state).await {
        Ok(options) => options,
        Err(error) => return server_error(ctx, &error),
    };
    let known = Known {
        tags: &options.tags,
        targets: &options.targets,
    };
    let built = {
        let _turn = state.slots.hold(slot).await;
        form::build_for_preview(&draft, t, &known, &state.checker).await
    };
    let new = match built {
        Ok(new) => new,
        Err(problems) => {
            panel.problems = problems.preview_messages(t);
            return render(&panel);
        }
    };
    // The watch being edited, if any, is only there to compare with its last version.
    let watch = form::preview_watch(&new, parse_id(fields.get("watch")).unwrap_or(slot));

    // The page: the one this form fetched before, unless it was asked to fetch again or the
    // request changed.
    let request = format!("{}\n{}", new.url, serde_json_text(&new));
    let (page, how) = match (query.refetch == 0)
        .then(|| state.previews.get(&draft.id, &request, Instant::now()))
        .flatten()
    {
        Some((page, age)) => (page, Fetched::Reused(age)),
        None => {
            let started = Instant::now();
            match state.checker.fetch_for_preview(&watch).await {
                Ok(page) => {
                    state
                        .previews
                        .put(&draft.id, request, page.clone(), Instant::now());
                    (page, Fetched::Now(started.elapsed()))
                }
                Err(message) => {
                    panel.fetch_error = Some(message);
                    return render(&panel);
                }
            }
        }
    };

    let report = {
        let _turn = state.slots.hold(slot).await;
        state.checker.preview_page(&watch, &page, slot).await
    };
    let report = match report {
        Ok(report) => report,
        Err(error) => return server_error(ctx, &error),
    };
    let preview = match report.result {
        Ok(preview) => preview,
        Err(error) => {
            panel.failure = Some(error.message);
            return render(&panel);
        }
    };

    let editing = parse_id(fields.get("watch")).is_some();
    let compare = compare(&state, t, &preview, editing).await;
    let timing = match how {
        Fetched::Now(took) => t.t2(
            "preview.timing",
            "fetch",
            took.as_millis(),
            "process",
            report.duration.as_millis(),
        ),
        Fetched::Reused(age) => {
            let at = now_ms() - i64::try_from(age.as_millis()).unwrap_or(0);
            t.t2(
                "preview.timing_reused",
                "when",
                fmt::when(t, Some(at), now_ms()).text,
                "process",
                report.duration.as_millis(),
            )
        }
    };
    let total_chars = preview.text.chars().count();
    panel.outcome = Some(Outcome {
        facts: format!(
            "HTTP {} · {} · {}",
            preview.http_status,
            preview.content_type.as_deref().unwrap_or("?"),
            fmt::bytes(preview.raw_size)
        ),
        timing,
        shown: (total_chars > SHOWN_CHARS)
            .then(|| t.t2("preview.shown", "shown", SHOWN_CHARS, "total", total_chars)),
        lines: t.plural("preview.lines", preview.text.lines().count()),
        text: preview.text.chars().take(SHOWN_CHARS).collect(),
        compare,
    });
    render(&panel)
}

/// How the page was got.
enum Fetched {
    Now(Duration),
    Reused(Duration),
}

/// What the request to the site was made of, so that a page is reused only for the same one.
fn serde_json_text(new: &vigil_store::NewWatch) -> String {
    format!(
        "{:?}|{:?}|{:?}",
        new.fetch.timeout_secs, new.fetch.user_agent, new.fetch.headers
    )
}

async fn compare(
    state: &AppState,
    t: &crate::i18n::Translator,
    preview: &vigil_scheduler::Preview,
    editing: bool,
) -> Compare {
    let mut compare = Compare {
        note: None,
        diff: None,
        verdict: None,
    };
    let Some(latest) = &preview.latest else {
        compare.note = Some(t.t(if editing {
            "preview.no_stored_version"
        } else {
            "preview.new_watch"
        }));
        return compare;
    };
    let same = preview.diff.as_ref().is_none_or(|diff| diff.is_empty());
    if same {
        compare.note = Some(t.t("preview.same"));
        return compare;
    }
    match state.store.snapshot_content(latest.id).await {
        Ok(old) => {
            compare.diff = Some(diff_html::inline(
                t,
                &DiffView::between(&old, &preview.text, CONTEXT),
            ));
        }
        Err(error) => tracing::warn!(%error, "reading the last version for a preview failed"),
    }
    compare.verdict = preview
        .verdict
        .as_ref()
        .map(|verdict| (verdict.notify, verdict.reason.to_string()));
    compare
}
