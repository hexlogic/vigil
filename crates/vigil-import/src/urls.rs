//! A plain list of addresses: one per line, optionally followed by tags.
//!
//! ```text
//! # comments and blank lines are ignored
//! https://example.com/pricing
//! https://example.com/blog  news, company
//! ```

use vigil_core::Schedule;
use vigil_fetch::check_url;
use vigil_store::{ImportTag, ImportWatch, NewWatch};

use crate::builder::{Builder, Existing};
use crate::report::Note;
use crate::text::short_url;
use crate::{Format, ImportError, ImportPlan, Options};

/// Longest tag name, as the web UI allows.
pub(crate) const TAG_NAME_LEN: usize = 50;

pub(crate) fn plan(
    bytes: &[u8],
    options: &Options,
    existing: &Existing,
) -> Result<ImportPlan, ImportError> {
    let text = String::from_utf8_lossy(bytes);
    let mut builder = Builder::new(Format::UrlList, options, existing);
    let every = options.every.as_secs();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        builder.room()?;
        let (url, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let source = short_url(url);
        if let Err(error) = check_url(url) {
            let why = match error {
                vigil_fetch::FetchError::UnsupportedScheme(_) => {
                    "only http and https addresses can be watched"
                }
                _ => "this is not an address",
            };
            builder.skip(
                format!("line {}", number + 1),
                vec![Note::skipped("url", why)],
            );
            continue;
        }
        let mut notes = Vec::new();
        let mut tags = Vec::new();
        for name in rest
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            if name.chars().count() > TAG_NAME_LEN {
                notes.push(Note::skipped(
                    "tags",
                    format!("a tag name over {TAG_NAME_LEN} characters was left out"),
                ));
                continue;
            }
            builder.define_tag(ImportTag {
                name: name.to_owned(),
                default_interval_secs: None,
                target_urls: Vec::new(),
            });
            if !tags.iter().any(|t| t == name) {
                tags.push(name.to_owned());
            }
        }
        builder.add(
            source,
            ImportWatch {
                settings: NewWatch::new(url, Schedule::Interval(every)),
                paused: false,
                tags,
                target_urls: Vec::new(),
                template: None,
            },
            notes,
        );
    }
    if builder.options().allow_duplicates {
        builder.note(Note::info(
            "url",
            "duplicates are allowed, so addresses already watched are made again",
        ));
    }
    Ok(builder.finish())
}

/// Whether a request header can be sent: a name of visible characters and a value without
/// control characters.
pub(crate) fn header_is_valid(name: &str, value: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_graphic())
        && !value.chars().any(char::is_control)
}
