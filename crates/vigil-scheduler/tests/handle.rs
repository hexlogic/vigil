//! Asking a running scheduler for a reload or a check now, with a fake backend and a paused
//! clock.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::time::{Duration, Instant, sleep};
use tokio_util::sync::CancellationToken;
use vigil_core::WatchId;
use vigil_scheduler::{Backend, Job, RunOutcome, Scheduler, SchedulerHandle, Settings, Stopped};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DONE: RunOutcome = RunOutcome::Done {
    consecutive_errors: 0,
};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn id(n: u32) -> Result<WatchId, Box<dyn std::error::Error>> {
    Ok(format!("00000000-0000-7000-8000-{n:012}").parse()?)
}

/// A watch checked a moment ago, so that nothing is due for a long while.
fn fresh_job(n: u32, host: &str, interval: u64) -> Result<Job, Box<dyn std::error::Error>> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
    Ok(Job {
        id: id(n)?,
        host: host.into(),
        interval: secs(interval),
        jitter: Duration::ZERO,
        last_checked_at: Some(i64::try_from(now.as_millis())?),
        consecutive_errors: 0,
    })
}

/// A backend that knows `known` watches, schedules the `scheduled` ones, and records runs.
struct Fake {
    scheduled: Mutex<Vec<Job>>,
    known: Mutex<Vec<Job>>,
    runs: Mutex<Vec<(WatchId, Instant)>>,
    run_time: Duration,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl Fake {
    fn new(scheduled: Vec<Job>, known: Vec<Job>, run_time: Duration) -> Arc<Self> {
        Arc::new(Self {
            scheduled: Mutex::new(scheduled),
            known: Mutex::new(known),
            runs: Mutex::new(Vec::new()),
            run_time,
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        })
    }

    fn runs_of(&self, watch: WatchId) -> usize {
        lock(&self.runs)
            .iter()
            .filter(|(id, _)| *id == watch)
            .count()
    }
}

struct Backing(Arc<Fake>);

impl Backend for Backing {
    async fn load(&self) -> Result<Vec<Job>, String> {
        Ok(lock(&self.0.scheduled).clone())
    }

    async fn find(&self, id: WatchId) -> Result<Option<Job>, String> {
        Ok(lock(&self.0.known).iter().find(|j| j.id == id).cloned())
    }

