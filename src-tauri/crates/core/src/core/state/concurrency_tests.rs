//! CONCURRENCY: several threads driving one `AppState`, and what must hold no
//! matter how they interleave.
//!
//! # What this tests
//!
//! `command_lock` serializes `execute`, the recipes, `undo` and `redo`; below
//! it the state's mutexes are taken in one order, `project -> file ->
//! undo_stack -> derived` (docs/undo-redo.md, "Lock Ordering"). Several paths
//! do NOT take `command_lock`: the reads (`get_project_state`, the history
//! lists, the memoised Computed View), the local-only durable save of a
//! display toggle (`save_display_setting`), and the full load
//! (`do_load_project_full`). Every one of them is in the mix here, beside
//! edits, undo, redo, saves to the open file and to other paths, and opens
//! through the executor.
//!
//! # The oracles are interleaving-independent
//!
//! Inputs are seeded per thread; the schedule is not and cannot be. So nothing
//! asserts a particular outcome, only properties of every schedule:
//!
//! | oracle | what it catches |
//! |---|---|
//! | LIVENESS | a deadlock (the lock order's reason to exist) |
//! | NO PANIC | an index or an unwrap only a race can reach |
//! | NO POISON | a panic inside a guard, even one another thread swallowed |
//! | COHERENT CACHE | a memoised raw layout or Computed View that is not the survey's (a worker probes it under the project lock, and once more after the run) |
//! | GUARD ON DISK | the external-modification fingerprint describes the file the app last wrote: a plain save after the run succeeds, and the file reads back as the survey |
//! | WHOLE HISTORY | after the run, every undo to the bottom and every redo back succeeds, the survey is valid at every depth, and the top is the state the walk started from |
//!
//! The history walk is stronger than the SQLite build's, which allowed two
//! documented wedges (#413 findings 6 and 13): every entry on the stacks was
//! recorded under `command_lock` against the survey it is on (a load replaces
//! the survey and the stacks together), and a changeset step checks its
//! preconditions, so a refusal anywhere is a finding.
//!
//! # Liveness, honestly
//!
//! A deadlock cannot be detected by waiting forever. The workers are joined
//! against a generous deadline and a no-progress window; when either expires
//! the test FAILS with what each thread was last seen doing (an atomic per
//! thread, written before every call). A wedged worker is left detached
//! rather than joined, so the failure is reported instead of hanging CI.
//!
//! # Data
//!
//! Everything is generated in code. No fixture is read, copied or derived from.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use tempfile::TempDir;

use super::project_load::IfChanged;
use super::robustness_kit::{
    adjusted_geometry_of, build_project, canonical, canonical_snapshot, derived_cache_is_coherent,
    random_offset, random_shot, read_targets, NullSink, Rng, COLORS,
};
use super::AppState;
use crate::core::Command;
use crate::models::{LineId, ShotData, ShotUpdates, StationRef};
use crate::persistence::stalx_v2::{self, DecodeOptions, Fingerprint};
use crate::test_helpers::isolated_config;

// ===========================================================================
// What a thread was doing
// ===========================================================================

/// The operation a worker is inside, as an index into `OPERATIONS`.
const OPERATIONS: &[&str] = &[
    "idle",
    "execute(CreateShot)",
    "execute(UpdateShot)",
    "execute(CreateLine)",
    "execute(MoveLine)",
    "execute(SetStationComment)",
    "execute(MergeStations)",
    "execute(DeleteShot)",
    "undo",
    "redo",
    "get_undo_stack",
    "get_redo_stack",
    "get_adjusted_stations",
    "integrity_check",
    "save_project(path)",
    "get_project_state",
    "save_project(open file)",
    "save_display_setting",
    "do_load_project_full",
    "execute(OpenProject)",
    "probe the derived cache",
    "undo_to",
    "finished",
];

const FINISHED: usize = OPERATIONS.len() - 1;

