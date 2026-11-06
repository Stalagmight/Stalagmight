//! [#576 FZ-INT] Interleaving fuzzer for the command layer: concurrent commands
//! against one `AppState`, under a DETERMINISTIC schedule.
//!
//! The commands run on the blocking pool, so an edit, an autosave, a display
//! toggle, a settled read, a load and an undo can all be in flight at once; the
//! largest bug class of the #576 audit was what happens when they interleave
//! (RV5-1: a display setting written while a save staged its temp file was lost;
//! RV6b-2: a display write landed in the project opened after it was made). The
//! existing `concurrency_tests` drive real threads, so a failing interleaving
//! cannot be replayed. Here every lock cell is a scheduling point
//! (`models::interleave`): a seed fixes the interleaving, a failing case is
//! replayed and shrunk, and a deadlock is FOUND rather than waited out.
//!
//! # Cases
//!
//! A case is a seed, a mode and 2-4 threads, each with a short op list:
//! - `Edits`: forward edits that each leave a unique marker section, undo, redo,
//!   save, Save As, display-setting writes (identity-checked and legacy), settled
//!   reads, and injected crashes (a panic while holding one cell);
//! - `Lifecycle`: edits, undo/redo, saves, identity-checked display writes and
//!   settled reads racing loads of three files (two of them copies of one project:
//!   same `created_at`, other path) and closes. Half of these cases are load
//!   storms: every thread opens with a load of a file that is not open.
//! - `Reads`: `Edits` without crashes, plus the IPC read `distance_field_checked`
//!   (`Measure`) next to the settled reads;
//! - `Sessions`: `Lifecycle` plus File > New (`Create`, the first save deferred
//!   or not) and `Measure`.
//!
//! An autosave is the same backend save as Ctrl+S (`is_autosave` only names the
//! trigger in the log), so `Save` stands for both.
//!
//! The seed also picks the scheduling strategy (`models::interleave`): a biased
//! random walk, or random thread priorities with a few change points (PCT), which
//! is what lets one load run a long stretch while another waits at one lock.
//!
//! # Oracles (interleaving-independent)
//!
//! 1. No deadlock (the scheduler finds one) and no panic other than the injected.
//! 2. A settled read (`settled_read`) never sees a half-applied command: inside it,
//!    the marker sections and the undo entries are in step (every forward edit here
//!    adds one marker and one undo entry; undo and redo move one of each), and
//!    `settled_snapshot` passes the app's integrity check with no duplicate marker.
//! 3. Every acknowledged edit is in the final project unless an acknowledged undo
//!    took it (`markers = created - undone + redone`, `markers` within `created`),
//!    and after a final save the file read back from disk holds exactly the
//!    in-memory sections (`Edits`, `Reads`; the others check the read-back only).
//! 4. Display settings: the last acknowledged write of each key is the live value
//!    and is on disk after a final save; a write made for the open project is never
//!    refused while no switch happens (`Edits`, `Reads`); a write lands only in the
//!    file it was made for, never in another project or another copy (`Lifecycle`,
//!    `Sessions`).
//! 5. Poison policy: after an injected crash in `command_lock`, `project` or
//!    `undo_stack`, every later undo/redo is refused with
//!    `REFUSED_AFTER_INTERNAL_ERROR` and nothing else is refused for that reason;
//!    the final save still succeeds; the internal-error notice is raised.
//! 6. Loads (and creates) commit in the order they started (`Lifecycle`,
//!    `Sessions`, no successful close): the open project at the end is the file of
//!    the successful load or create whose `start_new_load` came last, the one the
//!    frontend shows.
//! 7. On the lock order alone: a settled read (`SettledRead`, `SettledSnapshot`,
//!    `Measure`) never takes `project` after another thread's edit passed the
//!    `park_after_mutation` seam and before it pushed its undo entry, so a read
//!    that does not wait is caught even when its answer looks the same.
//! 8. Edit homes (`Lifecycle`, `Sessions`): every marker in every file and in the
//!    open project was made while that file was the open one - a command applies
//!    to the project open when it took `command_lock`, never to one a later load,
//!    create or close committed.
//! 9. The open project's database handle is its file: a probe write through it
//!    succeeds and is in the file at the project's path (a stale handle).
//!
//! # Running
//!
//! The CI seeds (`ci_seeds_*`) run in the normal `cargo test` job. On demand:
//! `FZ_INT_SEEDS=<n>` (from `FZ_INT_FIRST`, default 1000) or `FZ_INT_SECS=<s>`
//! runs a campaign in `on_demand_campaign` (`FZ_INT_MODES=Edits,Reads` narrows
//! the modes, `FZ_INT_STOP_AFTER=<n>` stops at the n-th failing seed);
//! `FZ_INT_SEED=<s>` runs one seed of every mode with its trace. A failure prints
//! the seed, the oracle class, the shrunk op lists and the schedule; add the seed
//! to `REGRESSION_SEEDS`.
//!
//! All data is synthetic, built in code.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::{AppState, DisplaySettingTarget};
use crate::core::{Command, CommandErrorCode, CommandResult};
use crate::models::interleave::{self, Outcome, SplitMix, Worker};
use crate::models::poison_policy::LockCell;
use crate::models::{CreationStamp, Project, ShotData};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

/// Seeds every CI run replays, per mode: a spread of fresh seeds, plus
/// [`REGRESSION_SEEDS`].
const CI_SEEDS: std::ops::Range<u64> = 0..12;
/// Seeds a campaign failed on, and seeds proven to catch a planted regression of
/// a fixed #576 bug (each one failed with that fix reverted, and passes with it).
/// They name schedules of THIS generator and scheduler: a change to either moves
/// them, so re-derive them (revert the fix, run a campaign, keep a failing seed).
const REGRESSION_SEEDS: &[(Mode, u64)] = &[
    // RV5-1: `save_display_setting` without `command_lock` - a write landing while
    // a save holds the handle is lost (shrinks to one Save and one write).
    (Mode::Edits, 127),
    // RV6b-2: `save_display_setting_for` without `command_lock` - a load commits
    // between the identity check and the write, which lands in another file
    // (shrinks to a write made for CaveA landing in a new project's file).
    (Mode::Sessions, 47),
    // A1: a load committing without `command_lock` - an edit that took the lock
    // while CaveA was open lands in the loaded file (shrinks to one load and one
    // edit). CI seed Sessions 7 catches it too.
    (Mode::Lifecycle, 97),
    // (C2: `settled_read` without `command_lock` - a read inside a command sees
    // the edit's section without its undo entry - is caught by CI seed Edits 11.)
    // RV5-4: `distance_field_checked` on `project_snapshot` - the IPC read takes
    // the project inside another thread's edit (oracle 7; shrinks to one Measure
    // and one edit).
    (Mode::Reads, 56),
    // RV4-R-C2: no cancellation re-check once a load holds `command_lock` - an
    // older load commits after a newer one (shrinks to two loads).
    (Mode::Lifecycle, 251),
    // FZ-FZ-INT-R1, found by the first long campaign: a second load of the open
    // file committed a session on the file a save had replaced (see
    // `a_second_load_of_the_open_file_does_not_commit_a_stale_session`). Of the
    // campaign's three seeds, 11102349 still fails with the fix reverted.
    (Mode::Lifecycle, 1_101_510),
    (Mode::Lifecycle, 11_102_349),
    (Mode::Lifecycle, 13_100_691),
];

