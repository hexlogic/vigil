//! Domain types, processing pipeline and diffing. Pure: no network, no database, no tokio.

pub mod conditions;
pub mod decode;
pub mod diff;
pub mod hash;
pub mod model;
pub mod normalize;
pub mod pattern;
pub mod pipeline;
pub mod process;

pub use conditions::{
    CompiledConditions, Conditions, ConditionsError, DiffShape, NotifyOn, Verdict, VerdictReason,
};
pub use diff::{ChangeKind, DiffLine, LineDiff};
pub use hash::ContentHash;
pub use model::{FetchSettings, Processor, Schedule, Watch, WatchId, WatchState};
pub use pipeline::{Pipeline, PipelineError, PipelineErrorKind, SelectMode, Step};
