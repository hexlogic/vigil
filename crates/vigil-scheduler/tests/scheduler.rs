//! Scheduler behaviour with a fake backend and a paused clock.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::time::{Duration, Instant, sleep};
use tokio_util::sync::CancellationToken;
use vigil_core::WatchId;
use vigil_scheduler::{Backend, Job, RunOutcome, Scheduler, Settings};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn id(n: u32) -> Result<WatchId, Box<dyn std::error::Error>> {
    Ok(format!("00000000-0000-7000-8000-{n:012}").parse()?)
}

fn job(n: u32, host: &str, interval: u64) -> Result<Job, Box<dyn std::error::Error>> {
    Ok(Job {
        id: id(n)?,
        host: host.into(),
        interval: secs(interval),
        jitter: Duration::ZERO,
        last_checked_at: None,
        consecutive_errors: 0,
    })
}

fn now_ms() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(elapsed.as_millis()).unwrap_or(0)
}

type OutcomeFn = Box<dyn Fn(usize) -> RunOutcome + Send + Sync>;

/// Records every run and lets a test script the outcomes.
struct Fake {
    jobs: Mutex<Vec<Job>>,
    runs: Mutex<Vec<(WatchId, Instant)>>,
    run_time: Duration,
    outcome: OutcomeFn,
    panic_on_first_run: bool,
    failing_loads: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
    finished: AtomicUsize,
}

impl Fake {
    fn new(jobs: Vec<Job>) -> Self {
        Self {
            jobs: Mutex::new(jobs),
            runs: Mutex::new(Vec::new()),
            run_time: Duration::ZERO,
            outcome: Box::new(|_| RunOutcome::Done {
                consecutive_errors: 0,
            }),
            panic_on_first_run: false,
            failing_loads: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            finished: AtomicUsize::new(0),
        }
    }

    fn runs_of(&self, watch: WatchId) -> Vec<Instant> {
        lock(&self.runs)
            .iter()
            .filter(|(id, _)| *id == watch)
            .map(|(_, at)| *at)
            .collect()
    }

    fn total_runs(&self) -> usize {
        lock(&self.runs).len()
    }
}

struct Handle(Arc<Fake>);

impl Backend for Handle {
    async fn load(&self) -> Result<Vec<Job>, String> {
        let remaining = self.0.failing_loads.load(Ordering::SeqCst);
        if remaining > 0 {
            self.0.failing_loads.store(remaining - 1, Ordering::SeqCst);
            return Err("database is locked".into());
        }
        Ok(lock(&self.0.jobs).clone())
    }

    async fn run(&self, watch: WatchId) -> RunOutcome {
        let fake = &self.0;
        let nth = {
            let mut runs = lock(&fake.runs);
            runs.push((watch, Instant::now()));
            runs.iter().filter(|(id, _)| *id == watch).count()
        };
        let active = fake.active.fetch_add(1, Ordering::SeqCst) + 1;
        fake.max_active.fetch_max(active, Ordering::SeqCst);
        if fake.panic_on_first_run && nth == 1 {
            fake.active.fetch_sub(1, Ordering::SeqCst);
            panic!("simulated crash in check");
        }
        sleep(fake.run_time).await;
        fake.active.fetch_sub(1, Ordering::SeqCst);
        fake.finished.fetch_add(1, Ordering::SeqCst);
        let outcome = (fake.outcome)(nth);
        if outcome == RunOutcome::Gone {
            lock(&fake.jobs).retain(|j| j.id != watch);
        }
        outcome
    }
}

fn start(fake: &Arc<Fake>, settings: Settings) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let scheduler = Scheduler::new(Handle(Arc::clone(fake)), settings);
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    (
        cancel,
        tokio::spawn(async move { scheduler.run(token).await }),
    )
}

fn gaps(runs: &[Instant]) -> Vec<Duration> {
    runs.windows(2).map(|pair| pair[1] - pair[0]).collect()
}