const MAX_STEPS: usize = 200_000;
const WATCHDOG: Duration = Duration::from_mins(2);
const CREATED_A: &str = "2026-01-01T10:00:00Z";
const CREATED_B: &str = "2026-02-02T10:00:00Z";
const MARKER: &str = "fz-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Edits,
    Lifecycle,
    /// `Edits` with the IPC reads that compute on a settled snapshot.
    Reads,
    /// `Lifecycle` with project creates and the IPC reads.
    Sessions,
}

impl Mode {
    const ALL: [Self; 4] = [Self::Edits, Self::Lifecycle, Self::Reads, Self::Sessions];

    /// One project for the whole case (a Save As moves it; nothing opens another).
    fn single_project(self) -> bool {
        matches!(self, Self::Edits | Self::Reads)
    }
}

/// The cells an injected crash can poison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cell {
    CommandLock,
    Project,
    UndoStack,
    Database,
    DisplayCache,
}

impl Cell {
    /// A crash here makes undo/redo refuse (`AppState::refuse_after_crash`).
    fn refuses_history(self) -> bool {
        matches!(self, Self::CommandLock | Self::Project | Self::UndoStack)
    }

    fn of(self, state: &AppState) -> &dyn LockCell {
        match self {
            Self::CommandLock => &state.command_lock,
            Self::Project => &state.project,
            Self::UndoStack => &state.undo_stack,
            Self::Database => &state.database,
            Self::DisplayCache => &state.display_settings_cache,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    /// A forward edit adding the marker section `name`.
    Edit(String),
    Undo,
    Redo,
    Save,
    /// Save As to this thread's n-th target.
    SaveAs(u32),
    /// `save_display_setting_for` of this thread's key, value number n.
    Display(u32),
    /// The identity-less `save_display_setting` of this thread's legacy key.
    Legacy(u32),
    SettledRead,
    SettledSnapshot,
    /// An IPC read computed on a settled snapshot (`distance_field_checked`).
    Measure,
    /// File > New: a project at this thread's n-th new file, its first save
    /// deferred when n is even.
    Create(u32),
    /// Load file n of the world (0 CaveA, 1 CaveB, 2 the copy of CaveA).
    Load(usize),
    Close,
    Crash(Cell),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Answer {
    Ok,
    Refused(Option<CommandErrorCode>, String),
    Crashed,
}

impl Answer {
    fn of(result: &CommandResult) -> Self {
        if result.success {
            Self::Ok
        } else {
            Self::Refused(result.code, result.error.clone().unwrap_or_default())
        }
    }
}

#[derive(Debug, Clone)]
struct Record {
    thread: usize,
    index: usize,
    op: Op,
    start: u64,
    end: u64,
    answer: Answer,
    /// For a display write: the value written.
    value: Option<String>,
    /// The op's window in `Outcome::acquired` (the locks it took).
    locks: (usize, usize),
}

#[derive(Debug, Clone)]
struct Case {
    mode: Mode,
    seed: u64,
    threads: Vec<Vec<Op>>,
}

fn display_key(thread: usize) -> String {
    format!("fzKey{thread}")
}

fn legacy_key(thread: usize) -> String {
    format!("fzLegacy{thread}")
}

/// The op lists a seed stands for.
fn generate(mode: Mode, seed: u64) -> Case {
    let salt = match mode {
        Mode::Edits => 0x45_44_49_54,
        Mode::Lifecycle => 0x4c_49_46_45,
        Mode::Reads => 0x52_45_41_44,
        Mode::Sessions => 0x53_45_53_53,
    };
    let mut rng = SplitMix::new(seed ^ salt);
    let crashes = mode == Mode::Edits && rng.below(3) == 0;
    // Half the lifecycle cases are load storms: every thread opens with a load of
    // a file that is not open (CaveB or the copy), so loads overlap.
    let storm = matches!(mode, Mode::Lifecycle | Mode::Sessions) && rng.below(2) == 0;
    let thread_count = 2 + rng.below(3);
    let mut threads = Vec::with_capacity(thread_count);
    for t in 0..thread_count {
        let len = 3 + rng.below(8);
        let mut ops = Vec::with_capacity(len);
        let mut n = 0u32;
        if storm {
            ops.push(Op::Load(1 + rng.below(2)));
        }
        for i in 0..len {
            n += 1;
            let roll = rng.below(100);
            let op = match mode {
                Mode::Edits => match roll {
                    0..=29 => Op::Edit(format!("{MARKER}t{t}-{i}")),
                    30..=39 => Op::Undo,
                    40..=45 => Op::Redo,
                    46..=53 => Op::Save,
                    54..=56 => Op::SaveAs(n),
                    57..=68 => Op::Display(n),
                    69..=74 => Op::Legacy(n),
                    75..=86 => Op::SettledRead,
                    87..=92 => Op::SettledSnapshot,
                    _ if crashes => Op::Crash(
                        [
                            Cell::CommandLock,
                            Cell::Project,
                            Cell::UndoStack,
                            Cell::Database,
                            Cell::DisplayCache,
                        ][rng.below(5)],
                    ),
                    _ => Op::SettledRead,
                },
                Mode::Lifecycle => match roll {
                    0..=19 => Op::Edit(format!("{MARKER}t{t}-{i}")),
                    20..=25 => Op::Undo,
                    26..=29 => Op::Redo,
                    30..=37 => Op::Save,
                    38..=55 => Op::Display(n),
                    56..=65 => Op::SettledRead,
                    66..=69 => Op::SettledSnapshot,
                    70..=89 => Op::Load(rng.below(3)),
                    _ => Op::Close,
                },
                Mode::Reads => match roll {
                    0..=29 => Op::Edit(format!("{MARKER}t{t}-{i}")),
                    30..=37 => Op::Undo,
                    38..=42 => Op::Redo,
                    43..=49 => Op::Save,
                    50..=51 => Op::SaveAs(n),
                    52..=59 => Op::Display(n),
                    60..=79 => Op::Measure,
                    80..=89 => Op::SettledRead,
                    _ => Op::SettledSnapshot,
                },
                Mode::Sessions => match roll {
                    0..=19 => Op::Edit(format!("{MARKER}t{t}-{i}")),
                    20..=24 => Op::Undo,
                    25..=27 => Op::Redo,
                    28..=35 => Op::Save,
                    36..=49 => Op::Display(n),
                    50..=57 => Op::Measure,
                    58..=63 => Op::SettledRead,
                    64..=79 => Op::Load(rng.below(3)),
                    80..=89 => Op::Create(n),
                    _ => Op::Close,
                },
            };
            ops.push(op);
        }
        threads.push(ops);
    }
    Case {
        mode,
        seed,
        threads,
    }
}

/// The `created_at` the wire carries (`DisplaySettingTarget`).
fn wire_created_at(project: &Project) -> String {
    project.created_at.as_str().to_owned()
}

/// What the frontend would name the open project as, now.
fn identity_now(state: &AppState) -> Option<DisplaySettingTarget> {
    state.project_snapshot().map(|p| DisplaySettingTarget {
        created_at: wire_created_at(&p),
        file_path: p
            .file_path
            .as_ref()
            .map(|f| f.to_string_lossy().into_owned()),
    })
}

/// The file a display value was made for, as written into the value.
fn tag_of(identity: Option<&DisplaySettingTarget>) -> String {
    match identity {
        None => "none".to_owned(),
        Some(DisplaySettingTarget {
            file_path: None, ..
        }) => "unsaved".to_owned(),
        Some(DisplaySettingTarget {
            file_path: Some(path),
            ..
        }) => stem(Path::new(path)),
    }
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The file a `Create(n)` of `thread` makes its project at.
fn new_file(dir: &Path, thread: usize, n: u32) -> PathBuf {
    dir.join(format!("New t{thread} n{n}.stalx"))
}

/// The marker section names of `project`, in section order (duplicates kept).
fn markers_of(project: &Project) -> Vec<String> {
    project
        .sections
        .iter()
        .filter_map(|s| s.name.as_deref())
        .filter(|n| n.starts_with(MARKER))
        .map(str::to_owned)
        .collect()
}

/// A saved project file with nothing in it.
fn write_project(path: &Path, name: &str, created_at: &str) {
    let mut project = Project::new(name.to_owned(), None);
    project.created_at = CreationStamp::parse(created_at).expect("timestamp");
    let mut db = Database::open(path).expect("create project file");
    db.save_project(&project).expect("write project file");
}

/// Shared by every worker of one run.
struct Ctx {
    state: Arc<AppState>,
    dir: PathBuf,
    files: Vec<PathBuf>,
    seq: AtomicU64,
    records: Mutex<Vec<Record>>,
    violations: Mutex<Vec<String>>,
}

impl Ctx {
    fn violation(&self, what: String) {
        self.violations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(what);
    }

    fn tick(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst)
    }
}

/// Oracle 2, inside one settled read.
fn settled_check(ctx: &Ctx, thread: usize, index: usize) {
    let state = &ctx.state;
    let (markers, undo) = state.settled_read(|| {
        let markers = state
            .project_snapshot()
            .map(|p| markers_of(&p))
            .unwrap_or_default();
        let undo = state.undo_stack.lock().undo_entries().count();
        (markers, undo)
    });
    if markers.len() != undo {
        ctx.violation(format!(
            "t{thread} op {index}: a settled read saw {} marker sections and {undo} undo entries",
            markers.len()
        ));
    }
}

fn run_op(ctx: &Ctx, thread: usize, index: usize, op: &Op) -> (Answer, Option<String>) {
    let state = &ctx.state;
    match op {
        Op::Edit(name) => {
            let result = state.execute(Command::CreateSection {
                name: Some(name.clone()),
                surveyor: None,
                date: None,
                color: "#336699".to_owned(),
                direction: None,
                shots: vec![
                    ShotData {
                        length: 3.0,
                        ..Default::default()
                    };
                    2
                ],
            });
            (Answer::of(&result), None)
        }
        Op::Undo => (Answer::of(&state.undo()), None),
        Op::Redo => (Answer::of(&state.redo()), None),
        Op::Save => (
            Answer::of(&state.execute(Command::SaveProject { path: None })),
            None,
        ),
        Op::SaveAs(n) => {
            let path = ctx.dir.join(format!("SaveAs t{thread} n{n}.stalx"));
            (
                Answer::of(&state.execute(Command::SaveProject { path: Some(path) })),
                None,
            )
        }
        Op::Display(n) => {
            let identity = identity_now(state);
            let value = format!("{}|t{thread}|{n}", tag_of(identity.as_ref()));
            let answer = match state.save_display_setting_for(
                identity.as_ref(),
                &display_key(thread),
                &value,
            ) {
                Ok(()) => Answer::Ok,
                Err(e) => Answer::Refused(Some(e.code), e.message),
            };
            (answer, Some(value))
        }
        Op::Legacy(n) => {
            let value = format!("legacy|t{thread}|{n}");
            let answer = match state.save_display_setting(&legacy_key(thread), &value) {
                Ok(()) => Answer::Ok,
                Err(e) => Answer::Refused(None, e.to_string()),
            };
            (answer, Some(value))
        }
        Op::SettledRead => {
            settled_check(ctx, thread, index);
            (Answer::Ok, None)
        }
        Op::SettledSnapshot => {
            if let Some(snapshot) = state.settled_snapshot() {
                let markers = markers_of(&snapshot);
                let unique: BTreeSet<&String> = markers.iter().collect();
                if unique.len() != markers.len() {
                    ctx.violation(format!(
                        "t{thread} op {index}: a settled snapshot holds a marker twice: {markers:?}"
                    ));
                }
                let verdict = AppState::integrity_check_of(&snapshot);
                if !verdict.messages.is_empty() {
                    ctx.violation(format!(
                        "t{thread} op {index}: a settled snapshot fails the integrity check: {:?}",
                        verdict.messages
                    ));
                }
            }
            (Answer::Ok, None)
        }
        Op::Load(file) => {
            let path = ctx.files.get(*file).cloned().unwrap_or_default();
            match state.do_load_project_full(path) {
                Ok(_) => (Answer::Ok, None),
                Err(e) => (Answer::Refused(None, format!("{e:?}")), None),
            }
        }
        Op::Close => (Answer::of(&state.execute(Command::CloseProject)), None),
        Op::Measure => match state.distance_field_checked(None) {
            Ok(_) => (Answer::Ok, None),
            Err(e) => (Answer::Refused(None, e), None),
        },
        Op::Create(n) => {
            let created = state.do_create_project_full(
                format!("New t{thread} n{n}"),
                None,
                None,
                Some(new_file(&ctx.dir, thread, *n)),
                n % 2 == 0,
            );
            match created {
                Ok(_) => (Answer::Ok, None),
                Err(e) => (Answer::Refused(None, format!("{e:?}")), None),
            }
        }
        Op::Crash(cell) => {
            let cell = cell.of(state);
            let crashed = panic::catch_unwind(AssertUnwindSafe(|| {
                cell.hold(&mut || panic!("synthetic crash in a cell (#576 FZ-INT)"));
            }));
            assert!(crashed.is_err(), "the injected crash panics");
            (Answer::Crashed, None)
        }
    }
}

/// One run of `case`, replaying `replay` (see `interleave::run`). Returns the
/// violations (empty: the case passed), the outcome and the records.
fn execute_case(case: &Case, replay: &[u32]) -> (Vec<String>, Outcome, Vec<Record>) {
    let temp = TempDir::new().expect("temp dir");
    let dir = temp.path().to_path_buf();
    let files = vec![
        dir.join("CaveA.stalx"),
        dir.join("CaveB.stalx"),
        dir.join("CaveA copy.stalx"),
    ];
    write_project(&files[0], "CaveA", CREATED_A);
    write_project(&files[1], "CaveB", CREATED_B);
    write_project(&files[2], "CaveA", CREATED_A);
    let state = Arc::new(state_with_null_sink());
    if let Err(e) = state.do_load_project_full(files[0].clone()) {
        panic!("the world's first load failed: {e:?}");
    }
    let ctx = Arc::new(Ctx {
        state,
        dir,
        files,
        seq: AtomicU64::new(0),
        records: Mutex::new(Vec::new()),
        violations: Mutex::new(Vec::new()),
    });
    let workers: Vec<Worker> = case
        .threads
        .iter()
        .enumerate()
        .map(|(thread, ops)| {
            let ctx = Arc::clone(&ctx);
            let ops = ops.clone();
            Box::new(move || {
                for (index, op) in ops.iter().enumerate() {
                    let start = ctx.tick();
                    let first_lock = interleave::acquired_so_far().unwrap_or(0);
                    let (answer, value) = run_op(&ctx, thread, index, op);
                    let last_lock = interleave::acquired_so_far().unwrap_or(0);
                    let end = ctx.tick();
                    ctx.records
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(Record {
                            thread,
                            index,
                            op: op.clone(),
                            start,
                            end,
                            answer,
                            value,
                            locks: (first_lock, last_lock),
                        });
                }
            }) as Worker
        })
        .collect();
    let outcome = interleave::run(case.seed, replay, MAX_STEPS, WATCHDOG, workers);
    let records = ctx
        .records
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let mut violations = ctx
        .violations
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(failure) = &outcome.failure {
        violations.push(format!("scheduler: {failure:?}"));
    }
    for (thread, message) in &outcome.panics {
        violations.push(format!("t{thread} panicked: {message}"));
    }
    // A run the scheduler aborted left its threads unwound mid-command: no
    // post-run state is meaningful then.
    if outcome.failure.is_none() {
        let after = panic::catch_unwind(AssertUnwindSafe(|| {
            post_run(case, &ctx, &records, &outcome)
        }));
        match after {
            Ok(found) => violations.extend(found),
            Err(payload) => violations.push(format!(
                "the post-run checks panicked: {}",
                payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .unwrap_or_default()
            )),
        }
    }
    drop(ctx);
    drop(temp);
    (violations, outcome, records)
}

/// The last acknowledged value of each display key: per the file it was made for
/// (`Lifecycle`), or per key alone (`Edits`, where a Save As moves the project and
/// every write follows it).
fn last_acked(mode: Mode, records: &[Record]) -> BTreeMap<(String, String), String> {
    let mut sorted: Vec<&Record> = records.iter().collect();
    sorted.sort_by_key(|r| (r.thread, r.index));
    let mut last = BTreeMap::new();
    for r in sorted {
        let (Some(value), Answer::Ok) = (&r.value, &r.answer) else {
            continue;
        };
        let key = match r.op {
            Op::Display(_) => display_key(r.thread),
            Op::Legacy(_) => legacy_key(r.thread),
            _ => continue,
        };
        let tag = match (mode, &r.op) {
            (mode, _) if mode.single_project() => "open".to_owned(),
            (_, Op::Legacy(_)) => "legacy".to_owned(),
            _ => value.split('|').next().unwrap_or_default().to_owned(),
        };
        last.insert((key, tag), value.clone());
    }
    last
}

/// Oracles 3-5 once every thread finished (plain locks: this thread is not
/// scheduled).
fn post_run(case: &Case, ctx: &Ctx, records: &[Record], outcome: &Outcome) -> Vec<String> {
    let mut found = Vec::new();
    let state = &ctx.state;
    let crashes: Vec<(Cell, u64)> = records
        .iter()
        .filter_map(|r| match r.op {
            Op::Crash(cell) => Some((cell, r.end)),
            _ => None,
        })
        .collect();
    let history_crash_end = crashes
        .iter()
        .filter(|(cell, _)| cell.refuses_history())
        .map(|(_, end)| *end)
        .min();
    for r in records {
        if !matches!(r.op, Op::Undo | Op::Redo) {
            continue;
        }
        let refused_after_crash = matches!(
            r.answer,
            Answer::Refused(Some(CommandErrorCode::RefusedAfterInternalError), _)
        );
        match history_crash_end {
            Some(end) if r.start > end && !refused_after_crash => found.push(format!(
                "t{} op {}: {:?} after a crash in a history cell answered {:?} (expected REFUSED_AFTER_INTERNAL_ERROR)",
                r.thread, r.index, r.op, r.answer
            )),
            None if refused_after_crash => found.push(format!(
                "t{} op {}: {:?} refused after an internal error with no history cell crashed",
                r.thread, r.index, r.op
            )),
            _ => {}
        }
    }

    found.extend(settled_windows(records, outcome));
    let open = state.project_snapshot();
    if case.mode.single_project() {
        let created: BTreeSet<String> = records
            .iter()
            .filter(|r| r.answer == Answer::Ok)
            .filter_map(|r| match &r.op {
                Op::Edit(name) => Some(name.clone()),
                _ => None,
            })
            .collect();
        let acked = |want: &Op| {
            records
                .iter()
                .filter(|r| r.answer == Answer::Ok && r.op == *want)
                .count()
        };
        let (undone, redone) = (acked(&Op::Undo), acked(&Op::Redo));
        let markers = open.as_ref().map(|p| markers_of(p)).unwrap_or_default();
        let expected = created.len() + redone - undone.min(created.len() + redone);
        if markers.len() != expected {
            found.push(format!(
                "the project holds {} markers; {} edits, {undone} undos and {redone} redos were acknowledged (expected {expected})",
                markers.len(),
                created.len()
            ));
        }
        if let Some(stray) = markers.iter().find(|m| !created.contains(*m)) {
            found.push(format!(
                "marker {stray} is in the project but its edit was not acknowledged"
            ));
        }
        for r in records {
            if matches!(r.op, Op::Display(_)) && r.answer != Answer::Ok {
                found.push(format!(
                    "t{} op {}: a display write for the open project was refused with no switch: {:?}",
                    r.thread, r.index, r.answer
                ));
            }
        }
        for ((key, _), value) in last_acked(case.mode, records) {
            let live = state.get_display_setting(&key).ok().flatten();
            if live.as_deref() != Some(value.as_str()) {
                found.push(format!(
                    "display {key}: live value {live:?}, last acknowledged write {value:?}"
                ));
            }
        }
    }
    let mut settled = Ctx {
        state: Arc::clone(&ctx.state),
        dir: ctx.dir.clone(),
        files: Vec::new(),
        seq: AtomicU64::new(0),
        records: Mutex::new(Vec::new()),
        violations: Mutex::new(Vec::new()),
    };
    settled_check(&settled, usize::MAX, usize::MAX);
    found.append(
        settled
            .violations
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner),
    );

    // Before the final save, which would paper over a handle to another file.
    found.extend(handle_is_file(state, open.as_deref()));
    // The final save, and the file read back.
    let saved_path = open.as_ref().and_then(|p| p.file_path.clone());
    if let Some(path) = saved_path {
        let saved = state.execute(Command::SaveProject { path: None });
        if saved.success {
            found.extend(read_back(state, &path));
        } else {
            found.push(format!(
                "the final save failed: {:?} {:?}",
                saved.code, saved.error
            ));
        }
    }
    if crashes.iter().any(|(cell, _)| *cell != Cell::DisplayCache)
        && state.internal_error_status().is_none()
    {
        found.push("a cell crashed and no internal-error notice was raised".to_owned());
    }
    found.extend(display_files(case, ctx, records));
    if !case.mode.single_project() {
        found.extend(edit_homes(ctx, records, outcome));
        found.extend(load_order(ctx, records, outcome));
    }
    found
}

/// Oracle 6 (`Lifecycle`, `Sessions`, RV4-R-C2 class): loads (and creates, which
/// share the load token) commit in the order they STARTED
/// (`start_new_load`, which cancels the load in flight). The frontend shows the
/// project of the load it issued last, so a backend that let an older load commit
/// after a newer one sits on a project the user moved past. With no successful
/// close in the case, the open project at the end is the file of the successful
/// load whose `start_new_load` (its first `active_loading` lock) came last.
fn load_order(ctx: &Ctx, records: &[Record], outcome: &Outcome) -> Vec<String> {
    let closed = records
        .iter()
        .any(|r| r.op == Op::Close && r.answer == Answer::Ok);
    if closed {
        return Vec::new();
    }
    let started_at = |r: &Record| {
        outcome
            .acquired
            .iter()
            .enumerate()
            .skip(r.locks.0)
            .take(r.locks.1.saturating_sub(r.locks.0))
            .find(|(_, (thread, cell))| *thread == r.thread && *cell == "active_loading")
            .map(|(at, _)| at)
    };
    let newest = records
        .iter()
        .filter(|r| r.answer == Answer::Ok && matches!(r.op, Op::Load(_) | Op::Create(_)))
        .filter_map(|r| started_at(r).map(|at| (at, r)))
        .max_by_key(|(at, _)| *at);
    let Some((_, r)) = newest else {
        return Vec::new();
    };
    let expected = slot_file(ctx, r).flatten();
    let open = ctx
        .state
        .project_snapshot()
        .and_then(|p| p.file_path.clone());
    if open == expected {
        return Vec::new();
    }
    vec![format!(
        "the newest successful load or create (t{} op {}, {:?}) did not leave its file open: open is {:?} (RV4-R-C2 class)",
        r.thread,
        r.index,
        expected.as_deref().map(stem),
        open.as_deref().map(stem)
    )]
}

/// The first `cell` the thread of `r` took inside `r`'s window, at or after
/// point `from` of `Outcome::acquired`.
fn first_in(outcome: &Outcome, r: &Record, cell: &str, from: usize) -> Option<usize> {
    outcome
        .acquired
        .iter()
        .enumerate()
        .take(r.locks.1)
        .skip(from.max(r.locks.0))
        .find(|(_, (thread, taken))| *thread == r.thread && *taken == cell)
        .map(|(at, _)| at)
}

/// Oracle 7 (every mode, C2 / RV5-4 class), on the lock order alone: a settled
/// read takes its project snapshot between commands. A forward edit holds
/// `command_lock` from before its mutation until after its undo push and passes
/// the `park_after_mutation` seam in between, so a read that took `project` after
/// another thread passed that seam and before that thread pushed its undo entry
/// computed on a project no user ever sees - whatever the read returns. This is
/// what catches a read that does not wait (`distance_field_checked` on
/// `project_snapshot`) even when its answer happens to look the same.
fn settled_windows(records: &[Record], outcome: &Outcome) -> Vec<String> {
    let edits: Vec<(&Record, usize, usize)> = records
        .iter()
        .filter(|e| matches!(e.op, Op::Edit(_)) && e.answer == Answer::Ok)
        .filter_map(|e| {
            let mutated = first_in(outcome, e, "park_after_mutation", 0)?;
            let pushed = first_in(outcome, e, "undo_stack", mutated + 1).unwrap_or(e.locks.1);
            Some((e, mutated, pushed))
        })
        .collect();
    let mut found = Vec::new();
    for r in records
        .iter()
        .filter(|r| matches!(r.op, Op::SettledRead | Op::SettledSnapshot | Op::Measure))
    {
        let Some(read) = first_in(outcome, r, "project", 0) else {
            continue;
        };
        for (e, mutated, pushed) in &edits {
            if e.thread != r.thread && *mutated < read && read < *pushed {
                found.push(format!(
                    "t{} op {}: a settled read ({:?}) took the project at point {read}, inside t{} op {}'s edit (mutated by {mutated}, undo entry pushed at {pushed}) (C2 / RV5-4 class)",
                    r.thread, r.index, r.op, e.thread, e.index
                ));
            }
        }
    }
    found
}

/// The file a project-slot op leaves open when it succeeds: `Some(None)` for a
/// close, `None` for an op that is not one.
fn slot_file(ctx: &Ctx, r: &Record) -> Option<Option<PathBuf>> {
    match r.op {
        Op::Load(file) => Some(ctx.files.get(file).cloned()),
        Op::Create(n) => Some(Some(new_file(&ctx.dir, r.thread, n))),
        Op::Close => Some(None),
        _ => None,
    }
}

/// Oracle 8 (`Lifecycle`, `Sessions`): a command applies to the project that was
/// open when it took `command_lock`, never to one a load, create or close
/// committed after it started. Each acknowledged edit's home is the file the
/// last slot commit before its `command_lock` left open; every marker in every
/// file of the world, and in the open project, must have been made while that
/// file was the open one. A commit is placed at the slot op's last `project`
/// lock - the slot swap, or a later point of the same `command_lock` span - not
/// at its `command_lock`, so a commit that stopped taking that lock is still
/// placed where it happened.
fn edit_homes(ctx: &Ctx, records: &[Record], outcome: &Outcome) -> Vec<String> {
    let mut commits: Vec<(usize, Option<PathBuf>)> = records
        .iter()
        .filter(|r| r.answer == Answer::Ok)
        .filter_map(|r| {
            let file = slot_file(ctx, r)?;
            let at = outcome
                .acquired
                .iter()
                .enumerate()
                .take(r.locks.1)
                .skip(r.locks.0)
                .filter(|(_, (thread, cell))| *thread == r.thread && *cell == "project")
                .map(|(at, _)| at)
                .next_back()?;
            Some((at, file))
        })
        .collect();
    commits.sort_by_key(|(at, _)| *at);
    let homes: BTreeMap<String, Option<PathBuf>> = records
        .iter()
        .filter(|r| r.answer == Answer::Ok)
        .filter_map(|r| {
            let Op::Edit(name) = &r.op else {
                return None;
            };
            let took = first_in(outcome, r, "command_lock", 0)?;
            let home = commits
                .iter()
                .rev()
                .find(|(at, _)| *at < took)
                .map_or_else(|| ctx.files.first().cloned(), |(_, file)| file.clone());
            Some((name.clone(), home))
        })
        .collect();
    let mut places: Vec<(String, Option<PathBuf>, Vec<String>)> = Vec::new();
    if let Some(open) = ctx.state.project_snapshot() {
        places.push((
            "the open project".to_owned(),
            open.file_path.clone(),
            markers_of(&open),
        ));
    }
    if let Ok(entries) = std::fs::read_dir(&ctx.dir) {
        for path in entries.flatten().map(|e| e.path()) {
            if path.extension().is_none_or(|e| e != "stalx") {
                continue;
            }
            if let Ok(project) = Database::open_readonly(&path).and_then(|db| db.load_project()) {
                places.push((stem(&path), Some(path), markers_of(&project)));
            }
        }
    }
    let mut found = Vec::new();
    for (place, file, markers) in places {
        for marker in markers {
            match homes.get(&marker) {
                Some(home) if *home == file => {}
                Some(home) => found.push(format!(
                    "{place}: marker {marker} was made while {:?} was open, not this file",
                    home.as_deref().map(stem)
                )),
                None => found.push(format!(
                    "{place}: marker {marker} is there but its edit was not acknowledged"
                )),
            }
        }
    }
    found
}

/// Oracle 9 (every mode): the open project's database handle is its file. A
/// session holding a handle on a file a save has since renamed another over
/// writes every display setting into the deleted file, and its next save builds
/// the side tables from there and drops the writes the file at the path holds.
/// One probe write through the handle must be in the file at the project's path.
fn handle_is_file(state: &AppState, open: Option<&Project>) -> Vec<String> {
    let Some(path) = open.and_then(|p| p.file_path.clone()) else {
        return Vec::new();
    };
    if state.database.lock().is_none() {
        return Vec::new();
    }
    let probe = "fzProbe";
    if let Err(e) = state.save_display_setting(probe, "probe") {
        return vec![format!(
            "a probe write through the open project's database handle failed ({e}): a stale handle (SQLite refuses a write to a file replaced since it was opened)"
        )];
    }
    let on_disk = Database::open_readonly(&path)
        .and_then(|db| db.load_display_settings())
        .ok()
        .and_then(|settings| settings.get(probe).cloned());
    if on_disk.as_deref() == Some("probe") {
        return Vec::new();
    }
    vec![format!(
        "{}: a write through the open project's database handle is not in the file at its path (on disk {on_disk:?}): a stale handle on a file a save replaced",
        stem(&path)
    )]
}

/// The open project against its file read back from disk (oracle 3).
fn read_back(state: &AppState, path: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let memory = state.get_project_state();
    let disk = Database::open_readonly(path).and_then(|db| db.load_project());
    match (memory, disk) {
        (Some(memory), Ok(disk)) => {
            let (m, d) = (markers_of(&memory), markers_of(&disk));
            if m != d {
                found.push(format!(
                    "saved file markers {d:?} differ from the project's {m:?}"
                ));
            }
            let mem_sections = serde_json::to_value(&memory.sections).unwrap_or_default();
            let disk_sections = serde_json::to_value(&disk.sections).unwrap_or_default();
            if mem_sections != disk_sections {
                found.push("the saved file's sections differ from the project's".to_owned());
            }
        }
        (memory, disk) => found.push(format!(
            "read-back failed: project open {}, file {:?}",
            memory.is_some(),
            disk.err()
        )),
    }
    found
}

/// Oracle 4 on disk: every file holds only values made for it, and the last
/// acknowledged value made for it.
fn display_files(case: &Case, ctx: &Ctx, records: &[Record]) -> Vec<String> {
    let mut found = Vec::new();
    let last = last_acked(case.mode, records);
    let Ok(entries) = std::fs::read_dir(&ctx.dir) else {
        return vec!["cannot list the world's files".to_owned()];
    };
    let open_path = ctx
        .state
        .project_snapshot()
        .and_then(|p| p.file_path.clone());
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "stalx") {
            continue;
        }
        let file = stem(&path);
        let settings =
            match Database::open_readonly(&path).and_then(|db| db.load_display_settings()) {
                Ok(settings) => settings,
                Err(e) => {
                    found.push(format!("{file}: display settings unreadable: {e}"));
                    continue;
                }
            };
        for (key, value) in &settings {
            let tag = value.split('|').next().unwrap_or_default();
            if !case.mode.single_project() && key.starts_with("fzKey") && tag != file {
                found.push(format!(
                    "{file}: display {key} = {value:?} was made for {tag:?} (RV6b-2 / NF-W8FE-3 class)"
                ));
            }
        }
        // Edits mode: the open file carries the last value of every key (Save As
        // moves the project; the file it is at now is the one that counts).
        let relevant = |tag: &str| {
            if case.mode.single_project() {
                open_path.as_deref() == Some(path.as_path())
            } else {
                tag == file
            }
        };
        for ((key, tag), value) in &last {
            if tag == "none" || tag == "unsaved" || !relevant(tag) {
                continue;
            }
            if settings.get(key) != Some(value) {
                found.push(format!(
                    "{file}: display {key} on disk is {:?}, last acknowledged write {value:?} (RV5-1 class)",
                    settings.get(key)
                ));
            }
        }
    }
    found
}

