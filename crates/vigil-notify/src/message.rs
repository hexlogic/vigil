//! Turning an event into a message that fits its channel.

use std::sync::Arc;

use crate::builtin::builtin;
use crate::error::TemplateError;
use crate::escape::{Markup, diff_link, escape, link};
use crate::event::Event;
use crate::template::{Vars, render};
use crate::truncate::{Limit, cut_to, shrink};

/// Longest title kept, in characters.
const MAX_TITLE: usize = 200;

/// A message ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// Subject line for channels that have one (email, ntfy, Gotify, webhooks).
    pub title: String,
    /// The message text, in the channel's markup and within its size limit.
    pub body: String,
    /// Whether the diff and other long values were cut to fit.
    pub truncated: bool,
    /// Set when a custom template failed and the built-in one was used instead.
    pub template_note: Option<String>,
}

/// Build the message for `event` in `markup`, within `limit`.
///
/// `template` is the custom template to try first. If it fails to render, the built-in template
/// is used and [`Prepared::template_note`] says why: a notification is worth more than a pretty
/// one. This never fails.
pub fn prepare(markup: Markup, limit: Limit, event: &Event, template: Option<&str>) -> Prepared {
    let mut note = None;
    if let Some(source) = template {
        match fit(markup, limit, source, event) {
            Ok(prepared) => return finish(prepared, None),
            Err(error) => {
                note = Some(format!("custom {error}; sent the built-in one"));
            }
        }
    }
    let prepared = fit(markup, limit, builtin(markup), event)
        .unwrap_or_else(|_| minimal(markup, limit, event));
    finish(prepared, note)
}

fn finish(mut prepared: Prepared, note: Option<String>) -> Prepared {
    let title = shrink(&prepared.title, MAX_TITLE, Limit::Chars(MAX_TITLE));
    prepared.title = title.lines().next().unwrap_or_default().to_owned();
    prepared.template_note = note;
    prepared
}

/// The long values of an event, which are what gets cut.
#[derive(Debug, Clone, Copy)]
enum Large {
    DiffFull,
    DiffAdded,
    DiffRemoved,
    Snapshot,
    Triggered,
}

const LARGE: [Large; 5] = [
    Large::DiffFull,
    Large::DiffAdded,
    Large::DiffRemoved,
    Large::Snapshot,
    Large::Triggered,
];

impl Large {
    /// How much of `limit` this value uses in `vars`.
    fn size(self, vars: &Vars, limit: Limit) -> usize {
        match self {
            Self::DiffFull => limit.measure(&vars.diff_full),
            Self::DiffAdded => limit.measure(&vars.diff_added),
            Self::DiffRemoved => limit.measure(&vars.diff_removed),
            Self::Snapshot => limit.measure(&vars.current_snapshot),
            Self::Triggered => vars
                .triggered_text
                .iter()
                .map(|t| limit.measure(t) + 2)
                .sum(),
        }
    }

    /// Set this value in `to` to `from`'s, cut to at most `max` units of `limit`.
    fn copy_cut(self, to: &mut Vars, from: &Vars, max: usize, limit: Limit) {
        match self {
            Self::DiffFull => to.diff_full = shrink(&from.diff_full, max, limit),
            Self::DiffAdded => to.diff_added = shrink(&from.diff_added, max, limit),
            Self::DiffRemoved => to.diff_removed = shrink(&from.diff_removed, max, limit),
            Self::Snapshot => to.current_snapshot = shrink(&from.current_snapshot, max, limit),
            Self::Triggered => {
                let mut left = max;
                for item in &from.triggered_text {
                    let need = limit.measure(item) + 2;
                    if need <= left {
                        left -= need;
                        to.triggered_text.push(item.clone());
                    } else {
                        if left > 2 {
                            to.triggered_text.push(shrink(item, left - 2, limit));
                        }
                        break;
                    }
                }
            }
        }
    }
}

/// How the notice at the end of a cut-short message reads.
fn suffix(markup: Markup, diff_url: &str) -> String {
    if diff_url.is_empty() {
        "\n\n… truncated.".to_owned()
    } else {
        format!("\n\n… truncated. {}", diff_link(markup, diff_url))
    }
}

/// Render `source`; if the result is too long, cut the long values it actually prints until it
/// fits, then say so.
fn fit(
    markup: Markup,
    limit: Limit,
    source: &str,
    event: &Event,
) -> Result<Prepared, TemplateError> {
    let vars = Vars::from_event(event);
    let whole = render(markup, source, &Arc::new(vars.clone()))?;
    if limit.fits(&whole.body) {
        return Ok(Prepared {
            title: whole.title,
            body: whole.body,
            truncated: false,
            template_note: None,
        });
    }

    let suffix = suffix(markup, &event.diff_url);
    let bare = vars.skeleton();
    let bare_size = limit.measure(&render(markup, source, &Arc::new(bare.clone()))?.body);

    // Only the long values that reach the output are worth cutting.
    let mut used = Vec::new();
    for which in LARGE {
        let mut only = bare.clone();
        which.copy_cut(&mut only, &vars, usize::MAX, limit);
        let shown = limit.measure(&render(markup, source, &Arc::new(only))?.body);
        if shown > bare_size {
            used.push((which, which.size(&vars, limit)));
        }
    }
    used.sort_by_key(|(_, len)| *len);

    let mut budget = limit
        .max()
        .saturating_sub(bare_size + limit.measure(&suffix));
    for _ in 0..4 {
        if budget == 0 || used.is_empty() {
            break;
        }
        // Smallest first, so what a short value does not use is shared by the long ones.
        let mut shrunk = bare.clone();
        let mut left = budget;
        let mut remaining = used.len();
        for (which, len) in &used {
            let share = (left / remaining).min(*len);
            left -= share;
            remaining -= 1;
            which.copy_cut(&mut shrunk, &vars, share, limit);
        }
        let rendered = render(markup, source, &Arc::new(shrunk))?;
        let body = format!("{}{suffix}", rendered.body);
        let size = limit.measure(&body);
        if size <= limit.max() {
            return Ok(Prepared {
                title: rendered.title,
                body,
                truncated: true,
                template_note: None,
            });
        }
        // Escaping made the text longer than the raw values were. Scale the budget by how far
        // over the limit the result is, with a little to spare so that the next pass fits.
        let scaled = budget as u128 * limit.max() as u128 / size as u128;
        let scaled = usize::try_from(scaled).unwrap_or(0);
        budget = scaled.saturating_sub(scaled / 50 + 1);
    }
    Ok(minimal(markup, limit, event))
}

/// The shortest useful message: what changed and where. It fits any limit of 2000 or more,
/// because the name and the URL are cut first.
fn minimal(markup: Markup, limit: Limit, event: &Event) -> Prepared {
    let name = shrink(&event.watch_name, 80, Limit::Chars(80));
    let url = shrink(&event.watch_url, 200, Limit::Chars(200));
    let headline = if event.is_test {
        name.clone()
    } else {
        format!("{name} changed")
    };
    let mut body = format!(
        "{}\n{}{}",
        escape(markup, &headline),
        link(markup, &url),
        suffix(markup, &event.diff_url),
    );
    if !limit.fits(&body) {
        // Only reachable with a tiny limit; a plain cut is the best that can be done.
        body = cut_to(&body, limit.max(), limit).to_owned();
    }
    Prepared {
        title: headline,
        body,
        truncated: true,
        template_note: None,
    }
}
