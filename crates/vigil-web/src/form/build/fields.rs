//! Small readers for the form's plain fields.

use uuid::Uuid;
use vigil_scheduler::{IntervalError, parse_interval};
use vigil_store::Tag;

use super::super::draft::Draft;
use super::{DEFAULT_EVERY_SECS, Problems};
use crate::i18n::Translator;

/// The ids in `chosen` that exist among `known`; anything else is a problem.
pub(super) fn chosen(
    chosen: &[String],
    known: impl Iterator<Item = Uuid>,
    problem: &mut Option<String>,
    t: &Translator,
) -> Vec<Uuid> {
    let known: Vec<Uuid> = known.collect();
    let mut out = Vec::new();
    for text in chosen {
        match text.parse::<Uuid>() {
            Ok(id) if known.contains(&id) => {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
            _ => *problem = Some(t.t("form.err.unknown_choice")),
        }
    }
    out
}

/// The interval: what was typed, else the shortest default of the chosen tags, else an hour.
pub(super) fn every(
    draft: &Draft,
    tags: &[Tag],
    chosen: &[Uuid],
    t: &Translator,
    problems: &mut Problems,
) -> u64 {
    let text = draft.every.trim();
    if text.is_empty() {
        return tags
            .iter()
            .filter(|tag| chosen.contains(&tag.id))
            .filter_map(|tag| tag.default_interval_secs)
            .min()
            .unwrap_or(DEFAULT_EVERY_SECS);
    }
    match parse_interval(text) {
        Ok(every) => every.as_secs(),
        Err(error) => {
            problems.every = Some(t.t(match error {
                IntervalError::Invalid(_) => "form.err.every_invalid",
                IntervalError::TooShort => "form.err.every_short",
                IntervalError::TooLong => "form.err.every_long",
            }));
            DEFAULT_EVERY_SECS
        }
    }
}

/// A whole number typed in a field, or `default` when it is empty.
pub(super) fn number(
    text: &str,
    default: u32,
    problem: &mut Option<String>,
    t: &Translator,
) -> u32 {
    match text.trim() {
        "" => default,
        text => text.parse().unwrap_or_else(|_| {
            *problem = Some(t.t("form.err.number"));
            default
        }),
    }
}
