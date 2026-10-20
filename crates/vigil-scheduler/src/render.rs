//! Running a pipeline on its own thread, under a timeout.
//!
//! Processing is CPU work on untrusted input (HTML, JSON, user-written jq programs). On an
//! async worker it would stall other checks, and in `spawn_blocking` a program that never
//! ends would keep the runtime from shutting down. A plain thread has neither problem: when
//! the limit passes the check fails and its slots are released, and the runtime does not wait
//! for the thread. A runaway thread keeps its CPU until the process exits; that is the price
//! of not being able to interrupt it.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::oneshot;
use vigil_core::WatchId;

/// Stack of the processing thread. Parsing and printing nested documents recurse, so this is
/// larger than the platform default. It is address space, not memory in use.
const STACK_BYTES: usize = 16 * 1024 * 1024;

/// Most processing threads alive at once, abandoned ones included.
const MAX_LIVE: usize = 32;

/// The watches that have a processing thread alive, including threads abandoned after a
/// timeout.
///
/// A thread that outlives its timeout keeps its CPU until it ends, and the watch is checked
/// again after its backoff. Without a limit each retry would add another runaway thread. A
/// watch with a thread still alive is refused a new one, so a stuck watch costs one thread,
/// not one per attempt; the overall cap keeps many stuck watches from adding up.
pub(crate) struct Slots {
    live: Mutex<Vec<WatchId>>,
    cap: usize,
}

/// The slots every check and preview in this process shares.
pub(crate) static SLOTS: Slots = Slots::new(MAX_LIVE);

impl Slots {
    const fn new(cap: usize) -> Self {
        Self {
            live: Mutex::new(Vec::new()),
            cap,
        }
    }

    fn acquire(&'static self, watch: WatchId) -> Result<Slot, String> {
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        if live.contains(&watch) {
            return Err("an earlier processing of this watch is still running".to_owned());
        }
        if live.len() >= self.cap {
            return Err(format!(
                "{} processing threads are still running; not starting another",
                live.len()
            ));
        }
        live.push(watch);
        Ok(Slot { slots: self, watch })
    }
}

/// A claimed slot, released when dropped.
struct Slot {
    slots: &'static Slots,
    watch: WatchId,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut live = self
            .slots
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        live.retain(|id| *id != self.watch);
    }
}

/// Run `work` for `watch` on a new thread and wait for it for at most `limit`.
///
/// A panic or a missed deadline becomes an error message; nothing here can take the caller
/// down. A watch whose earlier thread is still alive gets an error without starting one.
pub(crate) async fn run_with_limit<F>(
    slots: &'static Slots,
    watch: WatchId,
    limit: Duration,
    work: F,
) -> Result<String, String>
where
    F: FnOnce() -> Result<String, String> + Send + 'static,
{
    let slot = slots.acquire(watch)?;
    let (send, receive) = oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("vigil-render".to_owned())
        .stack_size(STACK_BYTES)
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(work))
                .unwrap_or_else(|_| Err("processing panicked".to_owned()));
            // Free the slot before the caller wakes, so its next request is not refused.
            drop(slot);
            // The receiver is gone when the deadline passed; nothing to tell then.
            let _ = send.send(result);
        });
    if let Err(error) = spawned {
        return Err(format!("could not start processing: {error}"));
    }
    match tokio::time::timeout(limit, receive).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("processing stopped unexpectedly".to_owned()),
        Err(_) => Err(format!("processing timed out after {}s", limit.as_secs())),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use vigil_core::WatchId;

    use super::{Slots, run_with_limit};

    const LIMIT: Duration = Duration::from_secs(5);

    fn watch(n: u32) -> WatchId {
        format!("00000000-0000-7000-8000-{n:012}")
            .parse()
            .unwrap_or_else(|_| unreachable!("a valid id"))
    }

    #[tokio::test]
    async fn the_result_comes_back() {
        static SLOTS: Slots = Slots::new(4);
        assert_eq!(
            run_with_limit(&SLOTS, watch(1), LIMIT, || Ok("done".to_owned())).await,
            Ok("done".to_owned())
        );
        assert_eq!(
            run_with_limit(&SLOTS, watch(1), LIMIT, || Err("bad".to_owned())).await,
            Err("bad".to_owned())
        );
    }

    #[tokio::test]
    async fn a_panic_becomes_an_error() {
        static SLOTS: Slots = Slots::new(4);
        let result = run_with_limit(&SLOTS, watch(1), LIMIT, || -> Result<String, String> {
            panic!("boom")
        })
        .await;
        assert_eq!(result, Err("processing panicked".to_owned()));
        // And the slot was freed.
        assert!(
            run_with_limit(&SLOTS, watch(1), LIMIT, || Ok(String::new()))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn work_that_outlasts_the_limit_is_abandoned() {
        static SLOTS: Slots = Slots::new(4);
        let started = Instant::now();
        let result = run_with_limit(&SLOTS, watch(1), Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_secs(2));
            Ok("late".to_owned())
        })
        .await;
        assert_eq!(result, Err("processing timed out after 0s".to_owned()));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn an_abandoned_thread_keeps_its_watch_from_starting_another() {
        static SLOTS: Slots = Slots::new(4);
        let slow = || {
            std::thread::sleep(Duration::from_millis(600));
            Ok("late".to_owned())
        };
        let first = run_with_limit(&SLOTS, watch(1), Duration::from_millis(50), slow).await;
        assert!(first.is_err());
        // The thread is still alive: the same watch is refused, others are not.
        let second = run_with_limit(&SLOTS, watch(1), LIMIT, || Ok(String::new())).await;
        assert_eq!(
            second,
            Err("an earlier processing of this watch is still running".to_owned())
        );
        assert!(
            run_with_limit(&SLOTS, watch(2), LIMIT, || Ok(String::new()))
                .await
                .is_ok()
        );
        // Once the thread ends, the watch can run again.
        tokio::time::sleep(Duration::from_millis(900)).await;
        assert!(
            run_with_limit(&SLOTS, watch(1), LIMIT, || Ok(String::new()))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn the_total_number_of_live_threads_is_capped() {
        static SLOTS: Slots = Slots::new(2);
        let hold = || {
            std::thread::sleep(Duration::from_millis(600));
            Ok(String::new())
        };
        for n in 1..=2 {
            let abandoned = run_with_limit(&SLOTS, watch(n), Duration::from_millis(20), hold).await;
            assert!(abandoned.is_err());
        }
        let refused = run_with_limit(&SLOTS, watch(3), LIMIT, || Ok(String::new())).await;
        assert_eq!(
            refused,
            Err("2 processing threads are still running; not starting another".to_owned())
        );
        tokio::time::sleep(Duration::from_millis(900)).await;
        assert!(
            run_with_limit(&SLOTS, watch(3), LIMIT, || Ok(String::new()))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn the_slot_is_free_by_the_time_the_caller_hears_back() {
        static SLOTS: Slots = Slots::new(4);
        for _ in 0..200 {
            assert!(
                run_with_limit(&SLOTS, watch(1), LIMIT, || Ok(String::new()))
                    .await
                    .is_ok()
            );
        }
    }

    #[tokio::test]
    async fn deep_recursion_has_room_to_run() {
        static SLOTS: Slots = Slots::new(4);
        fn depth(n: usize) -> usize {
            if n == 0 { 0 } else { 1 + depth(n - 1) }
        }
        let result =
            run_with_limit(&SLOTS, watch(1), LIMIT, || Ok(depth(20_000).to_string())).await;
        assert_eq!(result, Ok("20000".to_owned()));
    }
}
