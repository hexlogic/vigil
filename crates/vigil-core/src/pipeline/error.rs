//! Errors from building and running a pipeline.

use std::fmt;

use crate::model::Processor;
use crate::pattern::PatternError;
use crate::process::{MAX_OUTPUT_BYTES, ProcessError};

/// Why a pipeline could not be built or run.
///
/// Displays as `step 2 (regex_extract): matched nothing`, or just the reason when it is not
/// about one step (an undecodable body, say). Steps are counted from 1.
#[derive(Debug)]
pub struct PipelineError {
    step: Option<(usize, &'static str)>,
    kind: PipelineErrorKind,
}

/// What went wrong in a [`PipelineError`].
#[derive(Debug, thiserror::Error)]
pub enum PipelineErrorKind {
    /// The body could not be turned into text by the processor.
    #[error("{0}")]
    Process(#[from] ProcessError),
    /// A pattern or regular expression is invalid.
    #[error("{0}")]
    Pattern(#[from] PatternError),
    /// A CSS selector is invalid.
    #[error("invalid selector {selector:?}: {reason}")]
    Selector {
        /// The selector as given.
        selector: String,
        /// One-line description of the problem.
        reason: String,
    },
    /// A JSONPath expression is invalid.
    #[error("invalid jsonpath {expr:?}: {reason}")]
    JsonPath {
        /// The expression as given.
        expr: String,
        /// One-line description of the problem.
        reason: String,
    },
    /// An XPath expression is invalid or could not be evaluated.
    #[error("invalid xpath {expr:?}: {reason}")]
    XPath {
        /// The expression as given.
        expr: String,
        /// One-line description of the problem.
        reason: String,
    },
    /// A `remove` step selected something that cannot be deleted.
    #[error("remove can only delete elements and text, not attribute values or computed results")]
    NotRemovable,
    /// The step needs a different processor than the watch uses.
    #[error("{stage} steps need the {needed} processor, but the watch uses {found}")]
    WrongProcessor {
        /// Kind of step ("html").
        stage: &'static str,
        /// The processor the step needs.
        needed: Processor,
        /// The processor the watch uses.
        found: Processor,
    },
    /// Steps are in the wrong order.
    #[error("{stage} steps must come before {after} steps")]
    OutOfOrder {
        /// Kind of the step that is too late.
        stage: &'static str,
        /// Kind of the step that came before it.
        after: &'static str,
    },
    /// The regex has fewer capture groups than the step asks for.
    #[error("capture group {group} does not exist, the pattern has {groups}")]
    NoSuchGroup {
        /// The group the step asked for.
        group: usize,
        /// How many groups the pattern has, not counting the whole match.
        groups: usize,
    },
    /// A step that selects part of the text found nothing to select.
    #[error("matched nothing")]
    NoMatch,
    /// A step produced more than the output limit.
    #[error("the output is larger than {} MiB", MAX_OUTPUT_BYTES / (1024 * 1024))]
    TooLarge,
}

impl PipelineError {
    /// An error about the step at `index` (counted from 0) called `name`.
    pub(crate) fn at(index: usize, name: &'static str, kind: impl Into<PipelineErrorKind>) -> Self {
        Self {
            step: Some((index + 1, name)),
            kind: kind.into(),
        }
    }

    /// An error that is not about one step.
    pub(crate) fn general(kind: PipelineErrorKind) -> Self {
        Self { step: None, kind }
    }

    /// The step the error is about, counted from 1, if it is about one.
    pub fn step(&self) -> Option<usize> {
        self.step.map(|(number, _)| number)
    }

    /// What went wrong.
    pub fn kind(&self) -> &PipelineErrorKind {
        &self.kind
    }
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.step {
            Some((number, name)) => write!(f, "step {number} ({name}): {}", self.kind),
            None => self.kind.fmt(f),
        }
    }
}

impl std::error::Error for PipelineError {}

impl From<ProcessError> for PipelineError {
    fn from(error: ProcessError) -> Self {
        Self::general(error.into())
    }
}
