//! Reads watches from URL lists, OPML and other tools' exports.
//!
//! Importing is two steps. [`plan`] reads a file and says what would be made and what could
//! not be carried over; it touches nothing, so it is also the dry run. The caller then hands the
//! [`ImportPlan::batch`] to `Store::apply_import`. Nothing a source option means is dropped
//! silently: see [`report`].
//!
//! This crate does no I/O of its own. Callers hand it bytes and a description of what already
//! exists ([`Existing`]).

mod backup;
mod builder;
mod detect;
mod distill;
mod opml;
pub mod report;
mod steps;
mod text;
mod urls;
mod urlwatch;

use std::fmt;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::str::FromStr;
use std::time::Duration;

use serde::Serialize;
use vigil_store::ImportBatch;

pub use builder::Existing;
pub use report::{ItemReport, Note, NoteKind, Report, Status, Totals};

/// Largest file that is read, in bytes.
pub const MAX_INPUT_BYTES: usize = 256 * 1024 * 1024;

/// Largest text file (a list, OPML, YAML or JSON), in bytes.
const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;

/// Most watches one import reads.
const MAX_WATCHES: usize = 20_000;

/// What a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum Format {
    /// One address per line, optionally followed by tags.
    UrlList,
    /// An OPML outline of feeds, as feed readers export.
    Opml,
    /// A backup of another self-hosted change monitor: a ZIP with a folder per watch, or its
    /// older single JSON file.
    Backup,
    /// A `urls.yaml` job file of urlwatch.
    Urlwatch,
    /// A JSON export of Distill.
    Distill,
}

impl Format {
    /// Every format, as `--format` names them.
    pub const ALL: [Self; 5] = [
        Self::UrlList,
        Self::Opml,
        Self::Backup,
        Self::Urlwatch,
        Self::Distill,
    ];

    /// The name `--format` and the API take.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UrlList => "url-list",
            Self::Opml => "opml",
            Self::Backup => "backup",
            Self::Urlwatch => "urlwatch",
            Self::Distill => "distill",
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The text is not the name of a format.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown format {0:?}: use url-list, opml, backup, urlwatch or distill")]
pub struct ParseFormatError(String);

impl FromStr for Format {
    type Err = ParseFormatError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|format| format.as_str() == s.trim().to_ascii_lowercase())
            .ok_or_else(|| ParseFormatError(s.trim().to_owned()))
    }
}

/// Settings of an import.
#[derive(Debug, Clone)]
pub struct Options {
    /// The interval of watches whose source has none of its own.
    pub every: Duration,
    /// Make a watch even when one with its address exists.
    pub allow_duplicates: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            every: Duration::from_secs(3600),
            allow_duplicates: false,
        }
    }
}

/// Why a file could not be read at all. A watch that cannot be carried over is a note in the
/// report, not an error.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// The file is none of the formats.
    #[error(
        "the file is not a format that can be imported (url-list, opml, backup, urlwatch or distill)"
    )]
    UnknownFormat,
    /// The file is bigger than allowed.
    #[error("the file is larger than the {limit} byte limit")]
    TooLarge {
        /// The limit in bytes.
        limit: usize,
    },
    /// The file is of the format but cannot be read.
    #[error("cannot read the {format} file: {reason}")]
    Invalid {
        /// The format it was read as.
        format: Format,
        /// What is wrong, in a sentence.
        reason: String,
    },
    /// The file holds more watches than one import reads.
    #[error("the file holds more than {limit} watches")]
    TooMany {
        /// The limit.
        limit: usize,
    },
}

/// What an import would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPlan {
    /// What to make; hand it to `Store::apply_import`.
    pub batch: ImportBatch,
    /// What was read and what could not be carried over.
    pub report: Report,
}

/// Read `bytes` and say what importing them would do. With no `format` it is guessed from the
/// content.
pub fn plan(
    bytes: &[u8],
    format: Option<Format>,
    options: &Options,
    existing: &Existing,
) -> Result<ImportPlan, ImportError> {
    plan_reader(
        Cursor::new(bytes),
        bytes.len() as u64,
        format,
        options,
        existing,
    )
}

/// Like [`plan`], reading from a file or anything else that can seek, `len` bytes long. A ZIP
/// backup is read entry by entry, so a large archive is never held in memory; every other
/// format is read whole, up to 16 MiB.
pub fn plan_reader(
    mut reader: impl Read + Seek,
    len: u64,
    format: Option<Format>,
    options: &Options,
    existing: &Existing,
) -> Result<ImportPlan, ImportError> {
    let too_large = |limit: usize| ImportError::TooLarge { limit };
    if len > MAX_INPUT_BYTES as u64 {
        return Err(too_large(MAX_INPUT_BYTES));
    }
    let io = |error: std::io::Error| ImportError::Invalid {
        format: format.unwrap_or(Format::UrlList),
        reason: format!("the file cannot be read: {error}"),
    };
    let mut magic = [0u8; 4];
    let got = read_up_to(&mut reader, &mut magic).map_err(io)?;
    reader.seek(SeekFrom::Start(0)).map_err(io)?;
    if got == 4 && magic == *b"PK\x03\x04" && matches!(format, None | Some(Format::Backup)) {
        return backup::plan_zip(reader, options, existing);
    }
    if len > MAX_TEXT_BYTES as u64 {
        return Err(too_large(MAX_TEXT_BYTES));
    }
    let mut bytes = Vec::new();
    reader
        .take(MAX_TEXT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(too_large(MAX_TEXT_BYTES));
    }
    let format = match format {
        Some(format) => format,
        None => detect::detect(&bytes).ok_or(ImportError::UnknownFormat)?,
    };
    match format {
        Format::UrlList => urls::plan(&bytes, options, existing),
        Format::Opml => opml::plan(&bytes, options, existing),
        Format::Backup => backup::plan_json(&bytes, options, existing),
        Format::Urlwatch => urlwatch::plan(&bytes, options, existing),
        Format::Distill => distill::plan(&bytes, options, existing),
    }
}

/// Fill `buf` as far as the reader goes; the number of bytes read.
fn read_up_to(reader: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}
