//! A backup of another self-hosted change monitor: a ZIP with a folder per watch, or the older
//! single JSON file. See `docs/migrating.md` for what is carried over and how.
//!
//! The files are recognised by their shape and read as loose JSON, since the set of options
//! changes from release to release. An option the importer does not know is reported once, by
//! name, for the whole import; options with a default value are not reported at all.

mod extract;
mod filters;
mod notify;
mod request;
mod settings;
mod store;
mod template;
mod watch;

use std::collections::BTreeSet;
use std::io::{Read, Seek};

use crate::builder::{Builder, Existing};
use crate::report::Note;
use crate::{Format, ImportError, ImportPlan, Options};
use settings::Globals;
use store::{Datastore, Layout};
use watch::{Mapped, map_watch};

pub(crate) use crate::urls::header_is_valid;

/// Most unknown option names that are listed in the report.
const MOST_UNKNOWN: usize = 20;

/// Plan the import of a ZIP backup, reading only the entries that matter.
pub(crate) fn plan_zip(
    reader: impl Read + Seek,
    options: &Options,
    existing: &Existing,
) -> Result<ImportPlan, ImportError> {
    plan_data(store::from_zip(reader)?, options, existing)
}

/// Plan the import of the older single JSON file.
pub(crate) fn plan_json(
    bytes: &[u8],
    options: &Options,
    existing: &Existing,
) -> Result<ImportPlan, ImportError> {
    plan_data(store::from_json(bytes)?, options, existing)
}

fn plan_data(
    data: Datastore,
    options: &Options,
    existing: &Existing,
) -> Result<ImportPlan, ImportError> {
    let globals = Globals::read(&data.settings);
    let mut builder = Builder::new(Format::Backup, options, existing);

    if data.ignored_single {
        builder.note(Note::info(
            "layout",
            "the archive also holds a copy of the older single-file datastore; only the per-watch files were read",
        ));
    }
    if data.layout == Layout::Single {
        builder.note(Note::info(
            "layout",
            "this is the older single-file datastore",
        ));
    }
    if globals.all_paused {
        builder.note(Note::approximated(
            "all_paused",
            "all watches were paused in the source, so they are made paused",
        ));
    }
    if globals.all_muted {
        builder.note(Note::approximated("all_muted", "all notifications were muted in the source, so no watch is linked to a notification target, tags are made without any, and a watch is left off an existing tag that notifies"));
    }
    if globals.has_notification_template {
        builder.note(Note::skipped("notification_body", "the message template that applies to every watch was not imported; watches with a message of their own keep it"));
    }
    let copied = globals.copied_settings();
    if !copied.is_empty() {
        builder.note(Note::info(
            "global settings",
            format!(
                "settings that applied to every watch in the source were copied into each watch: {}",
                copied.join(", ")
            ),
        ));
    }
    if globals.user_agent.is_some() {
        builder.note(Note::info("default_ua", "the source's default User-Agent was copied to each watch, so that sites see the same client"));
    }

    let mut unknown = BTreeSet::new();
    for (id, reason) in &data.unreadable {
        builder.skip(
            format!("watch {id}"),
            vec![Note::skipped("watch", reason.clone())],
        );
    }
    for (_, value) in &data.watches {
        builder.room()?;
        match map_watch(value, &data.tags, &globals, &mut builder, &mut unknown) {
            Mapped::Made {
                source,
                watch,
                notes,
            } => builder.add(source, *watch, notes),
            Mapped::Skipped { source, notes } => builder.skip(source, notes),
        }
    }
    if !unknown.is_empty() {
        let shown: Vec<&str> = unknown
            .iter()
            .take(MOST_UNKNOWN)
            .map(String::as_str)
            .collect();
        let more = unknown.len().saturating_sub(MOST_UNKNOWN);
        builder.note(Note::skipped(
            "unknown options",
            format!(
                "options this importer does not know were ignored: {}{}",
                shown.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            ),
        ));
    }
    Ok(builder.finish())
}
