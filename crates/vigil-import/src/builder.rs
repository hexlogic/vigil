//! What every format builds the same way: the batch, the report and their bookkeeping.

use std::collections::{HashMap, HashSet};

use vigil_core::Watch;
use vigil_notify::Channel;
use vigil_store::{
    ImportBatch, ImportTag, ImportTarget, ImportWatch, NotificationTarget, Tag, match_name,
};

use crate::report::{ItemReport, Note, Report};
use crate::text::scheme_of;
use crate::{Format, ImportError, ImportPlan, MAX_WATCHES, Options};

/// What already exists in the store, so that a plan can say what it would reuse and skip.
#[derive(Debug, Clone, Default)]
pub struct Existing {
    tags: Vec<String>,
    target_urls: HashSet<String>,
    watch_urls: HashSet<String>,
}

impl Existing {
    /// What the store holds now.
    pub fn new(tags: &[Tag], targets: &[NotificationTarget], watches: &[Watch]) -> Self {
        Self {
            tags: tags.iter().map(|t| t.name.clone()).collect(),
            target_urls: targets.iter().map(|t| t.url.clone()).collect(),
            watch_urls: watches.iter().map(|w| w.url.clone()).collect(),
        }
    }
}

/// Collects the watches a format finds, and what to say about each.
pub(crate) struct Builder<'a> {
    format: Format,
    options: &'a Options,
    existing: &'a Existing,
    notes: Vec<Note>,
    items: Vec<ItemReport>,
    batch: ImportBatch,
    tag_defs: HashMap<String, ImportTag>,
    target_defs: HashMap<String, ImportTarget>,
}

impl<'a> Builder<'a> {
    pub(crate) fn new(format: Format, options: &'a Options, existing: &'a Existing) -> Self {
        Self {
            format,
            options,
            existing,
            notes: Vec::new(),
            items: Vec::new(),
            batch: ImportBatch {
                allow_duplicates: options.allow_duplicates,
                ..ImportBatch::default()
            },
            tag_defs: HashMap::new(),
            target_defs: HashMap::new(),
        }
    }

    /// The options of this import.
    pub(crate) fn options(&self) -> &Options {
        self.options
    }

    /// Fail if one more watch would be more than an import reads.
    pub(crate) fn room(&self) -> Result<(), ImportError> {
        if self.items.len() >= MAX_WATCHES {
            return Err(ImportError::TooMany { limit: MAX_WATCHES });
        }
        Ok(())
    }

    /// Something to say about the import as a whole.
    pub(crate) fn note(&mut self, note: Note) {
        self.notes.push(note);
    }

    /// A tag the watches may use. It is only made if a watch that is made uses it.
    pub(crate) fn define_tag(&mut self, tag: ImportTag) {
        self.tag_defs.entry(tag.name.clone()).or_insert(tag);
    }

    /// A target the watches and tags may use, by address. It is only made if something that is
    /// made uses it.
    pub(crate) fn define_target(&mut self, target: ImportTarget) {
        self.target_defs.entry(target.url.clone()).or_insert(target);
    }

    /// A watch that will be made, unless its address is already watched.
    pub(crate) fn add(&mut self, source: String, watch: ImportWatch, mut notes: Vec<Note>) {
        if !self.options.allow_duplicates && self.existing.watch_urls.contains(&watch.settings.url)
        {
            notes.push(Note::skipped(
                "url",
                "a watch for this address exists already; import with duplicates allowed to make another",
            ));
            self.items.push(ItemReport::skipped(source, notes));
            return;
        }
        for name in &watch.tags {
            self.use_tag(name);
        }
        for url in &watch.target_urls {
            self.use_target(url);
        }
        self.batch.watches.push(watch);
        self.items.push(ItemReport::made(source, notes));
    }

    /// A watch that will not be made.
    pub(crate) fn skip(&mut self, source: String, notes: Vec<Note>) {
        self.items.push(ItemReport::skipped(source, notes));
    }

    fn use_tag(&mut self, name: &str) {
        if self.batch.tags.iter().any(|t| t.name == name) {
            return;
        }
        let Some(tag) = self.tag_defs.get(name).cloned() else {
            return;
        };
        for url in &tag.target_urls {
            self.use_target(url);
        }
        self.batch.tags.push(tag);
    }

    fn use_target(&mut self, url: &str) {
        if self.batch.targets.iter().any(|t| t.url == url) {
            return;
        }
        if let Some(target) = self.target_defs.get(url).cloned() {
            self.batch.targets.push(target);
        }
    }

    pub(crate) fn finish(self) -> ImportPlan {
        let tags_created = self
            .batch
            .tags
            .iter()
            .filter(|tag| {
                match_name(self.existing.tags.iter().map(String::as_str), &tag.name).is_none()
            })
            .count();
        let targets_created = self
            .batch
            .targets
            .iter()
            .filter(|target| !self.existing.target_urls.contains(&target.url))
            .count();
        let mut report = Report::new(self.format, self.notes, self.items);
        report.totals.tags_created = tags_created;
        report.totals.targets_created = targets_created;
        ImportPlan {
            batch: self.batch,
            report,
        }
    }
}

/// What to call a notification target that has no name in the source: its kind and, when the
/// address names one, its host (`ntfy ntfy.sh`).
pub(crate) fn target_name(url: &str) -> String {
    let Ok(channel) = Channel::parse(url) else {
        return "notification".to_owned();
    };
    let kind = channel.kind().to_owned();
    // The redacted form hides credentials, so the host it shows is safe to use.
    let redacted = channel.redacted();
    let authority = redacted
        .split_once("://")
        .map_or("", |(_, rest)| rest)
        .split(['/', '?'])
        .next()
        .unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host.is_empty() || host.contains('*') {
        kind
    } else {
        format!("{kind} {host}")
    }
}

/// Check a notification address from a source. If it is no good the note says which kind could
/// not be carried over, never the address.
pub(crate) fn check_target(url: &str) -> Result<(), Note> {
    match Channel::parse(url.trim()) {
        Ok(_) => Ok(()),
        Err(_) => Err(match scheme_of(url.trim()) {
            Some(scheme) => Note::skipped(
                "notification_urls",
                format!(
                    "a {scheme}:// notification address is not supported or not valid, and was left out"
                ),
            ),
            None => Note::skipped(
                "notification_urls",
                "a notification address that is not a URL was left out",
            ),
        }),
    }
}