/// Case minus the op at `(thread, index)`; `None` when that empties the case.
fn without(case: &Case, thread: usize, index: usize) -> Option<Case> {
    let mut smaller = case.clone();
    let ops = smaller.threads.get_mut(thread)?;
    if index >= ops.len() {
        return None;
    }
    ops.remove(index);
    // An emptied thread stays (and finishes at once): thread numbers are what a
    // replayed schedule names.
    if smaller.threads.iter().filter(|ops| !ops.is_empty()).count() < 2 {
        return None;
    }
    Some(smaller)
}

/// Alternate schedules a smaller case gets when the recorded one no longer fails
/// it: a removed op shifts the candidate lists, so the old choices often steer
/// elsewhere.
const SHRINK_RESEEDS: u64 = 24;

/// Which oracle a violation comes from, so a shrink keeps chasing the bug it
/// started with instead of drifting to another one.
fn class_of(violation: &str) -> &'static str {
    const CLASSES: [(&str, &str); 12] = [
        ("scheduler:", "scheduler"),
        ("panicked", "panic"),
        ("settled", "settled-read"),
        ("RV4-R-C2", "load-order"),
        ("stale handle", "handle"),
        ("made while", "edit-home"),
        ("display", "display"),
        ("marker", "edits"),
        ("REFUSED_AFTER", "poison"),
        ("refused after an internal error", "poison"),
        ("notice", "poison"),
        ("final save", "save"),
    ];
    CLASSES
        .iter()
        .find(|(needle, _)| violation.contains(needle))
        .map_or("other", |(_, class)| class)
}

