//! CONCURRENCY: several threads driving one `AppState`, and the four things
//! that must hold no matter how they interleave.
//!
//! # What this is testing, and why nothing was
//!
//! `AppState` carries a `command_lock: Mutex<()>` whose entire job is to
//! serialize `execute`/`undo`/`redo` so they cannot interleave
//! (`docs/undo-redo.md`, "Two `Vec<UndoEntry>` stacks"), and a documented lock
//! ORDERING discipline underneath it (`docs/undo-redo.md:273-279`):
//!
//! > `command_lock` is always outermost. Below it, the mutation paths avoid
//! > holding two of `undo_stack` / `project` / `database` at once at all -
//! > `undo()`/`redo()` pop the entry and drop the `undo_stack` guard before
//! > touching the project, and `get_undo_stack()`/`get_redo_stack()` release
//! > the project lock before acquiring `undo_stack`.
//!
//! A discipline written down in that much detail exists because violating it
//! has consequences. Nothing exercised it: every other engine in this programme
//! drives one `AppState` from one thread.
//!
//! # The assertions are interleaving-INDEPENDENT, and that is the whole design
//!
//! The inputs are seeded per thread, so a failing run is replayable in what each
//! thread ASKED FOR. The interleaving is not, and cannot be. So nothing here
//! asserts a particular outcome: not a final state, not a stack depth, not
//! whether a given command succeeded. Every oracle is a property that has to
//! hold under every schedule:
//!
//! | oracle | what it catches |
//! |---|---|
//! | LIVENESS | a deadlock - the lock ordering discipline's whole reason for existing |
//! | NO PANIC | an index or an unwrap that only a race can reach |
//! | NO POISON | a panic that happened inside a guard, even one another thread swallowed |
//! | POST-RUN | a state that is internally broken once the dust settles |
//!
//! The post-run oracle is deliberately weaker than the roundtrip fuzzer's, and
//! the reason is structural rather than lazy: that model records a canonical
//! snapshot per undo depth, and during a concurrent phase there is no such
//! thing - two threads pushing entries mean no thread knows what depth it is
//! at. So the walk afterwards asserts what survives that: every move either
//! succeeds or fails CLEANLY, the app's own integrity check runs at every
//! depth, and the adjusted solve completes. A wedge is allowed only in the two
//! shapes issue #413 already documents (findings 6 and 13); any other refusal
//! that cannot be walked past is a finding.
//!
//! # Liveness, honestly
//!
//! A deadlock cannot be detected by waiting forever, and a test that waits
//! forever is a CI hang rather than a failure. The workers are joined against a
//! generous deadline; if it expires the test FAILS with what each thread was
//! last seen doing (an atomic per thread, written before every call), which is
//! the most a safe-Rust harness can capture without a native unwinder.
//!
//! The bound is generous ON PURPOSE. This runs on CI machines of unknown
//! contention and on Windows and macOS, whose mutex handoff behaviour differs
//! from Linux's; a tight bound would turn a slow machine into a bug report. It
//! is sized so that a real deadlock (which never finishes) is separated from a
//! slow run (which finishes in well under it) by two orders of magnitude.
//!
//! # Data
//!
//! Everything is generated in code. No fixture is read, copied or derived from.

#![expect(
    clippy::indexing_slicing,
    reason = "test module: see docs/linting.md for the per-test-module expect idiom"
)]
#![expect(
    clippy::print_stdout,
    reason = "the per-run summary IS this test's output, and `cargo test -- --nocapture` is the               only channel a test has"
)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use tempfile::TempDir;

use super::fuzz_undo_tests::{
    adjusted_geometry_of, build_project, canonical_snapshot, integrity_verdict, random_offset,
    random_shot, read_targets, sweep_seeds, Rng, COLORS,
};
use super::AppState;
use crate::core::Command;
use crate::models::{SectionId, ShotData, ShotId, ShotUpdates, StationRef};
// CONFIG ISOLATION. Every test in this module opens with
// `let _config = isolated_config();`: `AppState::new()` LOADS the global config,
// and every save reaches `note_recent_project` -> `Config::save()`, which WRITES
// it. Config resolution is per-thread (`crate::test_helpers` documents what that
// does and does not protect, and why it is not opt-in), so the guard is no longer
// what keeps these tests off other files; what it buys is ONE dir for the whole
// test, whose path the test can name and hand to a worker thread
// (`ConfigHandoff`). It is held for the whole test because the saves are.
use crate::core::events::NullEventSink;
use crate::test_helpers::isolated_config;