/// Shared, lock-free view of what every worker is doing (a watchdog that had
/// to take a lock to diagnose a deadlock could deadlock on it).
struct Progress {
    where_: Vec<AtomicUsize>,
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

/// Every panic any thread hit, with its site and thread.
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

/// Take the panics of `mine` and leave everyone else's: the hook is
/// process-global, and other tests in this binary poison mutexes on purpose.
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

// ===========================================================================
// The workers
// ===========================================================================

/// The files a run's workers save to and open.
struct Files {
    dir: PathBuf,
    /// Two copies of the starting survey: a full load alternates between them
    /// (it refuses the file that is already open).
    loads: [PathBuf; 2],
}

/// Problems a worker saw itself (the derived-cache probe).
type Seen = std::sync::Mutex<Vec<String>>;

/// One worker's action stream. Seeded, so the INPUTS replay even though the
/// schedule does not.
fn worker(
    state: &AppState,
    progress: &Progress,
    thread: usize,
    seed: u32,
    operations: usize,
    files: &Files,
    stop: &AtomicBool,
    seen: &Seen,
) {
    let mut rng = Rng::new(seed);
    for step in 0..operations {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let Some(targets) = read_targets(state) else {
            progress.enter(thread, 15);
            progress.leave(thread);
            continue;
        };
        let lines: Vec<LineId> = targets.lines.iter().map(|(id, _)| *id).collect();
        let operation = match rng.below(100) {
            0..=14 => 1,
            15..=27 => 2,
            28..=32 => 3,
            33..=38 => 4,
            39..=43 => 5,
            44..=47 => 6,
            48..=51 => 7,
            52..=59 => 8,
            60..=65 => 9,
            66..=68 => 10,
            69..=70 => 11,
            71..=75 => 12,
            76..=78 => 13,
            79..=80 => 14,
            81..=82 => 15,
            83..=85 => 16,
            86..=88 => 17,
            89..=90 => 18,
            91 => 19,
            92..=96 => 20,
            _ => 21,
        };
        progress.enter(thread, operation);
        match operation {
            1 => {
                if !lines.is_empty() {
                    state.execute(Command::CreateShot {
                        line_id: lines[rng.below(lines.len())],
                        shot: random_shot(&mut rng),
                        after_shot_id: None,
                        before_shot_id: None,
                    });
                }
            }
            2 => {
                if let Some((line_id, shot_id)) = targets.pick_shot(&mut rng) {
                    state.execute(Command::UpdateShot {
                        line_id,
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
                state.execute(Command::CreateLine {
                    name: Some(format!("t{thread}-{step}")),
                    surveyor: None,
                    date: None,
                    color: (*rng.pick(COLORS)).to_string(),
                    direction: None,
                    shots,
                    cave_section_id: None,
                });
            }
            4 => {
                if !lines.is_empty() {
                    state.execute(Command::MoveLine {
                        line_id: lines[rng.below(lines.len())],
                        offset: random_offset(&mut rng),
                    });
                }
            }
            5 => {
                if !targets.stations.is_empty() {
                    let (line_id, index) = targets.stations[rng.below(targets.stations.len())];
                    state.execute(Command::SetStationComment {
                        station_ref: StationRef::new(line_id, index),
                        comment: rng.chance(70).then(|| format!("t{thread}-{step}")),
                    });
                }
            }
            6 => {
                if targets.stations.len() >= 2 {
                    let a = targets.stations[rng.below(targets.stations.len())];
                    let others: Vec<(LineId, u32)> = targets
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
                            keep: None,
                            renames: None,
                        });
                    }
                }
            }
            7 => {
                if let Some((line_id, shot_id)) = targets.pick_shot(&mut rng) {
                    state.execute(Command::DeleteShot { line_id, shot_id });
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
                // The memoised solve, forced cold now and then: a cache that
                // is never invalidated is never raced.
                if rng.chance(25) {
                    *state
                        .derived
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                }
                let _ = state.get_adjusted_stations();
                let _ = state.get_adjustment_report();
            }
            13 => {
                let _ = state.integrity_violation_messages();
            }
            14 => {
                let path = files.dir.join(format!("concurrent-{thread}.stalx"));
                let _ = state.save_project(Some(path));
            }
            16 => {
                let _ = state.save_project(None);
            }
            17 => {
                let value = if rng.chance(50) { "true" } else { "false" };
                let _ = state.save_display_setting("showStations", value);
            }
            18 => {
                let path = files.loads[rng.below(2)].clone();
                let _ = state.do_load_project_full(path);
            }
            19 => {
                let path = files.loads[rng.below(2)].clone();
                state.execute(Command::OpenProject { path });
            }
            20 => {
                if let Err(problem) = derived_cache_is_coherent(state) {
                    if let Ok(mut seen) = seen.lock() {
                        seen.push(format!("thread {thread} step {step}: {problem}"));
                    }
                }
            }
            21 => {
                let depth = state.get_undo_stack().len();
                if depth > 0 {
                    let _ = state.undo_to(rng.below(depth.min(4)));
                }
            }
            _ => {
                let _ = state.get_project_state();
                let _ = state.project_state_wire();
            }
        }
        progress.leave(thread);
    }
    progress.enter(thread, FINISHED);
}

// ===========================================================================
// The run
// ===========================================================================

#[derive(Default)]
struct Outcome {
    operations: usize,
    problems: Vec<String>,
    /// Depths the post-run walk verified.
    walked: usize,
    /// Workers that ended by unwinding (a cross-check of the panic recorder).
    died: usize,
}

/// A fresh state holding the seed's starting survey, saved to both load files
/// and left open on the first (so every save path has a file to guard).
fn starting_state(seed: u32, dir: &Path) -> Option<(AppState, Files)> {
    let state = AppState::new().ok()?;
    state.set_event_sink(Arc::new(NullSink));
    let mut rng = Rng::new(seed);
    build_project(&state, &mut rng, false);
    let loads = [dir.join("load-a.stalx"), dir.join("load-b.stalx")];
    for path in &loads {
        if !state.save_project(Some(path.clone())).success {
            return None;
        }
    }
    if !state
        .execute(Command::OpenProject {
            path: loads[0].clone(),
        })
        .success
    {
        return None;
    }
    Some((
        state,
        Files {
            dir: dir.to_path_buf(),
            loads,
        },
    ))
}

fn thread_seed(seed: u32, thread: usize) -> u32 {
    seed.wrapping_mul(2_654_435_761)
        .wrapping_add((thread as u32).wrapping_mul(97_531))
}

/// Drive one `AppState` from `threads` workers, then check what survived.
fn run_concurrent(seed: u32, threads: usize, operations: usize, deadline_secs: u64) -> Outcome {
    install_panic_recorder();
    let _ = drain_panics(&[std::thread::current().id()]);
    let mut outcome = Outcome::default();
    let Ok(temp) = TempDir::new() else {
        return outcome;
    };
    let Some((state, files)) = starting_state(seed, temp.path()) else {
        outcome.problems.push(format!(
            "seed {seed}: the starting survey could not be saved"
        ));
        return outcome;
    };
    let state = Arc::new(state);
    let files = Arc::new(files);
    let progress = Arc::new(Progress::new(threads));
    let stop = Arc::new(AtomicBool::new(false));
    let seen: Arc<Seen> = Arc::new(std::sync::Mutex::new(Vec::new()));

    // Every save reaches `note_recent_project` -> `Config::save()`; the
    // handoff puts every worker on the run's ONE config file, so those writes
    // are contended as they are in the app.
    let config = crate::test_helpers::ConfigHandoff::capture();
    let shared_config_file = crate::config::Config::config_path().ok();
    let handles: Vec<std::thread::JoinHandle<()>> = (0..threads)
        .map(|thread| {
            let state = Arc::clone(&state);
            let progress = Arc::clone(&progress);
            let stop = Arc::clone(&stop);
            let files = Arc::clone(&files);
            let seen = Arc::clone(&seen);
            let config = config.clone();
            let shared_config_file = shared_config_file.clone();
            let seed = thread_seed(seed, thread);
            std::thread::spawn(move || {
                let _config = config.adopt();
                assert_eq!(
                    crate::config::Config::config_path().ok(),
                    shared_config_file,
                    "a worker must save into the run's ONE config file"
                );
                worker(
                    &state, &progress, thread, seed, operations, &files, &stop, &seen,
                );
            })
        })
        .collect();

    // LIVENESS: poll `is_finished` (a panicked worker is finished, not stuck)
    // against a whole-run deadline and a no-progress window.
    let target = threads * operations;
    let started = std::time::Instant::now();
    let mut last_total = 0;
    let mut last_progress_at = started;
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
        if started.elapsed().as_secs() >= deadline_secs
            || last_progress_at.elapsed().as_secs() >= deadline_secs / 2 + 5
        {
            stop.store(true, Ordering::Relaxed);
            stalled = true;
            outcome.problems.push(format!(
                "seed {seed}: the run did not finish: {total} of {target} operation(s) completed \
                 in {:?}, and no thread advanced for {:?}, while {} thread(s) were still \
                 running. This is what a DEADLOCK looks like from outside.\n{}",
                started.elapsed(),
                last_progress_at.elapsed(),
                handles.iter().filter(|h| !h.is_finished()).count(),
                progress.describe()
            ));
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let worker_threads: Vec<std::thread::ThreadId> =
        handles.iter().map(|handle| handle.thread().id()).collect();
    let mut abandoned = 0;
    for handle in handles {
        if stalled && !handle.is_finished() {
            abandoned += 1;
            continue;
        }
        if handle.join().is_err() {
            outcome.died += 1;
        }
    }
    if abandoned > 0 {
        outcome.problems.push(format!(
            "seed {seed}: {abandoned} worker thread(s) never returned and were left detached"
        ));
    }
    outcome.operations = progress.total_done();

    // NO PANIC, NO POISON.
    for panic in drain_panics(&worker_threads) {
        outcome
            .problems
            .push(format!("seed {seed}: a worker PANICKED: {panic}"));
    }
    if outcome.died > 0 && outcome.problems.is_empty() {
        outcome.problems.push(format!(
            "seed {seed}: {} worker(s) unwound but the panic hook recorded nothing",
            outcome.died
        ));
    }
    for (name, poisoned) in [
        ("project", state.project.is_poisoned()),
        ("file", state.file.is_poisoned()),
        ("undo stack", state.undo_stack.is_poisoned()),
        ("derived", state.derived.is_poisoned()),
        ("command lock", state.command_lock.is_poisoned()),
    ] {
        if poisoned {
            outcome
                .problems
                .push(format!("seed {seed}: the {name} mutex is poisoned"));
        }
    }
    if let Ok(mut seen) = seen.lock() {
        outcome
            .problems
            .extend(seen.drain(..).map(|p| format!("seed {seed}: {p}")));
    }

    // POST-RUN CONSISTENCY, inside `catch_unwind` so a future assert in any
    // command is this gate's own reported problem.
    if outcome.problems.is_empty() {
        let mut walked = 0;
        let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            check_final_state(&state, &mut walked)
        }));
        outcome.walked += walked;
        match checked {
            Ok(problems) => outcome
                .problems
                .extend(problems.into_iter().map(|p| format!("seed {seed}: {p}"))),
            Err(_) => {
                for panic in drain_panics(&[std::thread::current().id()]) {
                    outcome
                        .problems
                        .push(format!("seed {seed}: the post-run walk PANICKED: {panic}"));
                }
            }
        }
    }
    outcome
}