/// `smaller` under the recorded `choices`, then under fresh schedules: the first
/// run that still fails with a violation of `class`, with the case (its seed).
fn still_fails(
    smaller: &Case,
    choices: &[u32],
    class: &str,
) -> Option<(Case, Vec<String>, Outcome)> {
    let same = |violations: &[String]| violations.iter().any(|v| class_of(v) == class);
    let (violations, outcome, _) = execute_case(smaller, choices);
    if same(&violations) {
        return Some((smaller.clone(), violations, outcome));
    }
    for k in 1..=SHRINK_RESEEDS {
        let mut reseeded = smaller.clone();
        reseeded.seed = smaller.seed.wrapping_add(k.wrapping_mul(0x9E37_79B9));
        let (violations, outcome, _) = execute_case(&reseeded, &[]);
        if same(&violations) {
            return Some((reseeded, violations, outcome));
        }
    }
    None
}

/// Shrink a failing case: drop ops one at a time while it still fails (under the
/// recorded schedule, which names threads, or a fresh one).
fn shrink(
    case: &Case,
    choices: Vec<u32>,
    budget: Duration,
) -> (Case, Vec<u32>, Vec<String>, Outcome) {
    let started = Instant::now();
    let mut best = case.clone();
    let mut best_choices = choices;
    let (mut best_violations, mut best_outcome, _) = execute_case(&best, &best_choices);
    let class = best_violations.first().map_or("other", |v| class_of(v));
    let mut progress = true;
    while progress && started.elapsed() < budget {
        progress = false;
        for thread in (0..best.threads.len()).rev() {
            let len = best.threads.get(thread).map_or(0, Vec::len);
            for index in (0..len).rev() {
                if started.elapsed() >= budget {
                    break;
                }
                let Some(smaller) = without(&best, thread, index) else {
                    continue;
                };
                if let Some((failing, violations, outcome)) =
                    still_fails(&smaller, &best_choices, class)
                {
                    best = failing;
                    best_choices = outcome.choices.clone();
                    best_violations = violations;
                    best_outcome = outcome;
                    progress = true;
                }
            }
        }
    }
    (best, best_choices, best_violations, best_outcome)
}

