//! Running a pipeline on its own thread, under a timeout.
//!
//! Processing is CPU work on untrusted input (HTML, JSON, user-written jq programs). On an
//! async worker it would stall other checks, and in `spawn_blocking` a program that never
//! ends would keep the runtime from shutting down. A plain thread has neither problem: when
//! the limit passes the check fails and its slots are released, and the runtime does not wait
//! for the thread. A runaway thread keeps its CPU until the process exits; that is the price
//! of not being able to interrupt it.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use tokio::sync::oneshot;

/// Stack of the processing thread. Parsing and printing nested documents recurse, so this is
/// larger than the platform default. It is address space, not memory in use.
const STACK_BYTES: usize = 16 * 1024 * 1024;

/// Run `work` on a new thread and wait for it for at most `limit`.
///
/// A panic or a missed deadline becomes an error message; nothing here can take the caller
/// down.
pub(crate) async fn run_with_limit<F>(limit: Duration, work: F) -> Result<String, String>
where
    F: FnOnce() -> Result<String, String> + Send + 'static,
{
    let (send, receive) = oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("vigil-render".to_owned())
        .stack_size(STACK_BYTES)
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(work))
                .unwrap_or_else(|_| Err("processing panicked".to_owned()));
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

    use super::run_with_limit;

    const LIMIT: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn the_result_comes_back() {
        assert_eq!(
            run_with_limit(LIMIT, || Ok("done".to_owned())).await,
            Ok("done".to_owned())
        );
        assert_eq!(
            run_with_limit(LIMIT, || Err("bad".to_owned())).await,
            Err("bad".to_owned())
        );
    }

    #[tokio::test]
    async fn a_panic_becomes_an_error() {
        let result = run_with_limit(LIMIT, || -> Result<String, String> { panic!("boom") }).await;
        assert_eq!(result, Err("processing panicked".to_owned()));
    }

    #[tokio::test]
    async fn work_that_outlasts_the_limit_is_abandoned() {
        let started = Instant::now();
        let result = run_with_limit(Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_secs(2));
            Ok("late".to_owned())
        })
        .await;
        assert_eq!(result, Err("processing timed out after 0s".to_owned()));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn deep_recursion_has_room_to_run() {
        fn depth(n: usize) -> usize {
            if n == 0 { 0 } else { 1 + depth(n - 1) }
        }
        let result = run_with_limit(LIMIT, || Ok(depth(20_000).to_string())).await;
        assert_eq!(result, Ok("20000".to_owned()));
    }
}