/// What must be true of the state the threads left behind.
fn check_final_state(state: &AppState, walked: &mut usize) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(project) = state.get_project_state() else {
        problems.push("no project survived the concurrent phase".to_string());
        return problems;
    };
    // COHERENT CACHE, before anything bumps the revision again.
    if let Err(problem) = derived_cache_is_coherent(state) {
        problems.push(format!("after the run, {problem}"));
    }
    if canonical(&project).starts_with('<') {
        problems.push("the surviving survey could not be encoded".to_string());
    }
    if adjusted_geometry_of(state).is_empty() && project.lines().iter().any(|l| !l.shots.is_empty())
    {
        problems.push(
            "the Computed View returned nothing for a survey that still has shots".to_string(),
        );
    }

    // GUARD ON DISK: the fingerprint describes the file, so a plain save
    // succeeds and the file reads back as the survey.
    if let Some(path) = project.file_path {
        let expected = state.with_file(|file| file.fingerprint.clone());
        if let (Some(expected), Ok((_, on_disk))) = (expected, Fingerprint::read(&path, u64::MAX)) {
            if !on_disk.same_content(&expected) {
                problems.push(format!(
                    "the guard's fingerprint does not describe {} any more, so the next save \
                     would be refused as changed by another program",
                    path.display()
                ));
            }
        }
        let saved = state.save_project(None);
        if saved.success {
            match std::fs::read(&path)
                .ok()
                .and_then(|bytes| stalx_v2::decode(&bytes, &DecodeOptions::default()).ok())
            {
                Some(decoded) if canonical(&decoded.project) == canonical_snapshot(state) => {}
                Some(_) => problems.push(format!(
                    "{} does not read back as the survey after the final save",
                    path.display()
                )),
                None => problems.push(format!(
                    "{} does not decode after the final save",
                    path.display()
                )),
            }
        } else {
            problems.push(format!(
                "a plain save after the run was refused: {:?}",
                saved.error
            ));
        }
    }

    // WHOLE HISTORY: up to the top first (the workers' undos left redo
    // entries), then down to the bottom and back, every step succeeding and
    // the survey valid at every depth.
    let mut ahead = 0;
    while state.can_redo() && ahead < 1000 {
        let result = state.redo();
        ahead += 1;
        if !result.success {
            problems.push(format!(
                "a redo towards the top was refused at step {ahead}: {:?}",
                result.error
            ));
            return problems;
        }
        *walked += 1;
    }
    let top = canonical_snapshot(state);
    let mut steps = 0;
    while state.can_undo() && steps < 1000 {
        let result = state.undo();
        steps += 1;
        if !result.success {
            problems.push(format!(
                "the post-run undo walk was refused at step {steps}: {:?}",
                result.error
            ));
            return problems;
        }
        let violations = state.integrity_violation_messages();
        if !violations.is_empty() {
            problems.push(format!(
                "undo step {steps} left an invalid survey: {violations:?}"
            ));
            return problems;
        }
        *walked += 1;
    }
    let mut forward = 0;
    while state.can_redo() && forward < 1000 {
        let result = state.redo();
        forward += 1;
        if !result.success {
            problems.push(format!(
                "the post-run redo walk was refused at step {forward}: {:?}",
                result.error
            ));
            return problems;
        }
        *walked += 1;
    }
    if forward != steps {
        problems.push(format!(
            "the walk went down {steps} step(s) and back up {forward}"
        ));
    }
    let back = canonical_snapshot(state);
    if !same_up_to_counters(&top, &back) {
        problems.push(format!(
            "redoing to the top did not land on the state the walk started from: {}",
            super::robustness_kit::first_difference(&top, &back)
        ));
    }
    problems
}

