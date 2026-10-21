//! Where a watch of a backup sends its notifications: through its tags and to its own
//! addresses, and what is left when the source muted them.
//!
//! Vigil has no mute for one watch. A muted watch is simply not linked to a target, and it is
//! left off every tag that would notify, since a tag's targets are notified about all of its
//! watches. The tag's filters were already merged into the watch, so only the grouping is lost.

use std::collections::HashMap;

use serde_json::Value;
use vigil_store::{ImportTag, ImportTarget};

use super::settings::{Globals, flag, strings, text};
use crate::builder::{Builder, check_target, target_name};
use crate::report::Note;
use crate::urls::TAG_NAME_LEN;

/// Whether the source mutes the notifications of a watch, on its own or for every watch.
pub(super) fn is_muted(watch: &Value, globals: &Globals) -> bool {
    flag(watch.get("notification_muted"), false) || globals.all_muted
}

/// The tags of a watch.
pub(super) struct Tags<'a> {
    /// The names of the tags the watch gets.
    pub(super) names: Vec<String>,
    /// The definitions of all its tags in the source, including those it does not get: they
    /// add their filters and conditions to the watch either way.
    pub(super) values: Vec<&'a Value>,
}

/// Define the tags of a watch, with the targets they notify, and say which the watch gets.
/// When every notification is muted, tags are made without targets.
pub(super) fn map_tags<'a>(
    watch: &Value,
    tags: &'a HashMap<String, Value>,
    globals: &Globals,
    muted: bool,
    builder: &mut Builder<'_>,
    notes: &mut Vec<Note>,
) -> Tags<'a> {
    let mut out = Tags {
        names: Vec::new(),
        values: Vec::new(),
    };
    for id in strings(watch.get("tags")) {
        let Some(tag) = tags.get(&id.to_ascii_lowercase()).or_else(|| tags.get(&id)) else {
            continue;
        };
        let Some(name) = text(tag.get("title")) else {
            continue;
        };
        if name.chars().count() > TAG_NAME_LEN {
            notes.push(Note::skipped(
                "tags",
                format!("a tag name over {TAG_NAME_LEN} characters was left out"),
            ));
            continue;
        }
        let mut target_urls = Vec::new();
        if !globals.all_muted {
            for url in strings(tag.get("notification_urls")) {
                match check_target(&url) {
                    Ok(()) => {
                        builder.define_target(ImportTarget {
                            name: target_name(&url),
                            url: url.clone(),
                        });
                        target_urls.push(url);
                    }
                    Err(note) => notes.push(note),
                }
            }
        }
        builder.define_tag(ImportTag {
            name: name.to_owned(),
            default_interval_secs: None,
            target_urls,
        });
        out.values.push(tag);
        if muted && builder.tag_notifies(name) {
            notes.push(Note::approximated(
                "notification_muted",
                format!(
                    "the tag {name:?} was left off because it notifies and notifications were muted in the source"
                ),
            ));
            continue;
        }
        out.names.push(name.to_owned());
    }
    out
}

/// Define the notification targets of a watch itself and return their addresses. A muted watch
/// gets none.
pub(super) fn map_targets(
    watch: &Value,
    globals: &Globals,
    muted: bool,
    builder: &mut Builder<'_>,
    notes: &mut Vec<Note>,
) -> Vec<String> {
    let own_urls = strings(watch.get("notification_urls"));
    let urls = if own_urls.is_empty() {
        &globals.notification_urls
    } else {
        &own_urls
    };
    let mut target_urls = Vec::new();
    if muted {
        if !urls.is_empty() {
            notes.push(Note::approximated("notification_muted", "notifications were muted in the source, so the watch is not linked to any notification target"));
        }
        return target_urls;
    }
    for url in urls {
        match check_target(url) {
            Ok(()) => {
                builder.define_target(ImportTarget {
                    name: target_name(url),
                    url: url.clone(),
                });
                if !target_urls.contains(url) {
                    target_urls.push(url.clone());
                }
            }
            Err(note) => notes.push(note),
        }
    }
    target_urls
}
