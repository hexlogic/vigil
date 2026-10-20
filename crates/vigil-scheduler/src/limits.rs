//! Concurrency limits: a global cap plus a cap per target host.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Held while a check runs; dropping it frees the slots.
#[derive(Debug)]
pub struct Permit {
    _host: Option<OwnedSemaphorePermit>,
    _global: Option<OwnedSemaphorePermit>,
}

/// Limits how many checks run at once, overall and against any single host.
#[derive(Debug)]
pub struct Limits {
    global: Arc<Semaphore>,
    per_host: usize,
    hosts: Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl Limits {
    /// At most `global` checks at once and `per_host` per host. Zeros are raised to 1.
    pub fn new(global: usize, per_host: usize) -> Self {
        Self {
            global: Arc::new(Semaphore::new(global.max(1))),
            per_host: per_host.max(1),
            hosts: Mutex::new(HashMap::new()),
        }
    }

    /// Wait for a slot against `host` and then a global slot.
    ///
    /// The host slot comes first so a check queued behind a busy host does not hold a global
    /// slot that other hosts could use.
    pub async fn acquire(&self, host: &str) -> Permit {
        let semaphore = {
            let mut hosts = self.hosts.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(
                hosts
                    .entry(host.to_owned())
                    .or_insert_with(|| Arc::new(Semaphore::new(self.per_host))),
            )
        };
        // The semaphores are never closed, so acquiring cannot fail. If it somehow did, running
        // without that limit beats refusing to check at all.
        let host_permit = semaphore.acquire_owned().await.ok();
        let global_permit = Arc::clone(&self.global).acquire_owned().await.ok();
        Permit {
            _host: host_permit,
            _global: global_permit,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    async fn is_blocked(limits: &Limits, host: &str) -> bool {
        tokio::time::timeout(Duration::from_millis(50), limits.acquire(host))
            .await
            .is_err()
    }

    #[tokio::test(start_paused = true)]
    async fn per_host_limit_blocks_the_next_check() {
        let limits = Limits::new(10, 2);
        let _a = limits.acquire("a:443").await;
        let _b = limits.acquire("a:443").await;
        assert!(is_blocked(&limits, "a:443").await);
        // Other hosts are unaffected.
        let _c = limits.acquire("b:443").await;
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_a_permit_frees_the_slot() {
        let limits = Limits::new(10, 1);
        let first = limits.acquire("a:443").await;
        assert!(is_blocked(&limits, "a:443").await);
        drop(first);
        let _second = limits.acquire("a:443").await;
    }

    #[tokio::test(start_paused = true)]
    async fn global_limit_applies_across_hosts() {
        let limits = Limits::new(2, 2);
        let _a = limits.acquire("a:443").await;
        let _b = limits.acquire("b:443").await;
        assert!(is_blocked(&limits, "c:443").await);
    }

    #[tokio::test(start_paused = true)]
    async fn zero_limits_are_raised_to_one() {
        let limits = Limits::new(0, 0);
        let _a = limits.acquire("a:443").await;
    }
}