    async fn run(&self, watch: WatchId) -> RunOutcome {
        lock(&self.0.runs).push((watch, Instant::now()));
        let active = self.0.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.0.max_active.fetch_max(active, Ordering::SeqCst);
        sleep(self.0.run_time).await;
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        DONE
    }
}

struct Running {
    handle: SchedulerHandle,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn stop(self) -> TestResult {
        self.cancel.cancel();
        Ok(self.task.await?)
    }
}

fn start(fake: &Arc<Fake>, settings: Settings) -> Running {
    let scheduler = Scheduler::new(Backing(Arc::clone(fake)), settings);
    let handle = scheduler.handle();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let task = tokio::spawn(async move { scheduler.run(token).await });
    Running {
        handle,
        cancel,
        task,
    }
}

fn hourly_resync() -> Settings {
    Settings {
        resync: secs(3600),
        shutdown_grace: secs(1),
        ..Settings::default()
    }
}

#[tokio::test(start_paused = true)]
async fn a_reload_puts_a_new_watch_on_the_schedule_at_once() -> TestResult {
    let new = Job {
        last_checked_at: None,
        ..fresh_job(1, "a:443", 100)?
    };
    let fake = Fake::new(Vec::new(), vec![new.clone()], Duration::ZERO);
    let running = start(&fake, hourly_resync());
    sleep(secs(1)).await;

    lock(&fake.scheduled).push(new);
    sleep(secs(60)).await;
    assert_eq!(fake.runs_of(id(1)?), 0, "nothing is due until the resync");

    running.handle.reload().await?;
    sleep(secs(1)).await;
    assert_eq!(fake.runs_of(id(1)?), 1);
    running.stop().await
}

#[tokio::test(start_paused = true)]
async fn a_check_now_runs_a_watch_early_and_moves_its_next_run() -> TestResult {
    let job = fresh_job(1, "a:443", 3600)?;
    let fake = Fake::new(vec![job.clone()], vec![job], Duration::ZERO);
    let running = start(&fake, hourly_resync());
    sleep(secs(1)).await;
    assert_eq!(fake.runs_of(id(1)?), 0);

    let ticket = running.handle.check_now(id(1)?).await?;
    assert_eq!(ticket.wait().await?, DONE);
    assert_eq!(fake.runs_of(id(1)?), 1);

    // The next run is an interval after the requested one, not after the old schedule.
    sleep(secs(3500)).await;
    assert_eq!(fake.runs_of(id(1)?), 1);
    sleep(secs(200)).await;
    assert_eq!(fake.runs_of(id(1)?), 2);
    running.stop().await
}

#[tokio::test(start_paused = true)]
async fn a_second_request_joins_the_check_that_is_running() -> TestResult {
    let job = fresh_job(1, "a:443", 3600)?;
    let fake = Fake::new(vec![job.clone()], vec![job], secs(10));
    let running = start(&fake, hourly_resync());
    sleep(secs(1)).await;

    let first = running.handle.check_now(id(1)?).await?;
    sleep(secs(2)).await;
    let second = running.handle.check_now(id(1)?).await?;
    assert_eq!(first.wait().await?, DONE);
    assert_eq!(second.wait().await?, DONE);
    assert_eq!(fake.runs_of(id(1)?), 1, "one check served both requests");
    running.stop().await
}

#[tokio::test(start_paused = true)]
async fn requests_wait_for_the_concurrency_limit() -> TestResult {
    let jobs = vec![fresh_job(1, "a:443", 3600)?, fresh_job(2, "b:443", 3600)?];
    let fake = Fake::new(jobs.clone(), jobs, secs(10));
    let settings = Settings {
        concurrency: 1,
        ..hourly_resync()
    };
    let running = start(&fake, settings);
    sleep(secs(1)).await;

    let started = Instant::now();
    let first = running.handle.check_now(id(1)?).await?;
    let second = running.handle.check_now(id(2)?).await?;
    first.wait().await?;
    second.wait().await?;
    assert_eq!(fake.max_active.load(Ordering::SeqCst), 1);
    assert!(started.elapsed() >= secs(20), "{:?}", started.elapsed());
    running.stop().await
}

#[tokio::test(start_paused = true)]
async fn a_watch_that_is_not_scheduled_is_checked_once_per_request() -> TestResult {
    let paused = fresh_job(1, "a:443", 100)?;
    let fake = Fake::new(Vec::new(), vec![paused], Duration::ZERO);
    let running = start(&fake, hourly_resync());
    sleep(secs(1)).await;

    assert_eq!(running.handle.check_now(id(1)?).await?.wait().await?, DONE);
    assert_eq!(fake.runs_of(id(1)?), 1);
    // It was not put on the schedule by the request.
    sleep(secs(36_000)).await;
    assert_eq!(fake.runs_of(id(1)?), 1);

    running.handle.check_now(id(1)?).await?.wait().await?;
    assert_eq!(fake.runs_of(id(1)?), 2);
    running.stop().await
}

#[tokio::test(start_paused = true)]
async fn a_watch_that_does_not_exist_ends_as_gone_without_running() -> TestResult {
    let fake = Fake::new(Vec::new(), Vec::new(), Duration::ZERO);
    let running = start(&fake, hourly_resync());
    sleep(secs(1)).await;

    let outcome = running.handle.check_now(id(9)?).await?.wait().await?;
    assert_eq!(outcome, RunOutcome::Gone);
    assert_eq!(fake.runs_of(id(9)?), 0);
    running.stop().await
}

#[tokio::test(start_paused = true)]
async fn a_watch_resumed_during_a_requested_check_is_not_started_twice() -> TestResult {
    let job = Job {
        last_checked_at: None,
        ..fresh_job(1, "a:443", 100)?
    };
    let fake = Fake::new(Vec::new(), vec![job.clone()], secs(10));
    let running = start(&fake, hourly_resync());
    sleep(secs(1)).await;

    let ticket = running.handle.check_now(id(1)?).await?;
    sleep(secs(1)).await;
    // Resumed while the requested check is still running: it is due, but must wait.
    lock(&fake.scheduled).push(job);
    running.handle.reload().await?;
    ticket.wait().await?;
    sleep(secs(50)).await;
    assert_eq!(fake.runs_of(id(1)?), 1);

    // The schedule takes over from the end of the requested check.
    sleep(secs(70)).await;
    assert_eq!(fake.runs_of(id(1)?), 2);
    running.stop().await
}

#[tokio::test(start_paused = true)]
async fn requests_fail_once_the_scheduler_has_stopped() -> TestResult {
    let fake = Fake::new(Vec::new(), Vec::new(), Duration::ZERO);
    let running = start(&fake, hourly_resync());
    let handle = running.handle.clone();
    sleep(secs(1)).await;
    running.stop().await?;

    assert_eq!(handle.reload().await, Err(Stopped));
    assert!(matches!(handle.check_now(id(1)?).await, Err(Stopped)));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_ticket_waiting_at_shutdown_learns_that_the_scheduler_stopped() -> TestResult {
    let job = fresh_job(1, "a:443", 3600)?;
    let fake = Fake::new(vec![job.clone()], vec![job], secs(600));
    let running = start(&fake, hourly_resync());
    sleep(secs(1)).await;

    let ticket = running.handle.check_now(id(1)?).await?;
    sleep(secs(1)).await;
    running.stop().await?;
    assert_eq!(ticket.wait().await, Err(Stopped));
    Ok(())
}
