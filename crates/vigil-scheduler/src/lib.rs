//! Checking watches: one check (fetch, process, record) and the scheduler that repeats them.

pub mod backoff;
mod check;
mod delivery;
mod describe;
mod handle;
pub mod limits;
mod notify;
mod preview;
mod render;
mod runs;
mod scheduler;
mod state;

pub use check::{CheckReport, CheckResult, Checker};
pub use handle::{SchedulerHandle, Stopped, Ticket};
pub use notify::{Notifier, TargetReport};
pub use preview::{Preview, PreviewReport};
pub use scheduler::{Backend, Job, RunOutcome, Scheduler, Settings};
