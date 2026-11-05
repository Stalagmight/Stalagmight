//! [#576 FZ-INT] A deterministic, seeded, lock-granular thread scheduler for
//! interleaving fuzzers. Test builds only (`test-support`).
//!
//! # Why
//!
//! Real threads interleave however the OS likes, so a concurrency test that fails
//! once cannot be replayed, and one that passes says little. Every lock the backend
//! takes is a [`RecoveringMutex`](crate::poison_policy::RecoveringMutex) (#576
//! A11), so its `lock()` is a natural SCHEDULING POINT: a thread registered with a
//! [`run`] here stops at every lock it takes, and the scheduler - one seeded RNG -
//! decides which thread runs next. Exactly one registered thread runs at a time
//! (the baton), so a seed (or a recorded choice list) replays the same
//! interleaving every time.
//!
//! # How a lock is taken under the scheduler
//!
//! [`scheduled_lock`] first yields (the scheduler may switch threads here), then
//! acquires with `try_lock`. A contended lock does not block in the OS: the thread
//! yields again as BLOCKED on that cell and is retried only after some other thread
//! made progress (took a lock, started or finished). When every live thread is
//! blocked and none can progress, that is a DEADLOCK, found deterministically and
//! reported with the cells each thread waits on. A deadlocked (or over-long) run is
//! aborted by unwinding each blocked thread out of its wait.
//!
//! Threads that are not registered (every thread outside a [`run`]: the rest of the
//! test suite, helper threads a command spawns) take the ordinary blocking path; the
//! check costs one atomic load while no run is active. Locks other than
//! `RecoveringMutex` (std atomics, SQLite's own file locks) are not scheduling
//! points: the interleavings explored are those of the backend's lock cells.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LockResult, Mutex, MutexGuard, PoisonError, TryLockError};
use std::time::{Duration, Instant};

/// How many [`run`]s are in progress anywhere in the process. Zero means every
/// `lock()` takes the plain path without looking at the thread-local.
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// The run this thread is registered with, and its index in it.
    static CURRENT: RefCell<Option<(Arc<Shared>, usize)>> = const { RefCell::new(None) };
}

/// The payload a run's abort unwinds a blocked thread with.
struct Aborted;

/// splitmix64: tiny, seedable, and good enough to pick threads.
#[derive(Debug, Clone)]
pub struct SplitMix(u64);

impl SplitMix {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform-ish in `0..bound`; 0 for an empty range.
    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        usize::try_from(self.next_u64() % (bound as u64)).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    /// Waiting for its first turn, or stopped at a scheduling point.
    Runnable,
    /// Failed to take `cell` when the progress counter stood at `stuck_at`.
    Blocked {
        cell: &'static str,
        stuck_at: u64,
    },
    Done,
}

/// How a run picks the next thread when it is not replaying.
#[derive(Debug, Clone)]
enum Strategy {
    /// A biased random walk: at every point the running thread keeps the baton
    /// with probability one half, else a uniformly random candidate gets it.
    /// Good at short races between nearby lock points.
    Walk,
    /// PCT-style: every thread has a random priority and the highest-ranked
    /// candidate always runs; at a few random points the running thread drops to
    /// the bottom. Good at bugs that need one thread to run a long stretch while
    /// another is parked at one exact point (a load overtaking another).
    Priorities {
        rank: Vec<i64>,
        change_at: Vec<usize>,
    },
}

/// How far into a run the priority change points are drawn.
const CHANGE_HORIZON: usize = 400;

