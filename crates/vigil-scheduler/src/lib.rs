//! Checking watches: one check (fetch, process, record) and the scheduler that repeats them.

pub mod backoff;
mod check;
pub mod limits;
mod scheduler;

pub use check::{CheckReport, CheckResult, Checker};
pub use scheduler::{Backend, Job, RunOutcome, Scheduler, Settings};