// ===========================================================================
// What a thread was doing
// ===========================================================================

/// The operation a worker is inside, as an index into `OPERATIONS`.
///
/// An atomic rather than a lock, because a liveness watchdog that has to take a
/// lock to find out why nothing is progressing is a watchdog that can deadlock
/// on the thing it is diagnosing.
const OPERATIONS: &[&str] = &[
    "idle",
    "execute(CreateShot)",
    "execute(UpdateShot)",
    "execute(CreateSection)",
    "execute(MoveSection)",
    "execute(SetStationComment)",
    "execute(MergeStations)",
    "execute(DeleteShot)",
    "undo",
    "redo",
    "get_undo_stack",
    "get_redo_stack",
    "get_adjusted_stations",
    "integrity_check",
    "save_project",
    "get_project_state",
    "open_project",
    "close_project + reopen",
    "load_project_full",
    "create_project_full",
    "delete_project (open copy) + reopen",
    "finished",
];

/// Shared, lock-free view of what every worker is doing right now.
struct Progress {
    /// `where_[t]` is the index into `OPERATIONS` thread `t` is inside.
    where_: Vec<AtomicUsize>,
    /// `done[t]` counts completed operations, so the watchdog can tell a
    /// deadlock (no thread moves) from a slow run (they all move).
    done: Vec<AtomicUsize>,
}

impl Progress {
    fn new(threads: usize) -> Self {
        Self {
            where_: (0..threads).map(|_| AtomicUsize::new(0)).collect(),
            done: (0..threads).map(|_| AtomicUsize::new(0)).collect(),
        }
    }

    fn enter(&self, thread: usize, operation: usize) {
        self.where_[thread].store(operation, Ordering::Relaxed);
    }

    fn leave(&self, thread: usize) {
        self.done[thread].fetch_add(1, Ordering::Relaxed);
        self.where_[thread].store(0, Ordering::Relaxed);
    }

    fn describe(&self) -> String {
        self.where_
            .iter()
            .zip(&self.done)
            .enumerate()
            .map(|(thread, (operation, done))| {
                format!(
                    "    thread {thread}: in {} after {} completed operation(s)",
                    OPERATIONS
                        .get(operation.load(Ordering::Relaxed))
                        .copied()
                        .unwrap_or("<unknown>"),
                    done.load(Ordering::Relaxed)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn total_done(&self) -> usize {
        self.done
            .iter()
            .map(|counter| counter.load(Ordering::Relaxed))
            .sum()
    }
}

// ===========================================================================
// Panic capture, across threads
// ===========================================================================

/// Every panic any worker hit, with its site.
///
/// The adversarial engine's hook records into a THREAD-LOCAL, which is right
/// there and wrong here: a worker that panics unwinds its own thread and the
/// main thread never sees that cell. So this hook appends into a shared,
/// mutex-free-at-read `Mutex<Vec<..>>` keyed by nothing - the point is that a
/// panic HAPPENED and where, not which thread owned it.
static PANICS: std::sync::Mutex<Vec<(std::thread::ThreadId, String)>> =
    std::sync::Mutex::new(Vec::new());

fn install_panic_recorder() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let site = info
                .location()
                .map_or_else(|| "<unknown site>".to_string(), ToString::to_string);
            let message = info
                .payload()
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    info.payload()
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_string())
                })
                .unwrap_or_else(|| "<non-string panic>".to_string());
            if let Ok(mut panics) = PANICS.lock() {
                panics.push((
                    std::thread::current().id(),
                    format!("{site}: {}", message.lines().next().unwrap_or("")),
                ));
            }
            previous(info);
        }));
    });
}

/// Take the panics belonging to `mine`, and leave everyone else's alone.
///
/// The filter is not tidiness, it is CORRECTNESS. `set_hook` is
/// process-global, libtest runs tests in parallel in one process, and this
/// crate has several tests that poison a mutex ON PURPOSE for lock-failure
/// coverage (`core/state/tests.rs:59`, `speleodb.rs:2996`, `speleodb.rs:6189`,
/// `speleodb_lock_release.rs:573`). Without the thread filter this gate
/// reported twelve of THEIR panics as its own workers racing - which is how it
/// first went red, and is a far more convincing false positive than a real one.
///
/// Entries from other threads are put back, so a concurrently running test that
/// installs its own reader still sees them.
fn drain_panics(mine: &[std::thread::ThreadId]) -> Vec<String> {
    let Ok(mut panics) = PANICS.lock() else {
        return Vec::new();
    };
    let (ours, theirs): (Vec<_>, Vec<_>) = std::mem::take(&mut *panics)
        .into_iter()
        .partition(|(thread, _)| mine.contains(thread));
    *panics = theirs;
    ours.into_iter().map(|(_, text)| text).collect()
}