#[tokio::test(start_paused = true)]
async fn runs_a_watch_on_its_interval() -> TestResult {
    let fake = Arc::new(Fake::new(vec![job(1, "a:443", 100)?]));
    let (cancel, handle) = start(&fake, Settings::default());

    sleep(secs(350)).await;
    let runs = fake.runs_of(id(1)?);
    assert_eq!(runs.len(), 4);
    assert_eq!(gaps(&runs), [secs(100); 3]);

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn failures_back_off_exponentially_and_success_resets() -> TestResult {
    let mut fake = Fake::new(vec![job(1, "a:443", 10)?]);
    // Runs 1-3 fail with a growing streak, run 4 succeeds.
    fake.outcome = Box::new(|nth| RunOutcome::Done {
        consecutive_errors: if nth <= 3 {
            u32::try_from(nth).unwrap_or(0)
        } else {
            0
        },
    });
    let fake = Arc::new(fake);
    let (cancel, handle) = start(&fake, Settings::default());

    sleep(secs(155)).await;
    let runs = fake.runs_of(id(1)?);
    assert_eq!(gaps(&runs), [secs(20), secs(40), secs(80), secs(10)]);

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failure_to_record_counts_as_one_more_error() -> TestResult {
    let mut fake = Fake::new(vec![job(1, "a:443", 10)?]);
    fake.outcome = Box::new(|_| RunOutcome::Failed);
    let fake = Arc::new(fake);
    let (cancel, handle) = start(&fake, Settings::default());

    sleep(secs(75)).await;
    assert_eq!(gaps(&fake.runs_of(id(1)?)), [secs(20), secs(40)]);

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn global_limit_caps_parallel_checks() -> TestResult {
    let jobs = (1..=5)
        .map(|n| job(n, &format!("host{n}:443"), 1000))
        .collect::<Result<_, _>>()?;
    let mut fake = Fake::new(jobs);
    fake.run_time = secs(10);
    let fake = Arc::new(fake);
    let settings = Settings {
        concurrency: 2,
        per_host: 2,
        ..Settings::default()
    };
    let (cancel, handle) = start(&fake, settings);

    sleep(secs(100)).await;
    assert_eq!(fake.total_runs(), 5);
    assert_eq!(fake.max_active.load(Ordering::SeqCst), 2);

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn per_host_limit_caps_parallel_checks_on_one_host() -> TestResult {
    let jobs = (1..=4)
        .map(|n| job(n, "same:443", 1000))
        .collect::<Result<_, _>>()?;
    let mut fake = Fake::new(jobs);
    fake.run_time = secs(10);
    let fake = Arc::new(fake);
    let settings = Settings {
        concurrency: 8,
        per_host: 2,
        ..Settings::default()
    };
    let (cancel, handle) = start(&fake, settings);

    sleep(secs(100)).await;
    assert_eq!(fake.total_runs(), 4);
    assert_eq!(fake.max_active.load(Ordering::SeqCst), 2);

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn busy_host_does_not_block_other_hosts() -> TestResult {
    // Two watches on one host with a limit of 1, plus one on another host: the other host's
    // check must start right away instead of queueing behind the busy one.
    let jobs = vec![
        job(1, "slow:443", 1000)?,
        job(2, "slow:443", 1000)?,
        job(3, "fast:443", 1000)?,
    ];
    let mut fake = Fake::new(jobs);
    fake.run_time = secs(50);
    let fake = Arc::new(fake);
    let settings = Settings {
        concurrency: 2,
        per_host: 1,
        ..Settings::default()
    };
    let started = Instant::now();
    let (cancel, handle) = start(&fake, settings);

    sleep(secs(5)).await;
    let fast = fake.runs_of(id(3)?);
    assert_eq!(fast.len(), 1);
    assert!(fast[0] - started < secs(1));
    assert_eq!(fake.max_active.load(Ordering::SeqCst), 2);

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn removed_watches_stop_being_scheduled() -> TestResult {
    let fake = Arc::new(Fake::new(vec![job(1, "a:443", 100)?]));
    let (cancel, handle) = start(&fake, Settings::default());

    sleep(secs(5)).await;
    assert_eq!(fake.total_runs(), 1);
    lock(&fake.jobs).clear();
    sleep(secs(300)).await;
    assert_eq!(fake.total_runs(), 1);

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn new_watches_are_picked_up_on_resync() -> TestResult {
    let fake = Arc::new(Fake::new(Vec::new()));
    let (cancel, handle) = start(&fake, Settings::default());

    sleep(secs(1)).await;
    lock(&fake.jobs).push(job(1, "a:443", 1000)?);
    sleep(secs(31)).await;
    assert_eq!(fake.total_runs(), 1);

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn watches_reported_gone_are_dropped() -> TestResult {
    let mut fake = Fake::new(vec![job(1, "a:443", 10)?]);
    fake.outcome = Box::new(|_| RunOutcome::Gone);
    let fake = Arc::new(fake);
    let (cancel, handle) = start(&fake, Settings::default());

    sleep(secs(300)).await;
    assert_eq!(fake.total_runs(), 1);

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failing_reload_is_retried_at_the_next_resync() -> TestResult {
    let fake = Fake::new(vec![job(1, "a:443", 1000)?]);
    fake.failing_loads.store(1, Ordering::SeqCst);
    let fake = Arc::new(fake);
    let (cancel, handle) = start(&fake, Settings::default());

    sleep(secs(10)).await;
    assert_eq!(fake.total_runs(), 0);
    sleep(secs(25)).await;
    assert_eq!(fake.total_runs(), 1);

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_recently_checked_watch_waits_out_its_interval() -> TestResult {
    let mut recent = job(1, "a:443", 100)?;
    recent.last_checked_at = Some(now_ms() - 10_000);
    let mut overdue = job(2, "b:443", 100)?;
    overdue.last_checked_at = Some(now_ms() - 500_000);
    let fake = Arc::new(Fake::new(vec![recent, overdue]));
    let started = Instant::now();
    let (cancel, handle) = start(&fake, Settings::default());

    sleep(secs(120)).await;
    let overdue_runs = fake.runs_of(id(2)?);
    assert!(overdue_runs[0] - started < secs(1));
    let recent_runs = fake.runs_of(id(1)?);
    let wait = recent_runs[0] - started;
    assert!(wait >= secs(89) && wait <= secs(91), "waited {wait:?}");

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_zero_interval_cannot_spin() -> TestResult {
    let fake = Arc::new(Fake::new(vec![job(1, "a:443", 0)?]));
    let (cancel, handle) = start(&fake, Settings::default());

    sleep(Duration::from_millis(5_500)).await;
    let runs = fake.runs_of(id(1)?);
    assert_eq!(runs.len(), 6);
    assert_eq!(gaps(&runs), [secs(1); 5]);

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn jitter_delays_each_run_within_its_bound() -> TestResult {
    let mut jittery = job(1, "a:443", 100)?;
    jittery.jitter = secs(10);
    let fake = Arc::new(Fake::new(vec![jittery]));
    let (cancel, handle) = start(&fake, Settings::default());

    sleep(secs(1_000)).await;
    let runs = fake.runs_of(id(1)?);
    assert!(runs.len() >= 9);
    for gap in gaps(&runs) {
        assert!(gap >= secs(100) && gap <= secs(110), "gap {gap:?}");
    }

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_panicking_check_is_rescheduled_with_backoff() -> TestResult {
    let mut fake = Fake::new(vec![job(1, "a:443", 10)?]);
    fake.panic_on_first_run = true;
    let fake = Arc::new(fake);
    let (cancel, handle) = start(&fake, Settings::default());

    sleep(secs(25)).await;
    let runs = fake.runs_of(id(1)?);
    assert_eq!(runs.len(), 2);
    assert_eq!(gaps(&runs), [secs(20)]);

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn shutdown_waits_for_running_checks() -> TestResult {
    let mut fake = Fake::new(vec![job(1, "a:443", 1000)?]);
    fake.run_time = secs(5);
    let fake = Arc::new(fake);
    let started = Instant::now();
    let (cancel, handle) = start(&fake, Settings::default());

    sleep(secs(1)).await;
    cancel.cancel();
    handle.await?;
    assert_eq!(fake.finished.load(Ordering::SeqCst), 1);
    assert!(started.elapsed() >= secs(5));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn shutdown_gives_up_on_checks_after_the_grace_period() -> TestResult {
    let mut fake = Fake::new(vec![job(1, "a:443", 1000)?]);
    fake.run_time = secs(1000);
    let fake = Arc::new(fake);
    let settings = Settings {
        shutdown_grace: secs(2),
        ..Settings::default()
    };
    let started = Instant::now();
    let (cancel, handle) = start(&fake, settings);

    sleep(secs(1)).await;
    cancel.cancel();
    handle.await?;
    assert_eq!(fake.finished.load(Ordering::SeqCst), 0);
    assert!(started.elapsed() < secs(10));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn no_new_checks_start_after_cancellation() -> TestResult {
    let fake = Arc::new(Fake::new(vec![job(1, "a:443", 10)?]));
    let (cancel, handle) = start(&fake, Settings::default());

    sleep(secs(5)).await;
    cancel.cancel();
    handle.await?;
    let before = fake.total_runs();
    sleep(secs(100)).await;
    assert_eq!(fake.total_runs(), before);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn checks_still_queued_at_shutdown_never_start() -> TestResult {
    // One slot, three due watches: one runs, two wait for the slot.
    let jobs = (1..=3)
        .map(|n| job(n, &format!("host{n}:443"), 1000))
        .collect::<Result<_, _>>()?;
    let mut fake = Fake::new(jobs);
    fake.run_time = secs(10);
    let fake = Arc::new(fake);
    let settings = Settings {
        concurrency: 1,
        per_host: 1,
        ..Settings::default()
    };
    let started = Instant::now();
    let (cancel, handle) = start(&fake, settings);

    sleep(secs(1)).await;
    assert_eq!(fake.total_runs(), 1);
    cancel.cancel();
    handle.await?;

    // Only the running check was waited for; the queued ones were dropped, not started.
    assert_eq!(fake.total_runs(), 1);
    assert_eq!(fake.finished.load(Ordering::SeqCst), 1);
    assert!(
        started.elapsed() < secs(15),
        "shutdown took {:?}",
        started.elapsed()
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_absurdly_long_interval_does_not_crash_the_scheduler() -> TestResult {
    let mut forever = job(1, "a:443", 100)?;
    forever.interval = Duration::MAX;
    forever.jitter = secs(5);
    let fake = Arc::new(Fake::new(vec![forever, job(2, "b:443", 100)?]));
    let (cancel, handle) = start(&fake, Settings::default());

    sleep(secs(250)).await;
    assert!(!handle.is_finished(), "the scheduler task died");
    assert_eq!(fake.runs_of(id(1)?).len(), 1);
    // Other watches keep running.
    assert_eq!(fake.runs_of(id(2)?).len(), 3);

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_check_time_in_the_future_does_not_stall_a_watch() -> TestResult {
    // Recorded while the wall clock was a day ahead of where it is now.
    let mut skewed = job(1, "a:443", 100)?;
    skewed.last_checked_at = Some(now_ms() + 86_400_000);
    let fake = Arc::new(Fake::new(vec![skewed]));
    let (cancel, handle) = start(&fake, Settings::default());

    sleep(secs(110)).await;
    assert_eq!(fake.runs_of(id(1)?).len(), 1);

    cancel.cancel();
    handle.await?;
    Ok(())
}

fn settings_with_resync(resync: u64) -> Settings {
    Settings {
        resync: secs(resync),
        ..Settings::default()
    }
}

#[tokio::test(start_paused = true)]
async fn a_shorter_interval_takes_effect_at_the_next_reload() -> TestResult {
    // Checked a minute ago, once a day: nothing is due for almost a day.
    let mut daily = job(1, "a", 86_400)?;
    daily.last_checked_at = Some(now_ms() - 60_000);
    let fake = Arc::new(Fake::new(vec![daily.clone()]));
    let (cancel, handle) = start(&fake, settings_with_resync(10));
    sleep(secs(5)).await;
    assert_eq!(fake.total_runs(), 0);

    // The user edits it to every two minutes. The last check was a minute ago, so the next
    // one is a minute away, not a day.
    let started = Instant::now();
    daily.interval = secs(120);
    daily.last_checked_at = Some(now_ms() - 60_000);
    *lock(&fake.jobs) = vec![daily];
    sleep(secs(90)).await;
    let runs = fake.runs_of(id(1)?);
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert!(
        runs[0] - started < secs(75),
        "ran after {:?}",
        runs[0] - started
    );

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn clearing_the_failure_streak_ends_the_backoff() -> TestResult {
    // Five failures in a row: the next check is 32 intervals away.
    let mut failing = job(1, "a", 60)?;
    failing.last_checked_at = Some(now_ms() - 10_000);
    failing.consecutive_errors = 5;
    let fake = Arc::new(Fake::new(vec![failing.clone()]));
    let (cancel, handle) = start(&fake, settings_with_resync(10));
    sleep(secs(100)).await;
    assert_eq!(fake.total_runs(), 0);

    // An edit resets the streak, so the watch is back on its ordinary schedule.
    failing.consecutive_errors = 0;
    failing.last_checked_at = Some(now_ms() - 10_000);
    *lock(&fake.jobs) = vec![failing];
    sleep(secs(60)).await;
    assert_eq!(fake.runs_of(id(1)?).len(), 1);

    cancel.cancel();
    handle.await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_unchanged_watch_keeps_its_place_in_the_schedule() -> TestResult {
    let fake = Arc::new(Fake::new(vec![job(1, "a", 300)?]));
    let (cancel, handle) = start(&fake, settings_with_resync(10));
    // Reloads every 10 seconds must not push the checks around.
    sleep(secs(1_000)).await;
    let runs = fake.runs_of(id(1)?);
    assert_eq!(gaps(&runs), vec![secs(300); runs.len() - 1]);
    cancel.cancel();
    handle.await?;
    Ok(())
}