struct Inner {
    strategy: Strategy,
    status: Vec<Status>,
    current: Option<usize>,
    /// Bumped whenever a thread starts, takes a lock or finishes.
    progress: u64,
    rng: SplitMix,
    replay: Vec<u32>,
    choices: Vec<u32>,
    trace: Vec<(usize, &'static str, Option<usize>)>,
    /// `(thread, cell)` for every lock taken, in the order taken.
    acquired: Vec<(usize, &'static str)>,
    max_steps: usize,
    abort: Option<String>,
}

struct Shared {
    inner: Mutex<Inner>,
    turn: Condvar,
}

/// Why a run did not complete normally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Every live thread blocked on a cell another one holds.
    Deadlock(String),
    /// More scheduling points than the run allows (a livelock, or a run too long).
    StepLimit(usize),
    /// A thread stopped outside the scheduler (blocked in the OS) for longer than
    /// the watchdog allows: not attributable to a lock cell.
    Stalled(String),
}

/// What a [`run`] did: enough to replay it ([`Outcome::choices`]) and to read it.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// The thread the scheduler handed the baton to at every point. Feeding them
    /// back as `replay` reproduces the run.
    pub choices: Vec<u32>,
    /// `(thread, cell, next)` for every scheduling point, in order: `thread`
    /// stopped before taking `cell` (`<done>` when it finished) and `next` got the
    /// baton.
    pub trace: Vec<(usize, &'static str, Option<usize>)>,
    /// `(thread, cell)` for every lock a registered thread took, in the order
    /// taken (a scheduling point records the stop BEFORE a lock; this records the
    /// acquisition). A worker marks its own windows in it with [`acquired_so_far`].
    pub acquired: Vec<(usize, &'static str)>,
    pub failure: Option<Failure>,
    /// Workers whose body panicked (not the abort), with the message.
    pub panics: Vec<(usize, String)>,
}

impl Outcome {
    /// The last `n` scheduling points, one per line.
    pub fn trace_tail(&self, n: usize) -> String {
        let mut out = String::new();
        let start = self.trace.len().saturating_sub(n);
        for (i, (thread, cell, next)) in self.trace.iter().enumerate().skip(start) {
            let _ = match next {
                Some(next) if next != thread => {
                    writeln!(out, "  #{i}: t{thread} @ {cell}  -> switch to t{next}")
                }
                _ => writeln!(out, "  #{i}: t{thread} @ {cell}"),
            };
        }
        out
    }
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Pick who runs next, from `me`'s point of view, and hand it the baton.
    fn hand_over(&self, inner: &mut Inner, me: Option<usize>) {
        if inner.abort.is_some() {
            return;
        }
        let progress = inner.progress;
        let candidate = |s: &Status| match *s {
            Status::Runnable => true,
            Status::Blocked { stuck_at, .. } => stuck_at < progress,
            Status::Done => false,
        };
        let mut order: Vec<usize> = Vec::with_capacity(inner.status.len());
        if let Some(me) = me {
            if inner.status.get(me).is_some_and(candidate) {
                order.push(me);
            }
        }
        for (i, s) in inner.status.iter().enumerate() {
            if Some(i) != me && candidate(s) {
                order.push(i);
            }
        }
        if order.is_empty() {
            inner.current = None;
            if inner.status.iter().any(|s| *s != Status::Done) {
                let mut why = String::from("every live thread is blocked:");
                for (i, s) in inner.status.iter().enumerate() {
                    if let Status::Blocked { cell, .. } = s {
                        let _ = write!(why, " t{i} waits for `{cell}`;");
                    }
                }
                inner.abort = Some(why);
            }
            self.turn.notify_all();
            return;
        }
        if inner.choices.len() >= inner.max_steps {
            inner.abort = Some(format!("step limit {} reached", inner.max_steps));
            inner.current = None;
            self.turn.notify_all();
            return;
        }
        let at = inner.choices.len();
        let len = order.len();
        if let (Some(me), Strategy::Priorities { rank, change_at }) = (me, &mut inner.strategy) {
            // A change point: the running thread drops below every other one.
            if change_at.contains(&at) {
                let lowest = rank.iter().copied().min().unwrap_or(0);
                if let Some(r) = rank.get_mut(me) {
                    *r = lowest - 1;
                }
            }
        }
        let pick = match (inner.replay.get(at), &inner.strategy) {
            // A replayed choice names a thread; one that cannot run here keeps the
            // running thread (the first candidate) going.
            (Some(&wanted), _) => order
                .iter()
                .position(|&t| u32::try_from(t).ok() == Some(wanted))
                .unwrap_or(0),
            (None, Strategy::Priorities { rank, .. }) => order
                .iter()
                .enumerate()
                .max_by_key(|(_, &t)| rank.get(t).copied().unwrap_or(i64::MIN))
                .map_or(0, |(i, _)| i),
            // Half the time the running thread keeps going: long runs of one thread
            // with occasional switches reach deeper states than a coin per lock.
            (None, Strategy::Walk) if inner.rng.below(2) == 0 => 0,
            (None, Strategy::Walk) => inner.rng.below(len),
        };
        inner.current = order.get(pick).copied();
        let chosen = inner
            .current
            .and_then(|t| u32::try_from(t).ok())
            .unwrap_or(0);
        inner.choices.push(chosen);
        self.turn.notify_all();
    }

    /// Wait until it is `me`'s turn (or the run aborted).
    fn wait_turn<'a>(&self, mut inner: MutexGuard<'a, Inner>, me: usize) -> MutexGuard<'a, Inner> {
        while inner.current != Some(me) && inner.abort.is_none() {
            inner = self
                .turn
                .wait(inner)
                .unwrap_or_else(PoisonError::into_inner);
        }
        inner
    }

