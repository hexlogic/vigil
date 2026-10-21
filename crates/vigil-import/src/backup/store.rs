//! Reading the files of a backup into JSON values.
//!
//! Files are recognised by their shape, not by their names, apart from the one convention that
//! a ZIP keeps a folder per watch: `<uuid>/watch.json` and `<uuid>/tag.json`. Anything else in
//! the archive (snapshots, screenshots, compressed history) is never read, and nothing is ever
//! extracted to disk. Every entry is read through a limit that does not trust the size the
//! archive declares.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use serde_json::Value;

use crate::{Format, ImportError};

/// Most entries an archive may have.
const MAX_ENTRIES: usize = 200_000;

/// Largest single JSON file that is read, in bytes.
const MAX_ENTRY_BYTES: u64 = 10 * 1024 * 1024;

/// Most bytes read from one archive in all.
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// Which of the two layouts the data came in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Layout {
    /// One folder per watch and per tag, each with its own JSON file.
    PerWatch,
    /// One JSON file with every watch in a `watching` object and the tags in the settings.
    Single,
}

/// The data of a backup, as read.
#[derive(Debug)]
pub(super) struct Datastore {
    pub(super) layout: Layout,
    /// The settings object (the value of the file's `settings` key).
    pub(super) settings: Value,
    /// Tags by id.
    pub(super) tags: HashMap<String, Value>,
    /// Watches by id, oldest first.
    pub(super) watches: Vec<(String, Value)>,
    /// Files that could not be read, by the id of the watch they belong to.
    pub(super) unreadable: Vec<(String, String)>,
    /// A single-file datastore was present next to the per-watch files. New backups still carry
    /// a stale copy, so it is ignored.
    pub(super) ignored_single: bool,
}

fn invalid(reason: impl Into<String>) -> ImportError {
    ImportError::Invalid {
        format: Format::Backup,
        reason: reason.into(),
    }
}

/// `<uuid>/<file>`: the id and the file name, if the path has that shape and the id looks like
/// a UUID.
fn in_folder(name: &str) -> Option<(&str, &str)> {
    let (folder, file) = name.split_once('/')?;
    let is_id = folder.len() == 36 && folder.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
    (is_id && !file.contains('/')).then_some((folder, file))
}

enum Kind {
    Watch(String),
    Tag(String),
    Root,
}

fn kind_of(name: &str) -> Option<Kind> {
    match in_folder(name) {
        Some((id, "watch.json")) => Some(Kind::Watch(id.to_ascii_lowercase())),
        Some((id, "tag.json")) => Some(Kind::Tag(id.to_ascii_lowercase())),
        Some(_) => None,
        None => (!name.contains('/') && name.to_ascii_lowercase().ends_with(".json"))
            .then_some(Kind::Root),
    }
}

/// Read up to `limit` bytes; more is an error.
fn read_limited(reader: impl Read, limit: u64) -> Result<Vec<u8>, ImportError> {
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| invalid(format!("a file in the archive cannot be read: {error}")))?;
    if bytes.len() as u64 > limit {
        return Err(invalid("a file in the archive is too large"));
    }
    Ok(bytes)
}

/// Read a ZIP backup.
pub(super) fn from_zip(bytes: &[u8]) -> Result<Datastore, ImportError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| invalid(format!("this is not a readable ZIP file: {error}")))?;
    if archive.len() > MAX_ENTRIES {
        return Err(invalid("the archive has too many files"));
    }
    let mut total = 0u64;
    let mut found = Found::default();
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
        let Some(kind) = kind_of(&name) else {
            continue;
        };
        let bytes = read_limited(entry, MAX_ENTRY_BYTES)?;
        total += bytes.len() as u64;
        if total > MAX_TOTAL_BYTES {
            return Err(invalid("the archive holds too much data"));
        }
        found.add(kind, &name, &bytes);
    }
    found.finish()
}

/// Read a single JSON file: the older datastore.
pub(super) fn from_json(bytes: &[u8]) -> Result<Datastore, ImportError> {
    let mut found = Found::default();
    found.add(Kind::Root, "datastore", bytes);
    found.finish()
}

