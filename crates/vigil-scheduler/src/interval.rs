//! Reading intervals typed by a person, such as `5m` or `1h 30m`.

use std::time::Duration;

use crate::backoff::{MAX_INTERVAL, MIN_INTERVAL};

/// Why an interval could not be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntervalError {
    /// The text is not a duration.
    #[error("{0} (try 30s, 5m, 1h)")]
    Invalid(String),
    /// The interval is under one second.
    #[error("the interval must be at least 1s")]
    TooShort,
    /// The interval is over 365 days.
    #[error("the interval must be at most 365 days")]
    TooLong,
}

/// Read an interval such as `30s`, `5m`, `1h` or `1h 30m`. It must be between
/// [`MIN_INTERVAL`] and [`MAX_INTERVAL`], the range the scheduler accepts, so the same text is
/// valid in the command line and in the web UI.
pub fn parse_interval(text: &str) -> Result<Duration, IntervalError> {
    let interval = humantime::parse_duration(text.trim())
        .map_err(|error| IntervalError::Invalid(error.to_string()))?;
    if interval < MIN_INTERVAL {
        return Err(IntervalError::TooShort);
    }
    if interval > MAX_INTERVAL {
        return Err(IntervalError::TooLong);
    }
    Ok(interval)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compound_durations_are_accepted() {
        assert_eq!(parse_interval("1h 30m"), Ok(Duration::from_secs(5400)));
        assert_eq!(parse_interval("90s"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_interval("  5m "), Ok(Duration::from_secs(300)));
    }

    #[test]
    fn the_shortest_interval_is_a_second() {
        assert_eq!(parse_interval("1s"), Ok(MIN_INTERVAL));
        assert_eq!(parse_interval("0s"), Err(IntervalError::TooShort));
        assert_eq!(parse_interval("500ms"), Err(IntervalError::TooShort));
    }

    #[test]
    fn the_longest_interval_is_a_year() {
        assert_eq!(parse_interval("365d"), Ok(MAX_INTERVAL));
        assert_eq!(parse_interval("366d"), Err(IntervalError::TooLong));
        assert!(parse_interval("500000000000y").is_err());
    }

    #[test]
    fn text_that_is_not_a_duration_is_invalid() {
        for text in ["soon", "", "5", "-5m", "5 minutes ago"] {
            assert!(
                matches!(parse_interval(text), Err(IntervalError::Invalid(_))),
                "{text:?}"
            );
        }
    }

    #[test]
    fn the_error_says_what_to_type() {
        let message = parse_interval("soon").err().map(|e| e.to_string());
        assert!(
            message.is_some_and(|m| m.ends_with("(try 30s, 5m, 1h)")),
            "the hint is part of the message"
        );
    }
}