    /// A scheduling point: record it, let the scheduler choose, wait for the baton.
    /// Returns whether the run is aborting.
    fn yield_at(&self, me: usize, cell: &'static str, blocked: bool) -> bool {
        let mut inner = self.state();
        if inner.abort.is_some() {
            return true;
        }
        let progress = inner.progress;
        if let Some(s) = inner.status.get_mut(me) {
            *s = if blocked {
                Status::Blocked {
                    cell,
                    stuck_at: progress,
                }
            } else {
                Status::Runnable
            };
        }
        self.hand_over(&mut inner, Some(me));
        let next = inner.current;
        inner.trace.push((me, cell, next));
        let inner = self.wait_turn(inner, me);
        inner.abort.is_some()
    }

    fn progressed(&self, me: usize, took: Option<&'static str>) {
        let mut inner = self.state();
        inner.progress += 1;
        if let Some(cell) = took {
            inner.acquired.push((me, cell));
        }
        if let Some(s) = inner.status.get_mut(me) {
            *s = Status::Runnable;
        }
    }

    fn finish(&self, me: usize) {
        let mut inner = self.state();
        inner.progress += 1;
        if let Some(s) = inner.status.get_mut(me) {
            *s = Status::Done;
        }
        if inner.current == Some(me) {
            self.hand_over(&mut inner, None);
            let next = inner.current;
            inner.trace.push((me, "<done>", next));
        }
        self.turn.notify_all();
    }
}

/// The scheduler's side of `RecoveringMutex::lock`: `None` when this thread is not
/// part of a [`run`] (take the plain path), else the lock result taken under the
/// schedule. See the module docs.
pub fn scheduled_lock<'a, T: ?Sized>(
    cell: &'static str,
    mutex: &'a Mutex<T>,
) -> Option<LockResult<MutexGuard<'a, T>>> {
    if ACTIVE.load(Ordering::Acquire) == 0 {
        return None;
    }
    let (shared, me) = CURRENT.try_with(|c| c.borrow().clone()).ok().flatten()?;
    let mut aborting = shared.yield_at(me, cell, false);
    loop {
        if aborting {
            // Unwind out of the run; a lock taken while already unwinding (a Drop)
            // waits plainly - the other aborted threads are unwinding too.
            if std::thread::panicking() {
                return Some(mutex.lock());
            }
            panic::resume_unwind(Box::new(Aborted));
        }
        match mutex.try_lock() {
            Ok(guard) => {
                shared.progressed(me, Some(cell));
                return Some(Ok(guard));
            }
            Err(TryLockError::Poisoned(poisoned)) => {
                shared.progressed(me, Some(cell));
                return Some(Err(poisoned));
            }
            Err(TryLockError::WouldBlock) => aborting = shared.yield_at(me, cell, true),
        }
    }
}

/// How many locks the calling thread's run has recorded as taken so far (see
/// [`Outcome::acquired`]); `None` outside a run. A worker brackets an operation
/// with two reads to find that operation's acquisitions afterwards.
pub fn acquired_so_far() -> Option<usize> {
    let (shared, _) = CURRENT.try_with(|c| c.borrow().clone()).ok().flatten()?;
    let count = shared.state().acquired.len();
    Some(count)
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic".to_owned())
}

/// Decrements [`ACTIVE`] however [`run`] exits.
struct ActiveRun;

impl ActiveRun {
    fn start() -> Self {
        ACTIVE.fetch_add(1, Ordering::AcqRel);
        Self
    }
}

