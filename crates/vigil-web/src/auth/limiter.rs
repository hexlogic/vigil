//! Slowing down password guessing.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Failed sign-ins allowed within [`WINDOW`].
const MAX_FAILURES: usize = 5;

/// How far back failed sign-ins count.
const WINDOW: Duration = Duration::from_secs(60);

/// Remembers recent failed sign-ins. There is one account, so there is one count; it is
/// kept in memory and starts over with the process.
#[derive(Debug, Default)]
pub(crate) struct LoginLimiter {
    failures: Mutex<VecDeque<Instant>>,
}

impl LoginLimiter {
    /// How long to refuse sign-ins for, if there were too many failures lately.
    pub(crate) fn wait(&self, now: Instant) -> Option<Duration> {
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        forget_old(&mut failures, now);
        if failures.len() < MAX_FAILURES {
            return None;
        }
        failures
            .front()
            .map(|oldest| (*oldest + WINDOW).saturating_duration_since(now))
    }

    /// Note a failed sign-in.
    pub(crate) fn record_failure(&self, now: Instant) {
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        forget_old(&mut failures, now);
        failures.push_back(now);
    }

    /// Forget the failures, after a successful sign-in.
    pub(crate) fn reset(&self) {
        self.failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

fn forget_old(failures: &mut VecDeque<Instant>, now: Instant) {
    while failures
        .front()
        .is_some_and(|at| now.saturating_duration_since(*at) >= WINDOW)
    {
        failures.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_failures_in_a_minute_lock_sign_in_until_the_oldest_ages_out() {
        let limiter = LoginLimiter::default();
        let start = Instant::now();
        for second in 0..5 {
            assert_eq!(limiter.wait(start + Duration::from_secs(second)), None);
            limiter.record_failure(start + Duration::from_secs(second));
        }
        let wait = limiter.wait(start + Duration::from_secs(10));
        assert_eq!(wait, Some(Duration::from_secs(50)));
        assert_eq!(
            limiter.wait(start + Duration::from_secs(59)),
            Some(Duration::from_secs(1))
        );
        assert_eq!(limiter.wait(start + Duration::from_secs(60)), None);
    }

    #[test]
    fn a_success_clears_the_count() {
        let limiter = LoginLimiter::default();
        let now = Instant::now();
        for _ in 0..5 {
            limiter.record_failure(now);
        }
        assert!(limiter.wait(now).is_some());
        limiter.reset();
        assert_eq!(limiter.wait(now), None);
    }

    #[test]
    fn failures_spread_over_more_than_a_minute_never_lock() {
        let limiter = LoginLimiter::default();
        let start = Instant::now();
        for n in 0..20 {
            let at = start + Duration::from_secs(n * 20);
            assert_eq!(limiter.wait(at), None, "attempt {n}");
            limiter.record_failure(at);
        }
    }
}