/// Equal canonical texts, except that the id counters may stand higher (an
/// undo applies them as `max(live, recorded)`, plan 8.1).
fn same_up_to_counters(want: &str, got: &str) -> bool {
    let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), got.lines().collect());
    w.len() == g.len()
        && w.iter().zip(&g).all(|(w, g)| {
            w == g || (w.starts_with("\"counters\":") && g.starts_with("\"counters\":"))
        })
}

/// SEQUENTIAL EQUIVALENCE, in the only honest form: one worker's stream, run
/// alone, must leave a state that passes the same post-run checks (so a
/// concurrent failure is about concurrency, not about the stream).
fn check_sequential_replay(seed: u32, thread: usize, operations: usize) -> Vec<String> {
    let Ok(temp) = TempDir::new() else {
        return Vec::new();
    };
    let Some((state, files)) = starting_state(seed, temp.path()) else {
        return vec![format!(
            "seed {seed}: the starting survey could not be saved"
        )];
    };
    let progress = Progress::new(1);
    let stop = AtomicBool::new(false);
    let seen: Seen = std::sync::Mutex::new(Vec::new());
    worker(
        &state,
        &progress,
        0,
        thread_seed(seed, thread),
        operations,
        &files,
        &stop,
        &seen,
    );
    let mut problems: Vec<String> = seen.lock().map(|s| s.clone()).unwrap_or_default();
    let mut walked = 0;
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        check_final_state(&state, &mut walked)
    })) {
        Ok(found) => problems.extend(found),
        Err(payload) => problems.push(format!(
            "PANICKED: {}",
            super::robustness_kit::panic_text(payload.as_ref())
        )),
    }
    problems
        .into_iter()
        .map(|p| format!("seed {seed}, sequential replay of thread {thread}: {p}"))
        .collect()
}

