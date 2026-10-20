//! Retry delays and jitter.

use std::time::Duration;

/// Longest delay between checks of a failing watch.
pub const MAX_BACKOFF: Duration = Duration::from_secs(6 * 60 * 60);

/// Longest interval a watch may have; anything above is treated as this. It keeps the time
/// arithmetic far from overflow and is a limit nobody checking web pages will miss.
pub const MAX_INTERVAL: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// Upper bound of the default jitter.
const MAX_DEFAULT_JITTER: Duration = Duration::from_secs(60);

/// Time to wait before the next check.
///
/// Equals `interval` while checks succeed. After `consecutive_errors` failures the interval is
/// doubled that many times, capped at [`MAX_BACKOFF`]. A watch whose interval already exceeds
/// the cap keeps its own interval, so failures never make a check more frequent.
pub fn next_delay(interval: Duration, consecutive_errors: u32) -> Duration {
    if consecutive_errors == 0 {
        return interval;
    }
    let factor = 1u32 << consecutive_errors.min(30);
    interval.max(interval.saturating_mul(factor).min(MAX_BACKOFF))
}

/// Jitter for watches that do not set their own: a tenth of the interval, at most a minute.
pub fn default_jitter(interval: Duration) -> Duration {
    (interval / 10).min(MAX_DEFAULT_JITTER)
}

/// A random duration between zero and `max`, inclusive.
pub fn random_jitter(max: Duration) -> Duration {
    let millis = u64::try_from(max.as_millis()).unwrap_or(u64::MAX);
    if millis == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis(fastrand::u64(0..=millis))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    #[test]
    fn no_errors_means_the_plain_interval() {
        assert_eq!(next_delay(5 * MINUTE, 0), 5 * MINUTE);
    }

    #[test]
    fn each_error_doubles_the_delay() {
        assert_eq!(next_delay(MINUTE, 1), 2 * MINUTE);
        assert_eq!(next_delay(MINUTE, 2), 4 * MINUTE);
        assert_eq!(next_delay(MINUTE, 3), 8 * MINUTE);
    }

    #[test]
    fn delay_is_capped() {
        assert_eq!(next_delay(MINUTE, 20), MAX_BACKOFF);
        assert_eq!(next_delay(MINUTE, u32::MAX), MAX_BACKOFF);
    }

    #[test]
    fn long_intervals_are_never_shortened() {
        let daily = Duration::from_secs(24 * 3600);
        assert_eq!(next_delay(daily, 1), daily);
        assert_eq!(next_delay(daily, 10), daily);
    }

    #[test]
    fn default_jitter_is_a_tenth_up_to_a_minute() {
        assert_eq!(
            default_jitter(Duration::from_secs(100)),
            Duration::from_secs(10)
        );
        assert_eq!(default_jitter(Duration::from_secs(86_400)), MINUTE);
        assert_eq!(default_jitter(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn random_jitter_stays_within_bounds() {
        assert_eq!(random_jitter(Duration::ZERO), Duration::ZERO);
        let max = Duration::from_millis(50);
        for _ in 0..200 {
            assert!(random_jitter(max) <= max);
        }
    }
}