impl Drop for ActiveRun {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A worker body: runs on its own registered thread.
pub type Worker = Box<dyn FnOnce() + Send + 'static>;

/// Run `workers` on their own threads under one schedule chosen by `seed` (or
/// replayed from `replay`, an [`Outcome::choices`] list: each choice names the
/// thread to run, and one that cannot run at that point keeps the running thread
/// going, so a list recorded for one op set still steers a shrunk one; past its
/// end the seed decides). At most `max_steps` scheduling points. `watchdog` bounds the wall
/// time a run may take; a run past it is reported [`Failure::Stalled`] and its
/// threads are left behind (they are blocked outside the scheduler).
pub fn run(
    seed: u64,
    replay: &[u32],
    max_steps: usize,
    watchdog: Duration,
    workers: Vec<Worker>,
) -> Outcome {
    let _active = ActiveRun::start();
    let count = workers.len();
    let mut rng = SplitMix::new(seed);
    // One seed in three walks; the rest rank the threads (see `Strategy`).
    let strategy = if rng.below(3) == 0 {
        Strategy::Walk
    } else {
        let mut rank: Vec<i64> = (0..count).map(|i| i64::try_from(i).unwrap_or(0)).collect();
        for i in (1..rank.len()).rev() {
            rank.swap(i, rng.below(i + 1));
        }
        let depth = 1 + rng.below(3);
        let change_at = (0..depth).map(|_| rng.below(CHANGE_HORIZON)).collect();
        Strategy::Priorities { rank, change_at }
    };
    let shared = Arc::new(Shared {
        inner: Mutex::new(Inner {
            strategy,
            status: vec![Status::Runnable; count],
            current: None,
            progress: 0,
            rng,
            replay: replay.to_vec(),
            choices: Vec::new(),
            trace: Vec::new(),
            acquired: Vec::new(),
            max_steps,
            abort: None,
        }),
        turn: Condvar::new(),
    });
    let panics: Arc<Mutex<Vec<(usize, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::with_capacity(count);
    for (me, body) in workers.into_iter().enumerate() {
        let shared = Arc::clone(&shared);
        let panics = Arc::clone(&panics);
        let spawned = std::thread::Builder::new()
            .name(format!("interleave-t{me}"))
            .spawn(move || {
                CURRENT.with(|c| *c.borrow_mut() = Some((Arc::clone(&shared), me)));
                let aborted = {
                    let inner = shared.wait_turn(shared.state(), me);
                    inner.abort.is_some()
                };
                if !aborted {
                    shared.progressed(me, None);
                    if let Err(payload) = panic::catch_unwind(AssertUnwindSafe(body)) {
                        if !payload.is::<Aborted>() {
                            panics
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .push((me, panic_message(payload.as_ref())));
                        }
                    }
                }
                CURRENT.with(|c| *c.borrow_mut() = None);
                shared.finish(me);
            });
        if let Ok(handle) = spawned {
            handles.push(handle);
        }
    }
    {
        let mut inner = shared.state();
        shared.hand_over(&mut inner, None);
    }
    let deadline = Instant::now() + watchdog;
    let mut stalled = None;
    {
        let mut inner = shared.state();
        while inner.status.iter().any(|s| *s != Status::Done) {
            let now = Instant::now();
            if now >= deadline {
                let mut why = String::from("threads still running:");
                for (i, s) in inner.status.iter().enumerate() {
                    if *s != Status::Done {
                        let _ = write!(why, " t{i}={s:?}");
                    }
                }
                inner.abort = Some(why.clone());
                shared.turn.notify_all();
                stalled = Some(why);
                break;
            }
            let (next, _) = shared
                .turn
                .wait_timeout(inner, deadline - now)
                .unwrap_or_else(PoisonError::into_inner);
            inner = next;
        }
    }
    if stalled.is_none() {
        for handle in handles {
            let _ = handle.join();
        }
    }
    let panicked = panics
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let inner = shared.state();
    let failure = match (&stalled, &inner.abort) {
        (Some(why), _) => Some(Failure::Stalled(why.clone())),
        (None, Some(why)) if why.starts_with("step limit") => Some(Failure::StepLimit(max_steps)),
        (None, Some(why)) => Some(Failure::Deadlock(why.clone())),
        (None, None) => None,
    };
    Outcome {
        choices: inner.choices.clone(),
        trace: inner.trace.clone(),
        acquired: inner.acquired.clone(),
        failure,
        panics: panicked,
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test module: see docs/linting.md for the per-test-module expect idiom; one \
              worker panics ON PURPOSE"
)]
mod tests {
    use super::{run, Failure, Worker};
    use crate::poison_policy::RecoveringMutex;
    use std::sync::Arc;
    use std::time::Duration;

    const WATCHDOG: Duration = Duration::from_secs(30);

    /// Two threads appending to one cell under several locks: the final order is a
    /// pure function of the seed.
    fn interleaved(seed: u64, replay: &[u32]) -> (Vec<u32>, Vec<u32>) {
        let log = Arc::new(RecoveringMutex::new("log", Vec::new()));
        let other = Arc::new(RecoveringMutex::new("other", ()));
        let workers: Vec<Worker> = (0..3u32)
            .map(|t| {
                let log = Arc::clone(&log);
                let other = Arc::clone(&other);
                Box::new(move || {
                    for i in 0..4 {
                        drop(other.lock());
                        log.lock().push(t * 10 + i);
                    }
                }) as Worker
            })
            .collect();
        let outcome = run(seed, replay, 10_000, WATCHDOG, workers);
        assert_eq!(outcome.failure, None);
        let order = log.lock().clone();
        (order, outcome.choices)
    }

    #[test]
    fn a_seed_replays_the_same_interleaving() {
        let (a, choices) = interleaved(7, &[]);
        let (b, _) = interleaved(7, &[]);
        let (c, _) = interleaved(7_000_003, &[]);
        let (replayed, _) = interleaved(1, &choices);
        assert_eq!(a.len(), 12);
        assert_eq!(a, b, "same seed, same order");
        assert_eq!(a, replayed, "a recorded choice list replays the run");
        assert_ne!(a, c, "another seed interleaves differently (these two do)");
    }

    #[test]
    fn a_lock_order_inversion_is_found_as_a_deadlock() {
        let mut found = None;
        for seed in 0..200 {
            let a = Arc::new(RecoveringMutex::new("cell_a", ()));
            let b = Arc::new(RecoveringMutex::new("cell_b", ()));
            let (a2, b2) = (Arc::clone(&a), Arc::clone(&b));
            let workers: Vec<Worker> = vec![
                Box::new(move || {
                    let _a = a.lock();
                    let _b = b.lock();
                }),
                Box::new(move || {
                    let _b = b2.lock();
                    let _a = a2.lock();
                }),
            ];
            let outcome = run(seed, &[], 1_000, WATCHDOG, workers);
            if let Some(Failure::Deadlock(why)) = outcome.failure {
                found = Some((seed, why));
                break;
            }
        }
        let (seed, why) = found.expect("some seed interleaves the two into a deadlock");
        assert!(why.contains("cell_a") && why.contains("cell_b"), "{why}");
        // ... and that seed deadlocks every time.
        let a = Arc::new(RecoveringMutex::new("cell_a", ()));
        let b = Arc::new(RecoveringMutex::new("cell_b", ()));
        let (a2, b2) = (Arc::clone(&a), Arc::clone(&b));
        let again = run(
            seed,
            &[],
            1_000,
            WATCHDOG,
            vec![
                Box::new(move || {
                    let _a = a.lock();
                    let _b = b.lock();
                }),
                Box::new(move || {
                    let _b = b2.lock();
                    let _a = a2.lock();
                }),
            ],
        );
        assert!(matches!(again.failure, Some(Failure::Deadlock(_))));
    }

    #[test]
    fn a_worker_panic_is_reported_and_the_others_finish() {
        let cell = Arc::new(RecoveringMutex::new("cell", 0u32));
        let c2 = Arc::clone(&cell);
        let outcome = run(
            3,
            &[],
            1_000,
            WATCHDOG,
            vec![
                Box::new(move || {
                    let _held = cell.lock();
                    panic!("synthetic worker panic (interleave test)");
                }),
                Box::new(move || *c2.lock() += 1),
            ],
        );
        assert_eq!(outcome.failure, None);
        assert_eq!(outcome.panics.len(), 1);
        assert!(outcome.panics[0].1.contains("synthetic worker panic"));
    }
}