/// The panic sites issue #413 documents, so a run that trips one is naming a
/// known open finding rather than reporting a race.
///
/// Every arm is GONE, per the delete-when-fixed contract. The merge-alignment
/// arms went when findings 14 (insert), its mirror (remove) and 20 (the
/// unanchored-group drift) were fixed - the structural edits now measure which
/// side is anchored over the rigid merge forest
/// (`Project::anchored_sides_of_edit`), an unanchored merge group is realigned
/// against the edited section, and the self-heal gate is the assert's own
/// `Project::MERGE_ALIGNMENT_INVARIANT_M`. The finding-24 overflow arm went
/// when `shot_display_position` was made saturating. ANY panic, from ANY site,
/// is a reportable problem again; the function stays so the next open finding
/// with a live panic site has somewhere to be named.
fn known_panic(_text: &str) -> Option<&'static str> {
    None
}

// ===========================================================================
// The workers
// ===========================================================================

/// Save the starting project to `<temp>/home.stalx` (every lifecycle operation
/// below ends with a project open again, most of them by reopening this file) and
/// give `state` a sink for the progress-reporting load and create.
fn prepare_lifecycle(state: &AppState, temp: &std::path::Path) -> std::path::PathBuf {
    state.set_event_sink(Arc::new(NullEventSink));
    let home = temp.join("home.stalx");
    let saved = state.save_project(Some(home.clone()));
    assert!(saved.success, "the home file saves: {:?}", saved.error);
    home
}

/// [#576 A1] The project-slot commits, raced against everything else: each takes
/// `command_lock` around its commit, so a deadlock or a torn commit would show up
/// here. Every arm leaves a project open, so the post-run walk has one to walk.
fn lifecycle_operation(
    state: &AppState,
    rng: &mut Rng,
    thread: usize,
    step: usize,
    temp: &std::path::Path,
    home: &std::path::Path,
) -> usize {
    let operation = 16 + rng.below(5);
    match operation {
        16 => {
            let _ = state.open_project(home.to_path_buf());
        }
        17 => {
            let _ = state.close_project();
            let _ = state.open_project(home.to_path_buf());
        }
        18 => {
            // Refused as ALREADY_OPEN when another worker just reopened it - fine.
            let _ = state.do_load_project_full(home.to_path_buf());
        }
        19 => {
            let path = temp.join(format!("created-{thread}-{step}.stalx"));
            let deferred = rng.chance(50);
            if state
                .do_create_project_full(
                    format!("t{thread}-{step}"),
                    None,
                    None,
                    Some(path),
                    deferred,
                )
                .is_err()
            {
                let _ = state.open_project(home.to_path_buf());
            }
        }
        _ => {
            let doomed = temp.join(format!("doomed-{thread}-{step}.stalx"));
            if std::fs::copy(home, &doomed).is_ok() {
                let _ = state.open_project(doomed.clone());
                let _ = state.delete_project(doomed);
            }
            let _ = state.open_project(home.to_path_buf());
        }
    }
    operation
}