/// A schedule as runs of one thread: `t0x5 t1x2 ...` (who held the baton for how
/// many consecutive scheduling points).
/// The inverse of [`runs_of`]: a printed schedule back into replay choices.
fn choices_from_runs(runs: &str) -> Vec<u32> {
    runs.split_whitespace()
        .filter_map(|run| {
            let (thread, len) = run.strip_prefix('t')?.split_once('x')?;
            Some((thread.parse::<u32>().ok()?, len.parse::<usize>().ok()?))
        })
        .flat_map(|(thread, len)| std::iter::repeat_n(thread, len))
        .collect()
}

fn runs_of(choices: &[u32]) -> String {
    let mut out = String::new();
    let mut at = 0;
    while let Some(&thread) = choices.get(at) {
        let len = choices[at..].iter().take_while(|&&c| c == thread).count();
        let _ = write!(out, "t{thread}x{len} ");
        at += len;
    }
    out
}

/// What happened to each op kind: `"Undo ok"`, `"Load refused"`, ... -> count.
type Tally = BTreeMap<String, usize>;

fn tally(records: &[Record], into: &mut Tally) {
    for r in records {
        let kind = format!("{:?}", r.op);
        let kind = kind.split('(').next().unwrap_or_default();
        let answer = match &r.answer {
            Answer::Ok => "ok".to_owned(),
            Answer::Crashed => "crashed".to_owned(),
            Answer::Refused(Some(code), _) => format!("refused {code:?}"),
            Answer::Refused(None, _) => "refused".to_owned(),
        };
        *into.entry(format!("{kind} {answer}")).or_default() += 1;
    }
}

