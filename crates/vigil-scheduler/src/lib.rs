//! Checking watches: one check (fetch, process, record) and the scheduler that repeats them.

pub mod backoff;
mod check;
mod describe;
pub mod limits;
mod preview;
mod render;
mod scheduler;

pub use check::{CheckReport, CheckResult, Checker};
pub use preview::{Preview, PreviewReport};
pub use scheduler::{Backend, Job, RunOutcome, Scheduler, Settings};
