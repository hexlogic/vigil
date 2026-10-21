//! What every format builds the same way: the batch, the report and their bookkeeping.

use std::collections::{HashMap, HashSet};

use vigil_core::Watch;
use vigil_store::{ImportBatch, ImportTag, ImportWatch, NotificationTarget, Tag, match_name};

use crate::report::{ItemReport, Note, Report};
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
        self.batch.tags.push(tag);
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