#[derive(Default)]
struct Found {
    watches: HashMap<String, Value>,
    tags: HashMap<String, Value>,
    settings: Option<Value>,
    single: Option<Value>,
    unreadable: Vec<(String, String)>,
}

impl Found {
    fn add(&mut self, kind: Kind, name: &str, bytes: &[u8]) {
        let value: Result<Value, _> = serde_json::from_slice(bytes);
        match (kind, value) {
            (Kind::Watch(id), Ok(value)) if value.is_object() => {
                self.watches.insert(id, value);
            }
            (Kind::Tag(id), Ok(value)) if value.is_object() => {
                self.tags.insert(id, value);
            }
            (Kind::Watch(id), _) => self
                .unreadable
                .push((id, "its file is not a JSON object".to_owned())),
            // A tag that cannot be read only loses a tag.
            (Kind::Tag(_), _) => {}
            (Kind::Root, Ok(value)) => {
                if value.get("watching").is_some_and(Value::is_object) {
                    self.single = Some(value);
                } else if value.get("settings").is_some_and(Value::is_object) {
                    self.settings = Some(value);
                }
            }
            (Kind::Root, Err(_)) => {
                let _ = name;
            }
        }
    }

    fn finish(self) -> Result<Datastore, ImportError> {
        let Self {
            watches,
            tags,
            settings,
            single,
            unreadable,
        } = self;
        if !watches.is_empty() || !unreadable.is_empty() {
            // The per-watch files are the data. A single-file datastore beside them is a copy
            // from before they were split out, and may hold watches that were deleted since.
            let settings = settings
                .and_then(|file| file.get("settings").cloned())
                .unwrap_or(Value::Null);
            return Ok(Datastore {
                layout: Layout::PerWatch,
                settings,
                tags,
                watches: ordered(watches),
                unreadable,
                ignored_single: single.is_some(),
            });
        }
        let Some(single) = single else {
            return Err(invalid(
                "there are no watches in it: expected a folder per watch with a watch.json, or a JSON file with a watching object",
            ));
        };
        let settings = single.get("settings").cloned().unwrap_or(Value::Null);
        let tags = settings
            .pointer("/application/tags")
            .and_then(Value::as_object)
            .map(|tags| tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        let watches = single
            .get("watching")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter(|(_, v)| v.is_object())
                    .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Datastore {
            layout: Layout::Single,
            settings,
            tags,
            watches: ordered(watches),
            unreadable: Vec::new(),
            ignored_single: false,
        })
    }
}

/// Oldest first, so a plan reads the same however the archive was ordered.
fn ordered(watches: HashMap<String, Value>) -> Vec<(String, Value)> {
    let mut list: Vec<(String, Value)> = watches.into_iter().collect();
    list.sort_by_key(|(id, watch)| {
        (
            watch.get("date_created").and_then(Value::as_i64),
            id.clone(),
        )
    });
    list
}

#[cfg(test)]
mod tests {
    use super::{Kind, in_folder, kind_of};

    #[test]
    fn only_a_folder_named_like_an_id_holds_a_watch_or_a_tag() {
        let id = "ed00d6c9-83aa-4047-b59b-5ed1fb0434c8";
        assert!(matches!(
            kind_of(&format!("{id}/watch.json")),
            Some(Kind::Watch(_))
        ));
        assert!(matches!(
            kind_of(&format!("{id}/tag.json")),
            Some(Kind::Tag(_))
        ));
        assert!(kind_of(&format!("{id}/history.txt")).is_none());
        assert!(kind_of(&format!("{id}/sub/watch.json")).is_none());
        assert!(kind_of("../etc/watch.json").is_none());
        assert!(kind_of("not-an-id/watch.json").is_none());
        assert!(matches!(kind_of("anything.json"), Some(Kind::Root)));
        assert!(kind_of("notes.txt").is_none());
        assert_eq!(in_folder("short/watch.json"), None);
    }
}