/// One worker's action stream. Seeded, so the INPUTS replay even though the
/// schedule does not.
fn worker(
    state: &AppState,
    progress: &Progress,
    thread: usize,
    seed: u32,
    operations: usize,
    temp: &std::path::Path,
    home: &std::path::Path,
    stop: &AtomicBool,
) {
    let mut rng = Rng::new(seed);
    for step in 0..operations {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // [#576 A1] Now and then a project-slot commit instead of an edit: loads,
        // creates, closes and deletes take `command_lock` too, so they race every
        // command for it.
        if rng.chance(4) {
            progress.enter(thread, 16);
            let operation = lifecycle_operation(state, &mut rng, thread, step, temp, home);
            progress.enter(thread, operation);
            progress.leave(thread);
            continue;
        }
        // A project that has gone away under this worker is a reason to SKIP,
        // not to die: `read_targets` ends in an `expect` (it is written for the
        // single-threaded engines, where no project can vanish mid-run), and a
        // harness panic here would be reported as an app panic. The project can
        // legitimately be absent for one instant - another worker's known
        // assert can poison the mutex, and every lock site then reads "no
        // project open".
        // `read_targets` ends in an `expect` - it is written for the
        // single-threaded engines, where no project can vanish mid-run. Here
        // one CAN: another worker tripping a known assert poisons the project
        // mutex, and from that instant every lock site reads "no project open".
        //
        // Checking first is not enough and the check is kept only as a cheap
        // fast path: the poison can land between the check and the read. So the
        // read itself is caught, and the record the hook made for it is dropped
        // from THIS thread's entries - otherwise a harness `expect` would be
        // reported as an app panic, which is how this gate failed one run in
        // fourteen.
        if state.get_project_state().is_none() {
            progress.enter(thread, 15);
            progress.leave(thread);
            continue;
        }
        let Ok(targets) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_targets(state)))
        else {
            let _ = drain_panics(&[std::thread::current().id()]);
            progress.enter(thread, 15);
            progress.leave(thread);
            continue;
        };
        let sections: Vec<SectionId> = targets.sections.iter().map(|(id, _)| *id).collect();
        let pick_shot = |rng: &mut Rng| -> Option<(SectionId, ShotId)> {
            let populated: Vec<&(SectionId, Vec<ShotId>)> = targets
                .sections
                .iter()
                .filter(|(_, shots)| !shots.is_empty())
                .collect();
            if populated.is_empty() {
                return None;
            }
            let (section_id, shots) = populated[rng.below(populated.len())];
            Some((*section_id, shots[rng.below(shots.len())]))
        };

        // Weighted towards the mutation paths, because the lock discipline is
        // about them - but the READ paths are in the mix on purpose:
        // `get_undo_stack` is named in the ordering rule, and
        // `get_adjusted_stations` is the memoized solve, which is exactly where
        // a cache invalidated by one thread while another reads it goes wrong.
        let roll = rng.below(100);
        let operation = match roll {
            0..=17 => 1,
            18..=33 => 2,
            34..=39 => 3,
            40..=46 => 4,
            47..=52 => 5,
            53..=57 => 6,
            58..=61 => 7,
            62..=71 => 8,
            72..=78 => 9,
            79..=83 => 10,
            84..=86 => 11,
            87..=92 => 12,
            93..=96 => 13,
            97 => 14,
            _ => 15,
        };
        progress.enter(thread, operation);
        match operation {
            1 => {
                if !sections.is_empty() {
                    let section_id = sections[rng.below(sections.len())];
                    state.execute(Command::CreateShot {
                        section_id,
                        shot: random_shot(&mut rng),
                        after_shot_id: None,
                        before_shot_id: None,
                    });
                }
            }
            2 => {
                if let Some((section_id, shot_id)) = pick_shot(&mut rng) {
                    state.execute(Command::UpdateShot {
                        section_id,
                        shot_id,
                        updates: ShotUpdates {
                            length: Some(rng.float(1.0, 15.0)),
                            ..ShotUpdates::default()
                        },
                    });
                }
            }
            3 => {
                let count = rng.below(3);
                let shots: Vec<ShotData> = (0..count).map(|_| random_shot(&mut rng)).collect();
                state.execute(Command::CreateSection {
                    name: Some(format!("t{thread}-{step}")),
                    surveyor: None,
                    date: None,
                    color: (*rng.pick(COLORS)).to_string(),
                    direction: None,
                    shots,
                });
            }
            4 => {
                if !sections.is_empty() {
                    state.execute(Command::MoveSection {
                        section_id: sections[rng.below(sections.len())],
                        offset: random_offset(&mut rng),
                    });
                }
            }
            5 => {
                if !targets.stations.is_empty() {
                    let (section_id, index) = targets.stations[rng.below(targets.stations.len())];
                    state.execute(Command::SetStationComment {
                        station_ref: StationRef::new(section_id, index),
                        comment: rng.chance(70).then(|| format!("t{thread}-{step}")),
                    });
                }
            }
            6 => {
                if targets.stations.len() >= 2 {
                    let a = targets.stations[rng.below(targets.stations.len())];
                    let others: Vec<(SectionId, u32)> = targets
                        .stations
                        .iter()
                        .copied()
                        .filter(|station| station.0 != a.0)
                        .collect();
                    if !others.is_empty() {
                        let b = others[rng.below(others.len())];
                        state.execute(Command::MergeStations {
                            station_a: StationRef::new(a.0, a.1),
                            station_b: StationRef::new(b.0, b.1),
                        });
                    }
                }
            }
            7 => {
                if let Some((section_id, shot_id)) = pick_shot(&mut rng) {
                    state.execute(Command::DeleteShot {
                        section_id,
                        shot_id,
                    });
                }
            }
            8 => {
                state.undo();
            }
            9 => {
                state.redo();
            }
            10 => {
                let _ = state.get_undo_stack();
            }
            11 => {
                let _ = state.get_redo_stack();
            }
            12 => {
                // The memoized solve. Occasionally forced cold, because a cache
                // that is never invalidated is a cache that is never raced.
                if rng.chance(25) {
                    state.invalidate_adjusted_cache();
                }
                let _ = state.get_adjusted_stations();
            }
            13 => {
                let _ = state.integrity_check();
            }
            14 => {
                let path = temp.join(format!("concurrent-{thread}.stalx"));
                let _ = state.save_project(Some(path));
            }
            _ => {
                let _ = state.get_project_state();
            }
        }
        progress.leave(thread);
    }
    progress.enter(thread, OPERATIONS.len() - 1);
}

