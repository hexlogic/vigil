//! What an import did with each watch, and with each option it could not carry over.
//!
//! Nothing is dropped silently: a source option that has no equivalent, or only an approximate
//! one, is a note. A note never repeats a notification address, a header value or a request
//! body, since those hold credentials.

use serde::Serialize;

use crate::Format;

/// What happened to one watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Made as it was.
    Created,
    /// Made, but some options were skipped or approximated: see its notes.
    Partial,
    /// Not made: see its notes for why.
    Skipped,
}

/// What kind of thing a note says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteKind {
    /// The option has no equivalent and was left out.
    Skipped,
    /// The option was carried over in a way that behaves a little differently.
    Approximated,
    /// A fact worth knowing that changes nothing.
    Info,
}

/// One thing to know about a watch or about the import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Note {
    /// What kind of note.
    pub kind: NoteKind,
    /// The source option it is about, such as `trigger_text`.
    pub option: String,
    /// What was done, in a sentence.
    pub message: String,
}

impl Note {
    /// An option that was left out.
    pub fn skipped(option: &str, message: impl Into<String>) -> Self {
        Self::new(NoteKind::Skipped, option, message)
    }

    /// An option that was carried over approximately.
    pub fn approximated(option: &str, message: impl Into<String>) -> Self {
        Self::new(NoteKind::Approximated, option, message)
    }

    /// A fact.
    pub fn info(option: &str, message: impl Into<String>) -> Self {
        Self::new(NoteKind::Info, option, message)
    }

    fn new(kind: NoteKind, option: &str, message: impl Into<String>) -> Self {
        Self {
            kind,
            option: option.to_owned(),
            message: message.into(),
        }
    }
}

/// The report for one watch of the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemReport {
    /// Which watch: its name, or its address without a query string.
    pub source: String,
    /// What happened to it.
    pub status: Status,
    /// What to know about it.
    pub notes: Vec<Note>,
}

impl ItemReport {
    /// A watch that will be made. It is `Partial` if any note says something was left out or
    /// changed, `Created` if there are none or only facts.
    pub fn made(source: impl Into<String>, notes: Vec<Note>) -> Self {
        let status = if notes.iter().any(|n| n.kind != NoteKind::Info) {
            Status::Partial
        } else {
            Status::Created
        };
        Self {
            source: source.into(),
            status,
            notes,
        }
    }

    /// A watch that will not be made.
    pub fn skipped(source: impl Into<String>, notes: Vec<Note>) -> Self {
        Self {
            source: source.into(),
            status: Status::Skipped,
            notes,
        }
    }
}

/// Counts for the whole import.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Totals {
    /// Watches in the source.
    pub found: usize,
    /// Watches that will be made, with or without notes.
    pub created: usize,
    /// Of those, the ones with options left out or changed.
    pub partial: usize,
    /// Watches that will not be made.
    pub skipped: usize,
    /// Tags that will be made.
    pub tags_created: usize,
    /// Notification targets that will be made.
    pub targets_created: usize,
}

/// Everything an import has to say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// The format that was read.
    pub format: Format,
    /// Notes about the import as a whole, such as options of the source that apply to every
    /// watch.
    pub notes: Vec<Note>,
    /// One entry per watch of the source.
    pub items: Vec<ItemReport>,
    /// The counts.
    pub totals: Totals,
}

impl Report {
    /// A report of `items`; the totals are counted here, and the tags and targets that will be
    /// made are filled in by the planner.
    pub fn new(format: Format, notes: Vec<Note>, items: Vec<ItemReport>) -> Self {
        let created = items.iter().filter(|i| i.status != Status::Skipped).count();
        let partial = items.iter().filter(|i| i.status == Status::Partial).count();
        Self {
            format,
            notes,
            totals: Totals {
                found: items.len(),
                created,
                partial,
                skipped: items.len() - created,
                tags_created: 0,
                targets_created: 0,
            },
            items,
        }
    }
}

impl Report {
    /// The report as plain text, for a terminal: the totals, the notes about the import, and
    /// each watch with what could not be carried over.
    pub fn render_text(&self) -> String {
        use std::fmt::Write as _;
        let t = &self.totals;
        let mut out = String::new();
        // Writing to a String cannot fail.
        let _ = writeln!(
            out,
            "format: {}\nfound {} watch(es): {} will be made ({} with notes), {} skipped; {} tag(s) and {} notification target(s) will be made",
            self.format,
            t.found,
            t.created,
            t.partial,
            t.skipped,
            t.tags_created,
            t.targets_created
        );
        for note in &self.notes {
            let _ = writeln!(
                out,
                "  {:<12} {}: {}",
                note.kind.label(),
                note.option,
                note.message
            );
        }
        for item in &self.items {
            let _ = writeln!(out, "\n{} [{}]", item.source, item.status.label());
            for note in &item.notes {
                let _ = writeln!(
                    out,
                    "  {:<12} {}: {}",
                    note.kind.label(),
                    note.option,
                    note.message
                );
            }
        }
        out
    }
}

impl NoteKind {
    fn label(self) -> &'static str {
        match self {
            Self::Skipped => "skipped",
            Self::Approximated => "approximated",
            Self::Info => "info",
        }
    }
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Created => "will be made",
            Self::Partial => "will be made, with notes",
            Self::Skipped => "skipped",
        }
    }
}