/// Run one seed; on failure shrink it and return the report.
fn check_seed(mode: Mode, seed: u64, seen: &mut Tally) -> Result<usize, String> {
    let case = generate(mode, seed);
    let (violations, outcome, records) = execute_case(&case, &[]);
    tally(&records, seen);
    if violations.is_empty() {
        return Ok(outcome.trace.len());
    }
    let (small, choices, small_violations, small_outcome) =
        shrink(&case, outcome.choices, Duration::from_mins(2));
    let mut report = format!(
        "FZ-INT {mode:?} seed {seed} FAILED (class {}):\n  {}\n  original: {} ops on {} threads\n  shrunk to:\n",
        violations.first().map_or("other", |v| class_of(v)),
        violations.join("\n  "),
        case.threads.iter().map(Vec::len).sum::<usize>(),
        case.threads.len(),
    );
    for (t, ops) in small.threads.iter().enumerate() {
        let _ = writeln!(report, "    t{t}: {ops:?}");
    }
    let _ = write!(
        report,
        "  shrunk violations:\n    {}\n  replay: scheduler seed {} with choices ({} points) {}\n  last points:\n{}",
        small_violations.join("\n    "),
        small.seed,
        small_outcome.trace.len(),
        runs_of(&choices),
        small_outcome.trace_tail(40),
    );
    Err(report)
}