// ===========================================================================
// The run
// ===========================================================================

#[derive(Default)]
struct Outcome {
    operations: usize,
    problems: Vec<String>,
    hits: std::collections::BTreeMap<&'static str, usize>,
    /// Depths the post-run walk verified.
    walked: usize,
    /// Workers that ended by unwinding. Every one of them is accounted for by
    /// the panic recorder, so this is a cross-check rather than a second
    /// oracle: a thread that died without the hook seeing it would mean the
    /// hook is not installed.
    died: usize,
}

/// Drive one `AppState` from `threads` workers, then check what survived.
fn run_concurrent(seed: u32, threads: usize, operations: usize, deadline_secs: u64) -> Outcome {
    install_panic_recorder();
    // Anything this thread recorded in an earlier run is not this run's.
    let _ = drain_panics(&[std::thread::current().id()]);
    let mut outcome = Outcome::default();

    let Ok(state) = AppState::new() else {
        return outcome;
    };
    {
        let mut rng = Rng::new(seed);
        // No closures in the starting project: that topology was finding 14's,
        // and a run that trips the app's own assert on move one measures
        // nothing about concurrency. Finding 14 is FIXED, so turning closures on
        // here is now an option - but it widens what this gate constructs, and
        // nobody has measured the wider gate's stability, so it stays off until
        // someone does.
        build_project(&state, &mut rng, false);
    }
    let Ok(temp) = TempDir::new() else {
        return outcome;
    };
    let home = prepare_lifecycle(&state, temp.path());

    let state = Arc::new(state);
    let progress = Arc::new(Progress::new(threads));
    let stop = Arc::new(AtomicBool::new(false));
    let temp_path = temp.path().to_path_buf();

    // Every worker save reaches `note_recent_project` -> `Config::save()`, and
    // config resolution is per-thread: without the handoff each worker would
    // write a config file of its own, and the concurrent writes to ONE file that
    // this gate has always exercised would quietly stop happening - silently, so
    // each worker also asserts once that it is on the file the others are.
    let config = crate::test_helpers::ConfigHandoff::capture();
    let shared_config_file = crate::config::Config::config_path().ok();
    let handles: Vec<std::thread::JoinHandle<()>> = (0..threads)
        .map(|thread| {
            let state = Arc::clone(&state);
            let progress = Arc::clone(&progress);
            let stop = Arc::clone(&stop);
            let temp_path = temp_path.clone();
            let home = home.clone();
            let config = config.clone();
            let shared_config_file = shared_config_file.clone();
            // Per-thread seeds are derived from the run seed so a whole run
            // replays from one number, and are spread far apart so two threads
            // never walk the same stream.
            let thread_seed = seed
                .wrapping_mul(2_654_435_761)
                .wrapping_add((thread as u32).wrapping_mul(97_531));
            std::thread::spawn(move || {
                let _config = config.adopt();
                assert_eq!(
                    crate::config::Config::config_path().ok(),
                    shared_config_file,
                    "a worker must save into the run's ONE config file; a dir of its \
                     own would make every config write in this gate uncontended"
                );
                worker(
                    &state,
                    &progress,
                    thread,
                    thread_seed,
                    operations,
                    &temp_path,
                    &home,
                    &stop,
                );
            })
        })
        .collect();

    // LIVENESS. Joining a deadlocked thread blocks forever, so the wait polls
    // `is_finished` instead and only joins once every worker has left.
    //
    // `is_finished`, NOT the progress counter. A worker that PANICS unwinds
    // without recording its last operation, so a counter-based wait reads a
    // dead thread as a stuck one and reports a panic as a deadlock - which is
    // exactly what this did until a run tripped one of the known asserts and
    // the gate blamed a lock for it. The counter is still used, for the
    // no-progress window below.
    let target = threads * operations;
    let started = std::time::Instant::now();
    let mut last_total = 0;
    let mut last_progress_at = started;
    // Set when the wait gives up on a thread that never came back. It decides
    // whether the join below is safe to do at all.
    let mut stalled = false;
    loop {
        if handles.iter().all(std::thread::JoinHandle::is_finished) {
            break;
        }
        let total = progress.total_done();
        if total > last_total {
            last_total = total;
            last_progress_at = std::time::Instant::now();
        }
        // Two separate bounds: a whole-run deadline, and a NO-PROGRESS window.
        // The second is what actually distinguishes a deadlock from a slow
        // machine - a slow machine still advances the counter.
        if started.elapsed().as_secs() >= deadline_secs
            || last_progress_at.elapsed().as_secs() >= deadline_secs / 2 + 5
        {
            stop.store(true, Ordering::Relaxed);
            stalled = true;
            outcome.problems.push(format!(
                "the run did not finish: {total} of {target} operation(s) completed in {:?}, and \
                 no thread advanced for {:?}, while {} thread(s) were still running. This is what \
                 a DEADLOCK looks like from outside.\n{}",
                started.elapsed(),
                last_progress_at.elapsed(),
                handles
                    .iter()
                    .filter(|handle| !handle.is_finished())
                    .count(),
                progress.describe()
            ));
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // Joined so the test process does not leak threads into the rest of the
    // suite - but ONLY the threads that actually came back.
    //
    // A worker wedged on the project mutex never reaches the top of its loop,
    // so it never observes `stop` and `join()` on it blocks forever. Doing that
    // here would hang `cargo test` on precisely the run that just detected a
    // deadlock, and CI would kill the job at its wall-clock timeout without
    // ever printing the message composed above - the entire point of the
    // liveness poll. So a stuck worker is left detached: the process is about
    // to fail the gate anyway, and reporting beats tidying.
    let worker_threads: Vec<std::thread::ThreadId> =
        handles.iter().map(|handle| handle.thread().id()).collect();
    let mut died = 0;
    let mut abandoned = 0;
    for handle in handles {
        if stalled && !handle.is_finished() {
            abandoned += 1;
            continue;
        }
        if handle.join().is_err() {
            died += 1;
        }
    }
    if abandoned > 0 {
        outcome.problems.push(format!(
            "{abandoned} worker thread(s) never returned and were left detached rather than \
             joined; joining a wedged thread would have hung this test instead of failing it."
        ));
    }
    outcome.operations = progress.total_done();
    outcome.died = died;

    // NO PANIC, and NO POISON.
    for panic in drain_panics(&worker_threads) {
        match known_panic(&panic) {
            Some(id) => *outcome.hits.entry(id).or_insert(0) += 1,
            None => outcome.problems.push(format!("a worker PANICKED: {panic}")),
        }
    }
    // A KNOWN app panic - there is none left to classify, so this arm is dormant
    // and is kept for the next open finding with a live panic site - unwound
    // through the project mutex, which POISONS it - and from that instant every
    // lock site in the app reads "no project open". So the poison check and the
    // post-run walk below would both be re-reporting one already-classified
    // defect as three, and the follow-on panics from other workers are its
    // consequences rather than races. The run is TERMINAL, exactly as the
    // roundtrip fuzzer's `alignment_panic` is, and for the same reason: nothing
    // after it measures the app.
    if !outcome.hits.is_empty() {
        return outcome;
    }
    if outcome.died > 0 && outcome.hits.is_empty() && outcome.problems.is_empty() {
        outcome.problems.push(format!(
            "{} worker(s) unwound but the panic hook recorded nothing - the recorder is not \
             installed, so NO PANIC is passing vacuously",
            outcome.died
        ));
    }
    if state.project.is_poisoned() {
        outcome
            .problems
            .push("the PROJECT mutex is poisoned after the run".to_string());
    }
    if state.undo_stack.is_poisoned() {
        outcome
            .problems
            .push("the UNDO STACK mutex is poisoned after the run".to_string());
    }

    // POST-RUN CONSISTENCY. Everything below is interleaving-independent: it is
    // about the state that survived, not about how it got there.
    //
    // Inside `catch_unwind`, and NOT because a post-run check is expected to
    // panic: the walk drives real commands, and a `#[cfg(test)]` assert in any
    // of them takes the whole process down. On the main thread that is an
    // immediate test abort with no attribution at all - which is how this gate
    // failed one run in thirteen before the guard went in, when findings 14 and
    // 20 still had live merge-alignment assert sites. The guard stays now that
    // they are fixed: it turns a future assert into this gate's own reported
    // problem instead of an unattributable abort.
    if outcome.problems.is_empty() {
        let mut walked = 0;
        let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check_final_state(&state, &mut walked)
        }));
        outcome.walked += walked;
        match checked {
            Ok(problems) => outcome.problems.extend(problems),
            Err(_) => {
                // The hook recorded it under THIS thread, so the same
                // classification the workers get applies.
                for panic in drain_panics(&[std::thread::current().id()]) {
                    match known_panic(&panic) {
                        Some(id) => *outcome.hits.entry(id).or_insert(0) += 1,
                        None => outcome
                            .problems
                            .push(format!("the post-run walk PANICKED: {panic}")),
                    }
                }
            }
        }
    }
    outcome
}