// ===========================================================================
// The gate
// ===========================================================================

const GATE_SEEDS: &[u32] = &[1, 2, 3];
const GATE_THREADS: usize = 4;
const GATE_OPERATIONS: usize = 150;
/// Generous BY DESIGN: a real deadlock never finishes, a slow CI box finishes
/// this in seconds.
const GATE_DEADLINE_SECS: u64 = 120;

/// THE CONCURRENCY GATE. Not `#[ignore]`d.
#[test]
fn concurrency_gate() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    let mut operations = 0;
    let mut walked = 0;
    for &seed in GATE_SEEDS {
        let outcome = run_concurrent(seed, GATE_THREADS, GATE_OPERATIONS, GATE_DEADLINE_SECS);
        operations += outcome.operations;
        walked += outcome.walked;
        problems.extend(outcome.problems);
        problems.extend(check_sequential_replay(seed, 0, GATE_OPERATIONS / 3));
    }
    println!(
        "concurrency: {} seed(s) x {GATE_THREADS} thread(s) x {GATE_OPERATIONS} operation(s) = \
         {operations} completed, {walked} post-run depth(s) walked",
        GATE_SEEDS.len()
    );
    let floor = GATE_SEEDS.len() * GATE_THREADS * GATE_OPERATIONS;
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

