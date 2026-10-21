//! The request settings of the form.

use std::collections::BTreeMap;

use vigil_core::{FetchSettings, FetchSettingsError, Method};

use super::super::draft::Draft;
use super::Problems;
use crate::i18n::Translator;

/// The timeouts a watch may have, in seconds.
pub(crate) const TIMEOUT_SECS: std::ops::RangeInclusive<u64> = 1..=3600;

/// Whether a request header can be sent: a name of visible characters and a value without
/// control characters.
pub(crate) fn header_is_valid(name: &str, value: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_graphic())
        && !value.chars().any(char::is_control)
}

pub(super) fn fetch_settings(
    draft: &Draft,
    t: &Translator,
    problems: &mut Problems,
) -> FetchSettings {
    let timeout_secs = match draft.timeout.trim() {
        "" => None,
        text => match text.parse::<u64>() {
            Ok(n) if TIMEOUT_SECS.contains(&n) => Some(n),
            _ => {
                problems.timeout = Some(t.t("form.err.timeout"));
                None
            }
        },
    };
    let mut headers = BTreeMap::new();
    for (number, line) in draft.headers.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parsed = line.split_once(':').map(|(n, v)| (n.trim(), v.trim()));
        match parsed {
            Some((name, value)) if header_is_valid(name, value) => {
                headers.insert(name.to_owned(), value.to_owned());
            }
            _ => {
                problems.headers = Some(t.t1("form.err.header", "n", number + 1));
                break;
            }
        }
    }
    let user_agent = Some(draft.user_agent.trim().to_owned()).filter(|ua| !ua.is_empty());
    let method = match draft.method.trim() {
        "" => Method::default(),
        text => text.parse().unwrap_or_else(|_| {
            problems.method = Some(t.t("form.err.method"));
            Method::default()
        }),
    };
    // A body is sent exactly as typed, except that a form submits line breaks as CRLF.
    let body = Some(draft.body.replace("\r\n", "\n")).filter(|body| !body.trim().is_empty());
    let settings = FetchSettings {
        timeout_secs,
        user_agent,
        headers,
        method,
        body,
    };
    if problems.method.is_none()
        && let Err(error) = settings.validate()
    {
        problems.body = Some(t.t(match error {
            FetchSettingsError::BodyWithGet => "form.err.body_get",
            FetchSettingsError::BodyTooLarge => "form.err.body_size",
        }));
    }
    settings
}