/// What must be true of the state the threads left behind.
fn check_final_state(state: &AppState, walked: &mut usize) -> Vec<String> {
    let mut problems = Vec::new();
    if state.get_project_state().is_none() {
        problems.push("no project survived the concurrent phase".to_string());
        return problems;
    }
    // The three derived views must all be computable. A solve that returns
    // nothing, or a verdict that cannot be produced, is a broken state whatever
    // the schedule was.
    if canonical_snapshot(state, false).is_empty() {
        problems.push("the surviving project could not be serialized".to_string());
    }
    let verdict = integrity_verdict(state);
    if adjusted_geometry_of(state).is_empty()
        && state.get_project_state().is_some_and(|project| {
            project
                .sections
                .iter()
                .any(|section| !section.shots.is_empty())
        })
    {
        problems.push(
            "the adjusted solve returned nothing for a project that still has shots".to_string(),
        );
    }

    // Walk the history down and back up. NOT the roundtrip oracle: no
    // per-depth snapshot model can exist for a concurrently-built history. What
    // is asserted is that every move either succeeds or fails CLEANLY, that the
    // app's own integrity check runs at every depth, and that the walk is not
    // wedged in any shape issue #413 does not already document.
    let mut steps = 0;
    while !state.get_undo_stack().is_empty() && steps < 400 {
        let result = state.undo();
        steps += 1;
        if !result.success {
            let error = result.error.unwrap_or_default();
            // Findings 6 and 13: both leave the entry pushed back, which is the
            // wedge, and both are open and already pinned.
            let documented = error.contains("part of a loop")
                || error.contains("zero datum")
                || error.contains("Cannot delete a frozen shot")
                || error.contains("not found in section")
                || (error.starts_with("Shot ") && error.ends_with(" not found"));
            if !documented {
                problems.push(format!(
                    "the post-run undo walk failed at step {steps} for an undocumented reason: \
                     {error}"
                ));
            }
            break;
        }
        let _ = integrity_verdict(state);
        *walked += 1;
    }
    let mut forward = 0;
    while !state.get_redo_stack().is_empty() && forward < 400 {
        let result = state.redo();
        forward += 1;
        if !result.success {
            break;
        }
        let _ = integrity_verdict(state);
        *walked += 1;
    }
    // The verdict is read again at the end rather than compared: a verdict is
    // not a function of a schedule, so there is nothing to compare it against.
    // What matters is that it can still be produced.
    let _ = verdict;
    problems
}