fn run_seeds(
    seeds: impl Iterator<Item = (Mode, u64)>,
    seen: &mut Tally,
) -> (usize, usize, Vec<String>) {
    let (mut cases, mut points, mut failures) = (0, 0, Vec::new());
    for (mode, seed) in seeds {
        cases += 1;
        match check_seed(mode, seed, seen) {
            Ok(n) => points += n,
            Err(report) => failures.push(report),
        }
    }
    (cases, points, failures)
}

fn ci(mode: Mode) {
    let started = Instant::now();
    let seeds = CI_SEEDS.map(move |s| (mode, s)).chain(
        REGRESSION_SEEDS
            .iter()
            .copied()
            .filter(move |(m, _)| *m == mode),
    );
    let mut seen = Tally::new();
    let (cases, points, failures) = run_seeds(seeds, &mut seen);
    println!(
        "[FZ-INT] {mode:?}: {cases} cases, {points} scheduling points, {} failures, {:?}\n  answers: {seen:?}",
        failures.len(),
        started.elapsed()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn ci_seeds_edits() {
    ci(Mode::Edits);
}

#[test]
fn ci_seeds_lifecycle() {
    ci(Mode::Lifecycle);
}

#[test]
fn ci_seeds_reads() {
    ci(Mode::Reads);
}

#[test]
fn ci_seeds_sessions() {
    ci(Mode::Sessions);
}

/// [#576 FZ-FZ-INT-R1] Two loads of ONE file racing: the second passed the
/// "already open" check (made when it STARTED) before the first committed, read
/// the file, and committed after the first session had saved - a save renames a
/// fresh file over the path - so the session it installed held a database handle
/// on the deleted file. Every later write of the session (a display setting, an
/// ignored warning, ...) went into that deleted file, and the next save, built
/// from it, dropped the write made through the first session. Found by the
/// campaign (host seed 14100050 shrunk to this case); replayed here with its
/// schedule. The load now re-checks "already open" under `command_lock` and
/// answers ALREADY_OPEN, as it does when it starts after the first committed.
#[test]
fn a_second_load_of_the_open_file_does_not_commit_a_stale_session() {
    let case = Case {
        mode: Mode::Lifecycle,
        seed: 14_100_050,
        threads: vec![
            vec![Op::Load(2), Op::Save, Op::Display(2)],
            vec![Op::Load(2)],
        ],
    };
    let schedule = choices_from_runs(
        "t1x2 t0x9 t1x1 t0x1 t1x1 t0x3 t1x3 t0x2 t1x2 t0x1 t1x1 t0x1 t1x1 t0x6 t1x1 \
         t0x14 t1x1 t0x5 t1x1 t0x1 t1x14",
    );
    let (violations, _, records) = execute_case(&case, &schedule);
    let answers: Vec<String> = records
        .iter()
        .map(|r| format!("t{} {:?} -> {:?}", r.thread, r.op, r.answer))
        .collect();
    println!("[FZ-FZ-INT-R1] answers: {answers:?}\n  violations: {violations:?}");
    assert!(violations.is_empty(), "{violations:?}");
}

/// A seed is a reproducible bug report only if the same seed runs the same
/// schedule: the same thread at every point, the same locks, the same answers.
#[test]
fn a_seed_replays_the_same_run() {
    for (mode, seed) in [
        (Mode::Edits, 3),
        (Mode::Lifecycle, 4),
        (Mode::Reads, 5),
        (Mode::Sessions, 6),
    ] {
        let case = generate(mode, seed);
        let answers = |records: &[Record]| {
            let mut sorted: Vec<(usize, usize, Answer)> = records
                .iter()
                .map(|r| (r.thread, r.index, r.answer.clone()))
                .collect();
            sorted.sort_by_key(|(t, i, _)| (*t, *i));
            sorted
        };
        let (v1, first, r1) = execute_case(&case, &[]);
        let (v2, second, r2) = execute_case(&case, &[]);
        let (v3, replayed, r3) = execute_case(&case, &first.choices);
        println!(
            "[FZ-INT] {mode:?} seed {seed}: {} points; rerun identical: {}; replay identical: {}",
            first.trace.len(),
            first.trace == second.trace,
            first.trace == replayed.trace
        );
        assert!(v1.is_empty() && v2.is_empty() && v3.is_empty());
        assert_eq!(
            first.choices, second.choices,
            "{mode:?}: the seed alone fixes the schedule"
        );
        assert_eq!(first.trace, second.trace);
        assert_eq!(first.acquired, second.acquired);
        assert_eq!(
            first.trace, replayed.trace,
            "{mode:?}: the choices replay the run"
        );
        assert_eq!(answers(&r1), answers(&r2));
        assert_eq!(answers(&r1), answers(&r3));
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

/// `FZ_INT_SEED`, `FZ_INT_SEEDS` (+ `FZ_INT_FIRST`) or `FZ_INT_SECS`: see the
/// module doc. Without any of them this returns at once.
#[test]
fn on_demand_campaign() {
    if let Some(seed) = env_u64("FZ_INT_SEED") {
        for mode in Mode::ALL {
            let case = generate(mode, seed);
            let (violations, outcome, records) = execute_case(&case, &[]);
            println!(
                "[FZ-INT] {mode:?} seed {seed}: {} threads {:?}\n  {} points, violations {violations:?}\n  answers: {:?}",
                case.threads.len(),
                case.threads,
                outcome.trace.len(),
                records
                    .iter()
                    .map(|r| format!("t{}#{} {:?}", r.thread, r.index, r.answer))
                    .collect::<Vec<_>>()
            );
            assert!(
                violations.is_empty(),
                "{}",
                check_seed(mode, seed, &mut Tally::new())
                    .err()
                    .unwrap_or_default()
            );
        }
        return;
    }
    let first = env_u64("FZ_INT_FIRST").unwrap_or(1000);
    let count = env_u64("FZ_INT_SEEDS");
    let secs = env_u64("FZ_INT_SECS");
    if count.is_none() && secs.is_none() {
        println!("[FZ-INT] on-demand campaign skipped (set FZ_INT_SEEDS or FZ_INT_SECS)");
        return;
    }
    let deadline = secs.map(|s| Instant::now() + Duration::from_secs(s));
    // `FZ_INT_MODES=Edits,Reads` runs those modes only; `FZ_INT_STOP_AFTER=<n>`
    // ends the campaign at its n-th failing seed (a plant hunt, a farm slice).
    let modes: Vec<Mode> = match std::env::var("FZ_INT_MODES") {
        Ok(list) => Mode::ALL
            .into_iter()
            .filter(|m| list.split(',').any(|name| name.trim() == format!("{m:?}")))
            .collect(),
        Err(_) => Mode::ALL.to_vec(),
    };
    let stop_after = env_u64("FZ_INT_STOP_AFTER")
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(usize::MAX);
    let started = Instant::now();
    let mut failures = Vec::new();
    let mut seen = Tally::new();
    let (mut cases, mut points) = (0usize, 0usize);
    let mut seed = first;
    loop {
        if count.is_some_and(|n| seed >= first + n)
            || deadline.is_some_and(|d| Instant::now() >= d)
            || failures.len() >= stop_after
        {
            break;
        }
        for &mode in &modes {
            let (c, p, f) = run_seeds(std::iter::once((mode, seed)), &mut seen);
            cases += c;
            points += p;
            for report in f {
                println!("{report}");
                failures.push(report);
            }
        }
        seed += 1;
    }
    println!(
        "[FZ-INT] campaign seeds {first}..{seed}: {cases} cases, {points} scheduling points, {} failures, {:?}\n  answers: {seen:?}",
        failures.len(),
        started.elapsed()
    );
    assert!(
        failures.is_empty(),
        "{} failing seeds (reports above)",
        failures.len()
    );
}
