//! Tracing subscriber setup.

use tracing_subscriber::EnvFilter;

use crate::config::LogFormat;

/// Install the global tracing subscriber. Verbosity comes from `RUST_LOG`.
///
/// The server logs at `info` to stdout. One-shot commands log at `warn` to stderr so that
/// stdout carries only their results.
pub fn init(format: LogFormat, server: bool) {
    let default_level = if server { "info" } else { "warn" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match (format, server) {
        (LogFormat::Text, true) => builder.init(),
        (LogFormat::Json, true) => builder.json().init(),
        (LogFormat::Text, false) => builder.with_writer(std::io::stderr).init(),
        (LogFormat::Json, false) => builder.json().with_writer(std::io::stderr).init(),
    }
}