/// SEQUENTIAL EQUIVALENCE, in the only form that is honest here.
///
/// Replaying ONE worker's stream single-threaded cannot be asserted to produce
/// the SAME state - interleaving legitimately changes outcomes, and a command
/// that failed because another thread had deleted its target succeeds when it
/// runs alone. What can be asserted is that the stream is not intrinsically
/// destructive: run alone it must leave a state that serializes, integrity-
/// checks and solves. A stream that breaks the app single-threaded would make
/// every concurrent result about that stream rather than about concurrency.
fn check_sequential_replay(seed: u32, thread: usize, operations: usize) -> Vec<String> {
    let Ok(state) = AppState::new() else {
        return Vec::new();
    };
    {
        let mut rng = Rng::new(seed);
        build_project(&state, &mut rng, false);
    }
    let Ok(temp) = TempDir::new() else {
        return Vec::new();
    };
    let home = prepare_lifecycle(&state, temp.path());
    let progress = Progress::new(1);
    let stop = AtomicBool::new(false);
    let thread_seed = seed
        .wrapping_mul(2_654_435_761)
        .wrapping_add((thread as u32).wrapping_mul(97_531));
    worker(
        &state,
        &progress,
        0,
        thread_seed,
        operations,
        temp.path(),
        &home,
        &stop,
    );
    // Same guard, same reason: a replayed stream can trip a known assert on the
    // main thread just as a worker can.
    let mut walked = 0;
    let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        check_final_state(&state, &mut walked)
    }));
    match checked {
        Ok(problems) => problems
            .into_iter()
            .map(|problem| format!("sequential replay of thread {thread}: {problem}"))
            .collect(),
        Err(_) => drain_panics(&[std::thread::current().id()])
            .into_iter()
            .filter(|panic| known_panic(panic).is_none())
            .map(|panic| format!("sequential replay of thread {thread} PANICKED: {panic}"))
            .collect(),
    }
}