/// A full load racing edits, with the derived cache probed under the project
/// lock between them. The load does not take `command_lock`, and it seeds the
/// memoised raw layout of the survey it opened after the commit; an edit
/// landing between the two must not leave that layout cached as the edited
/// survey's.
#[test]
fn a_full_load_racing_edits_never_leaves_a_stale_derived_cache() {
    let _config = isolated_config();
    install_panic_recorder();
    let temp = TempDir::new().unwrap();
    let (state, files) = starting_state(7, temp.path()).unwrap();
    let state = Arc::new(state);
    let files = Arc::new(files);
    let stop = Arc::new(AtomicBool::new(false));
    let config = crate::test_helpers::ConfigHandoff::capture();
    let loader = {
        let (state, files, stop, config) = (
            Arc::clone(&state),
            Arc::clone(&files),
            Arc::clone(&stop),
            config.clone(),
        );
        std::thread::spawn(move || {
            let _config = config.adopt();
            let loads: usize = std::env::var("STALAGMIGHT_RUST_CONC_LOADS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(300);
            for i in 0..loads {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let _ = state.do_load_project_full(files.loads[i % 2].clone());
            }
        })
    };
    let editor = {
        let (state, stop) = (Arc::clone(&state), Arc::clone(&stop));
        std::thread::spawn(move || {
            let _config = config.adopt();
            let mut rng = Rng::new(11);
            while !stop.load(Ordering::Relaxed) {
                if let Some((line_id, shot_id)) =
                    read_targets(&state).and_then(|t| t.pick_shot(&mut rng))
                {
                    state.execute(Command::UpdateShot {
                        line_id,
                        shot_id,
                        updates: ShotUpdates {
                            length: Some(rng.float(1.0, 15.0)),
                            ..ShotUpdates::default()
                        },
                    });
                }
            }
        })
    };
    let mut stale = Vec::new();
    while !loader.is_finished() {
        if let Err(problem) = derived_cache_is_coherent(&state) {
            stale.push(problem);
        }
    }
    stop.store(true, Ordering::Relaxed);
    loader.join().unwrap();
    editor.join().unwrap();
    assert!(
        stale.is_empty(),
        "{} stale derived cache(s), first: {}",
        stale.len(),
        stale[0]
    );
}

/// Walk the persisted history of the file at `path` in a fresh state: down to
/// the bottom and back up, every step succeeding (the entries are decoded,
/// so each step checks its preconditions against the body the file holds).
fn walk_the_file_history(path: &Path) -> Result<usize, String> {
    let state = super::robustness_kit::open_fresh(path)
        .ok_or_else(|| format!("{} does not open", path.display()))?;
    let mut steps = 0;
    while state.can_redo() {
        let r = state.redo();
        if !r.success {
            return Err(format!("redo {steps} refused: {:?}", r.error));
        }
        steps += 1;
    }
    while state.can_undo() {
        let r = state.undo();
        if !r.success {
            return Err(format!("undo after {steps} step(s) refused: {:?}", r.error));
        }
        steps += 1;
    }
    Ok(steps)
}

/// A display toggle's local-only save racing saves, undo and redo: the
/// history it writes beside the body on disk must be the one that goes with
/// that body. The toggle does not take `command_lock`; it used to read the
/// dirty flag and the stacks without the `project` guard, so an undo caught
/// between popping its entry and marking the project dirty had the file
/// written with the pre-undo body and the post-undo stacks, and the next open
/// refused its history ("the project is not in the state this history step
/// expects"). Found by `concurrency_soak` (seeds 16 and 27 of 30 x 8 x 800).
#[test]
fn a_display_toggle_racing_undo_never_writes_a_history_that_does_not_fit_the_body() {
    let _config = isolated_config();
    install_panic_recorder();
    let temp = TempDir::new().unwrap();
    let (state, files) = starting_state(5, temp.path()).unwrap();
    let mut rng = Rng::new(5);
    for _ in 0..6 {
        if let Some((line_id, _)) = read_targets(&state).and_then(|t| t.lines.first().cloned()) {
            state.execute(Command::CreateShot {
                line_id,
                shot: random_shot(&mut rng),
                after_shot_id: None,
                before_shot_id: None,
            });
        }
    }
    assert!(state.save_project(None).success);
    let rounds: usize = std::env::var("STALAGMIGHT_RUST_CONC_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(40);
    let state = Arc::new(state);
    let stop = Arc::new(AtomicBool::new(false));
    let config = crate::test_helpers::ConfigHandoff::capture();
    let toggler = {
        let (state, stop) = (Arc::clone(&state), Arc::clone(&stop));
        std::thread::spawn(move || {
            let _config = config.adopt();
            let mut on = false;
            while !stop.load(Ordering::Relaxed) {
                on = !on;
                let _ = state.save_display_setting("showGrid", if on { "true" } else { "false" });
            }
        })
    };
    let mut broken = Vec::new();
    for round in 0..rounds {
        // A clean file, then one step: the toggles that follow write the
        // history the file had, whatever the stacks say now.
        let saved = state.save_project(None);
        assert!(saved.success, "round {round}: {:?}", saved.error);
        let step = if round % 2 == 0 {
            state.undo()
        } else {
            state.redo()
        };
        assert!(step.success, "round {round}: {:?}", step.error);
        if let Err(why) = walk_the_file_history(&files.loads[0]) {
            broken.push(format!("after round {round}: {why}"));
        }
    }
    stop.store(true, Ordering::Relaxed);
    toggler.join().unwrap();
    if let Err(why) = walk_the_file_history(&files.loads[0]) {
        broken.push(format!("at the end: {why}"));
    }
    assert!(
        broken.is_empty(),
        "{} file(s) whose history does not fit the body, first: {}",
        broken.len(),
        broken[0]
    );
}

/// Whether the guard's fingerprint describes the open file's bytes.
fn guard_describes_the_open_file(state: &AppState) -> Result<(), String> {
    let path = state
        .open_file_path()
        .ok_or_else(|| "no file is open".to_string())?;
    let expected = state
        .with_file(|file| file.fingerprint.clone())
        .ok_or_else(|| "the open file has no fingerprint".to_string())?;
    let (_, on_disk) = Fingerprint::read(&path, u64::MAX).map_err(|e| e.to_string())?;
    if on_disk.same_content(&expected) {
        Ok(())
    } else {
        Err(format!(
            "the guard's fingerprint does not describe {}",
            path.display()
        ))
    }
}

/// An open reads the file without a lock and commits it later. One of our
/// own writes of that file in between (here a display toggle's local-only
/// save of the open file, which takes no `command_lock`) left the commit
/// installing the fingerprint of bytes no longer on disk, so the next save
/// was refused as "changed by another program". Found by `concurrency_soak`
/// (seeds 32000087 and 32000349, 4 threads x 800 ops). The interleaving is
/// forced here: the read, the toggle, then the commit, which must refuse
/// the read and change nothing.
#[test]
fn a_commit_refuses_a_read_our_own_write_made_stale() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let (state, files) = starting_state(3, temp.path()).unwrap();
    let path = &files.loads[0];
    let opened = state.read_project_file(path).unwrap();
    let (_, read) = Fingerprint::read(path, u64::MAX).unwrap();
    let shown = state.get_display_setting("showGrid").unwrap();
    let flipped = if shown.as_deref() == Some("true") {
        "false"
    } else {
        "true"
    };
    state.save_display_setting("showGrid", flipped).unwrap();
    let (_, toggled) = Fingerprint::read(path, u64::MAX).unwrap();
    assert!(
        !toggled.same_content(&read),
        "the toggle must rewrite the file for this test to mean anything"
    );
    let revision = state.current_revision();
    assert!(
        state.commit_opened(opened, IfChanged::ReadAgain).is_err(),
        "a read the file no longer holds was committed"
    );
    assert_eq!(
        state.current_revision(),
        revision,
        "the refusal committed something"
    );
    guard_describes_the_open_file(&state).unwrap();
    let saved = state.save_project(None);
    assert!(saved.success, "{:?}", saved.error);
    // The executor's open reads it again and commits what the file holds.
    let reopened = state.execute(Command::OpenProject { path: path.clone() });
    assert!(reopened.success, "{:?}", reopened.error);
    guard_describes_the_open_file(&state).unwrap();
}

/// The same window through the IPC load, forced by the load's own progress
/// event: a Save As of the open project onto the file being loaded lands
/// between the load's read and its commit. The load reads the file again,
/// opens what it now holds, and guards those bytes. Before the fix it opened
/// the bytes the Save As replaced, guarded by their fingerprint, and the
/// next save was refused.
#[test]
fn a_save_as_onto_the_file_being_loaded_is_read_again() {
    struct SaveAsMidLoad {
        state: std::sync::OnceLock<std::sync::Weak<AppState>>,
        target: PathBuf,
        fired: AtomicBool,
    }
    impl crate::core::events::EventSink for SaveAsMidLoad {
        fn emit(&self, _event: &str, payload: serde_json::Value) {
            let mid_load =
                serde_json::to_value(crate::core::loading::LoadingPhase::ComputingStations).ok();
            if payload.get("phase") != mid_load.as_ref() || self.fired.swap(true, Ordering::SeqCst)
            {
                return;
            }
            if let Some(state) = self.state.get().and_then(std::sync::Weak::upgrade) {
                let saved = state.save_project(Some(self.target.clone()));
                assert!(saved.success, "the Save As mid-load: {:?}", saved.error);
            }
        }

        fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
            false
        }
    }

    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let (state, files) = starting_state(4, temp.path()).unwrap();
    // The open survey differs from the one load-b holds, so the Save As
    // rewrites load-b.
    let (line_id, _) = read_targets(&state).unwrap().lines[0].clone();
    let edited = state.execute(Command::SetStationComment {
        station_ref: StationRef::new(line_id, 0),
        comment: Some("written by the Save As".to_string()),
    });
    assert!(edited.success, "{:?}", edited.error);
    let state = Arc::new(state);
    let sink = Arc::new(SaveAsMidLoad {
        state: std::sync::OnceLock::new(),
        target: files.loads[1].clone(),
        fired: AtomicBool::new(false),
    });
    let _ = sink.state.set(Arc::downgrade(&state));
    state.set_event_sink(Arc::clone(&sink) as Arc<dyn crate::core::events::EventSink>);

    let loaded = state.do_load_project_full(files.loads[1].clone());
    assert!(loaded.is_ok(), "{:?}", loaded.err());
    assert!(sink.fired.load(Ordering::SeqCst), "the Save As never ran");
    guard_describes_the_open_file(&state).unwrap();
    let on_disk = stalx_v2::decode(
        &std::fs::read(&files.loads[1]).unwrap(),
        &DecodeOptions::default(),
    )
    .unwrap();
    assert_eq!(
        canonical(&on_disk.project),
        canonical_snapshot(&state),
        "the load opened bytes the file no longer holds"
    );
    let saved = state.save_project(None);
    assert!(saved.success, "{:?}", saved.error);
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
///
/// `STALAGMIGHT_RUST_CONC_FIRST_SEED=n` starts the seed window at `n` (default 1).
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
    let mut dirty = Vec::new();
    let mut completed = 0;
    for seed in crate::test_helpers::sweep_seeds("STALAGMIGHT_RUST_CONC_FIRST_SEED", seeds) {
        let outcome = run_concurrent(seed, threads, operations, 300);
        completed += outcome.operations;
        if !outcome.problems.is_empty() {
            dirty.push(seed);
            for problem in &outcome.problems {
                println!("{problem}");
            }
        }
    }
    println!(
        "concurrency soak: {seeds} seed(s) x {threads} thread(s) x {operations} op(s) = \
         {completed} completed"
    );
    println!("seeds with a failure ({}): {dirty:?}", dirty.len());
}