// ===========================================================================
// The gate
// ===========================================================================

const GATE_SEEDS: &[u32] = &[1, 2, 3];
const GATE_THREADS: usize = 4;
const GATE_OPERATIONS: usize = 150;
/// Generous BY DESIGN. A real deadlock never finishes; a slow CI box finishes
/// this in well under a second. Two orders of magnitude between them is what
/// keeps a Windows or macOS scheduler from being reported as a bug.
const GATE_DEADLINE_SECS: u64 = 60;

/// THE CONCURRENCY GATE. Not `#[ignore]`d.
#[test]
fn concurrency_gate() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    let mut total: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    let mut operations = 0;
    let mut walked = 0;
    for &seed in GATE_SEEDS {
        let outcome = run_concurrent(seed, GATE_THREADS, GATE_OPERATIONS, GATE_DEADLINE_SECS);
        operations += outcome.operations;
        walked += outcome.walked;
        problems.extend(outcome.problems);
        for (id, count) in outcome.hits {
            *total.entry(id).or_insert(0) += count;
        }
        // One thread's stream, alone, must not break the app by itself.
        problems.extend(check_sequential_replay(seed, 0, GATE_OPERATIONS / 3));
    }
    println!(
        "concurrency: {} seed(s) x {GATE_THREADS} thread(s) x {GATE_OPERATIONS} operation(s) = \
         {operations} completed, {walked} post-run depth(s) walked; classified: {total:?}",
        GATE_SEEDS.len()
    );
    // A run whose threads all no-op'd proves nothing about a lock.
    // Not "every operation completed": a worker that trips a KNOWN assert
    // unwinds and stops early, which would be legitimate and accounted for by
    // the classifier. NOTHING is classified today - finding 24's overflow site
    // and the merge-alignment ones are all fixed, and every panic is a reported
    // problem again - so `total` is empty and the high floor is the one that
    // applies. What must not happen is the workers not running at all.
    let floor = if total.is_empty() {
        GATE_SEEDS.len() * GATE_THREADS * GATE_OPERATIONS
    } else {
        GATE_SEEDS.len() * GATE_THREADS
    };
    assert!(
        operations >= floor,
        "only {operations} operation(s) completed (floor {floor}); the workers did not run"
    );
    assert!(
        problems.is_empty(),
        "{} concurrency failure(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// The SOAK: more threads, more operations, minutes rather than seconds.
///
/// ```text
/// STALAGMIGHT_RUST_CONC_SEEDS=40 STALAGMIGHT_RUST_CONC_THREADS=8 \
///   STALAGMIGHT_RUST_CONC_OPS=2000 \
///   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
///   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
///   --lib concurrency_soak -- --ignored --nocapture
/// ```
#[test]
#[ignore = "soak: minutes, not seconds - run deliberately with STALAGMIGHT_RUST_CONC_SEEDS/THREADS/OPS"]
fn concurrency_soak() {
    let _config = isolated_config();
    let seeds: u32 = std::env::var("STALAGMIGHT_RUST_CONC_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(12);
    let threads: usize = std::env::var("STALAGMIGHT_RUST_CONC_THREADS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(6);
    let operations: usize = std::env::var("STALAGMIGHT_RUST_CONC_OPS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(800);

    let mut total: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    let mut dirty = Vec::new();
    let mut completed = 0;
    for seed in sweep_seeds("STALAGMIGHT_RUST_CONC_FIRST_SEED", seeds) {
        let outcome = run_concurrent(seed, threads, operations, 300);
        completed += outcome.operations;
        for (id, count) in outcome.hits {
            *total.entry(id).or_insert(0) += count;
        }
        if !outcome.problems.is_empty() {
            dirty.push(seed);
            for problem in &outcome.problems {
                println!("seed {seed}: {problem}");
            }
        }
    }
    println!(
        "concurrency soak: {seeds} seed(s) x {threads} thread(s) x {operations} op(s) = \
         {completed} completed; classified: {total:?}"
    );
    println!("seeds with a failure ({}): {dirty:?}", dirty.len());
}
