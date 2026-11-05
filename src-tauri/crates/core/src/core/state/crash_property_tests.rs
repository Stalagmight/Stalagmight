//! CRASH-TIMING properties: what the save pipeline owes a kill between any two
//! of its steps.
//!
//! # Why this is its own engine
//!
//! Every other engine in this programme measures a pipeline that RAN. The save
//! path's central promise is about one that DID NOT:
//!
//! > All saves use a temp file with atomic rename to prevent data loss on crash
//! > or power failure. (`docs/saving.md`, "Atomic Save")
//!
//! Nothing tested that. The file-format engine
//! ([`super::format_property_tests`]) is the only one that looks at the
//! `.stalx` at all, and it only ever looks at files a save FINISHED writing. So
//! the interesting states - a temp file half-written, a temp file complete but
//! not yet renamed, a stale rollback journal orphaned beside the destination -
//! had no coverage, and neither did the loader's behaviour when it meets one.
//!
//! # The pipeline, as measured
//!
//! `AppState::save_project` (`core/state/project_io.rs:46`), with the line that
//! performs each step. Only the steps that TOUCH THE DISK are listed; the heals
//! and snapshots before them are in-memory:
//!
//! ```text
//!  :213  temp_path = save_path.with_extension("stalx.tmp")   <- the name, exactly
//!  :482  remove a leftover <stem>.stalx.tmp from a previous failed save
//!  :487  Database::open(&temp_path)          - creates the temp database
//!  :496  db.save_project(project)            - the survey, in one transaction
//!  :509  display settings, one row at a time
//!  :521  resolved warnings
//!  :542  speleo_link / undo chain / watermark
//!  :584  the undo and redo stacks
//!  :611  drop(db)                            - the temp database is closed
//!  :652  close the LIVE handle on the destination (Windows rename hazard)
//!  :660  std::fs::rename(&temp_path, &save_path)   <- the atomic moment
//!  :704  Database::open(&save_path)          - reopen for continued editing
//!  :748  note_recent_project(&save_path)
//! ```
//!
//! Two facts fall out of it and shape everything below.
//!
//! **The destination is only ever touched by the rename.** Every write before
//! `:660` goes into `<stem>.stalx.tmp`. So for every pre-rename boundary the
//! destination still holds the OLD file, BYTE for byte - which is why the
//! expectation for those is not "old or new" but the strictly stronger "exactly
//! old".
//!
//! **There is no WAL.** Nothing in `persistence/sqlite/open.rs` sets
//! `journal_mode`, so a `.stalx` uses SQLite's default DELETE journal: a write
//! in progress is accompanied by `<file>-journal`, deleted on commit. That is
//! what makes two of the injected states possible at all (below), and it is why
//! there is no `-wal`/`-shm` case here - the pipeline cannot produce one.
//!
//! # Simulated artifacts, not killed processes
//!
//! A kill is reproduced by CONSTRUCTING the exact set of files it would leave,
//! rather than by racing a signal against a real save. That is not a weakening:
//! `rename(2)` is atomic and the temp is a private file, so the set of on-disk
//! states a kill can leave is small, enumerable, and reachable by copying bytes.
//! It is also deterministic, fast enough for a CI gate, and portable - none of
//! which a signal race is.
//!
//! The two reference files are produced by the REAL pipeline: a synthetic
//! project is built with history, saved (that file's bytes are the OLD state),
//! edited further, and saved again over the same path (those bytes are the NEW
//! state, and are byte-identical to what the temp file held at the instant of
//! the rename, because the rename does not modify what it moves).
//!
//! # The states, and where each one comes from
//!
//! | injected state | the kill lands |
//! |---|---|
//! | destination only | before `:487`, or after any failure cleanup (`:491`, `:499`, `:588`, `:661`) |
//! | destination + temp truncated to N bytes | inside `:496`-`:608` |
//! | destination + temp mid-transaction + its hot journal | inside `:509`-`:608`, after the survey committed |
//! | destination + whole temp | between `:611` and `:660` |
//! | new destination | between `:660` and `:704`/`:748` |
//! | new destination + an orphaned `<stem>.stalx.tmp-journal` | see below |
//! | destination mid-transaction + its hot journal | inside a write through the handle reopened at `:704` |
//!
//! The orphaned-journal row is the one that is not obvious, and it is the reason
//! the journal cases are here at all. `:482` removes a leftover
//! `<stem>.stalx.tmp` before the next save starts - but NOT that temp's
//! `-journal`. So a save killed mid-transaction leaves two files; the next save
//! deletes one of them, creates a fresh temp database at that same name, and
//! SQLite meets a hot journal belonging to a file that no longer exists. That is
//! the classic orphaned-journal hazard, it is reachable from an ordinary crash,
//! and the recovery half of this engine (below) walks straight through it.
//!
//! Measured outcome: benign. The next save succeeds every time, and the stale
//! journal does not survive it - SQLite discards it when the fresh temp database
//! is created at that name, so it does not accumulate. Both facts are counted
//! rather than assumed (`orphaned-journal-cleared-by-the-next-save` in the
//! summary line), and the engine fails if anything disarms the hazard before the
//! recovery save reaches it.
//!
//! # The oracle
//!
//! For each injected state, in order:
//!
//! 1. **No panic.** Every open runs inside `catch_unwind`. A file shape a crash
//!    can leave must not take the process down.
//! 2. **Old or new, never a blend.** The destination is opened and compared - by
//!    the file-format engine's own [`super::format_property_tests::observe`] /
//!    `load_differences`, so the two engines cannot disagree about what a
//!    difference is - against the loaded OLD reference and the loaded NEW
//!    reference. It must match one of them EXACTLY - and every state names WHICH
//!    one, because the pipeline decides it: everything before the rename must be
//!    the old file, everything after it the new one. "Old or new" would be the
//!    weaker property; this catches a state that landed on the wrong side of the
//!    rename as well as one that blended the two.
//! 3. **It opens.** For every state here the destination is either byte-intact
//!    or recoverable by SQLite's own journal replay, so a clean refusal is NOT
//!    an acceptable answer: it would mean a crash cost the user their file.
//! 4. **Recovery forward.** The reopened project is saved again. That save must
//!    succeed, and it must leave NO `.stalx.tmp` behind - `:482` promises the
//!    leftover is removed, and this is what walks the fresh temp into the
//!    orphaned journal described above.
//! 5. **The leftover staging file.** A temp cannot BE the project by accident -
//!    the loader takes an explicit path - but something opens one on purpose:
//!    the recovery UI lists `.tmp` files as restore candidates
//!    (`commands/recovery.rs:37`, `persistence/sqlite/repair.rs:528`) and
//!    `restore_from_backup` copies the chosen one over the project. So opening
//!    one must not panic, and a COMPLETE leftover temp (whole, or
//!    committed-then-interrupted with a journal to roll back) must load as
//!    exactly the save that was interrupted. For a TORN temp the outcome is
//!    measured rather than asserted - see below.
//!
//! # What the torn temps measure
//!
//! The truncation sweep is the one place this engine reports instead of
//! asserting, and the numbers are worth reading. Over 60 seeds x 6 truncation
//! points, 306 of 360 torn temps were REFUSED by the loader, 11 to 13 opened as
//! exactly the interrupted save, and 41 to 43 opened as something else. The
//! range is not sloppiness: four runs of one sweep gave 12/42, 12/42, 11/43 and
//! 13/41, because which row a fixed byte offset cuts through depends on the
//! order the writer lays its rows down in. The 306 did not move, and neither did
//! any total.
//!
//! That last bucket is not a blend of the old and new projects - it cannot be,
//! because a temp is a database built from scratch and never holds a row of the
//! old one. Every instance measured is the same shape: the SURVEY is intact and
//! the undo history is gone (`undo=20` before, `undo=0` after), because the
//! gzipped history blobs are the last thing the save writes and so the first
//! thing a truncation takes. A one-byte-short database opening at all is SQLite
//! zero-filling a short final page, not a defect here.
//!
//! It is recorded rather than asserted because the app is not doing anything
//! wrong with it: the destination is untouched in every one of those states, and
//! a torn scratch file that loses history while keeping the survey is a
//! defensible outcome for a torn scratch file. It is worth WATCHING because
//! `restore_from_backup` performs no validation of its own beyond "the file
//! exists", so the bucket sizes are the honest measure of what that button can
//! install.
//!
//! # What is deliberately NOT injected
//!
//! **A truncated destination.** `rename(2)` is atomic, so no kill can produce
//! one; asserting on a state the pipeline cannot reach would be asserting on
//! SQLite rather than on this app.
//!
//! **A torn rollback journal.** Truncating a journal while keeping the main
//! database file that the same interrupted transaction had already spilled into
//! describes a machine that lost the two files' writes in an order its own
//! `synchronous=FULL` ordering forbids. SQLite would (correctly) ignore the
//! invalid journal and expose the spilled pages, and the resulting blend would
//! be an artifact of the injection rather than a defect. The INTACT hot journal
//! is the faithful case and is injected.
//!
//! **The recent-projects config write.** `:748` is the pipeline's one write to a
//! file that is not the `.stalx`, and it has its own atomic-write scheme with its
//! own abandoned-temp sweep and its own tests (`config/atomic_write.rs:239`,
//! `config/tests.rs:3049-3361`). Duplicating that here would test the same code
//! twice and the save pipeline not at all.
//!
//! # Result
//!
//! No violation. Over 60 seeds - 720 injected states - every destination came
//! back as exactly the old file or exactly the new one, every one of them opened,
//! every one saved again cleanly, and none left a staging file behind. The oracle
//! is not vacuous: a hand-grafted hybrid (the old file with two shot lengths
//! taken from the new one) was rejected on 59 of 60 seeds, naming the changed
//! shots, the metres the adjusted geometry moved and the differing table rows.
//!
//! # Data
//!
//! Everything is generated in code: the projects, both reference files and every
//! injected artifact. No fixture is read, copied or derived from.

#![expect(
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::panic,
    reason = "test module: see docs/linting.md for the per-test-module expect idiom"
)]
#![expect(
    clippy::print_stdout,
    reason = "a property sweep's per-seed summary IS its output, and `cargo test -- --nocapture` is \
              the only channel a test has"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use tempfile::TempDir;

use super::format_property_tests::{
    build_state, dump_database, dump_differences, load_differences, mutate, observe, open_fresh,
    Loaded,
};
use super::fuzz_undo_tests::{sweep_seeds, Rng};
// CONFIG ISOLATION. Every test in this module opens with
// `let _config = isolated_config();`: `AppState::new()` LOADS the global config,
// and every save reaches `note_recent_project` -> `Config::save()`, which WRITES
// it. Config resolution is per-thread (`crate::test_helpers` documents what that
// does and does not protect, and why it is not opt-in), so the guard is no longer
// what keeps these tests off other files; what it buys is ONE dir for the whole
// test, whose path the test can name and hand to a worker thread
// (`ConfigHandoff`). It is held for the whole test because the saves are.
use crate::test_helpers::isolated_config;

// ===========================================================================
// The two reference files
// ===========================================================================

/// The OLD and NEW files, as the real pipeline wrote them, plus what the loader
/// makes of each.
struct Reference {
    /// The bytes on the destination before the interrupted save.
    old: Vec<u8>,
    /// The bytes the interrupted save was trying to install - identical to what
    /// its temp file held at the instant of the rename.
    new: Vec<u8>,
    old_view: Loaded,
    new_view: Loaded,
}

/// Where the pipeline stages its write: `save_path.with_extension("stalx.tmp")`
/// (`project_io.rs:213`). Spelled the same way here on purpose, and pinned by
/// `the_engine_injects_at_the_temp_path_the_pipeline_uses` - an engine that
/// injected at a name the pipeline does not use would pass vacuously forever.
fn temp_path_of(save_path: &Path) -> PathBuf {
    save_path.with_extension("stalx.tmp")
}

/// SQLite's rollback journal for `db`: the database's own file name with
/// `-journal` appended. Built by hand rather than through `with_extension`,
/// which would have to spell `stalx-journal` and read like a typo.
fn journal_path_of(db: &Path) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push("-journal");
    db.with_file_name(name)
}

/// Build the pair of reference files by running the real save pipeline twice.
///
/// Returns `None` on any skip - a project the builder could not make saveable,
/// or a second save that produced no observable change (which would make the
/// whole property vacuous, since "old or new" is trivially satisfied when the
/// two are the same file).
fn build_reference(seed: u32, moves: usize, extra: usize) -> Option<(TempDir, Reference)> {
    let state = build_state(seed, false, moves)?;
    let stash = TempDir::new().ok()?;
    let live = stash.path().join("reference.stalx");
    if !state.save_project(Some(live.clone())).success {
        return None;
    }
    let old = std::fs::read(&live).ok()?;

    // A second batch of edits, then an IMPLICIT save - the pipeline overwriting
    // an existing project at its own path, which is the shape a crash actually
    // interrupts. The stream is offset from the builder's so the two are not
    // correlated draws of one sequence.
    let mut rng = Rng::new(seed.wrapping_add(0x9e37_79b9));
    for _ in 0..extra {
        mutate(&state, &mut rng);
    }
    // Leave the redo stack non-empty: both stacks are persisted, and a NEW file
    // whose history differs from the OLD one's only on the undo side would not
    // exercise half of what the save writes.
    state.undo();
    if !state.save_project(None).success {
        return None;
    }
    let new = std::fs::read(&live).ok()?;
    drop(state);

    if old == new {
        return None;
    }

    // Observe each reference through a COPY. Opening a project writes to it
    // (`project_load.rs` backfills resolved-warning UUIDs, and
    // `ensure_schema_version` can migrate), so observing the originals would
    // change the bytes this engine is about to inject.
    let old_view = observe(&open_fresh(&write_copy(
        stash.path(),
        "old-view.stalx",
        &old,
    )?)?);
    let new_view = observe(&open_fresh(&write_copy(
        stash.path(),
        "new-view.stalx",
        &new,
    )?)?);

    // The two references must be DISTINGUISHABLE to the loader, not merely
    // different on disk: a byte difference the loaded state cannot see (a
    // freelist page, say) would make "exactly old" and "exactly new" the same
    // assertion and every one of them would pass for the wrong reason.
    if load_differences(&old_view, &new_view).is_empty() {
        return None;
    }

    Some((
        stash,
        Reference {
            old,
            new,
            old_view,
            new_view,
        },
    ))
}

fn write_copy(dir: &Path, name: &str, bytes: &[u8]) -> Option<PathBuf> {
    let path = dir.join(name);
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

// ===========================================================================
// The injected states
// ===========================================================================

/// What the destination must present after the injected crash.
#[derive(Clone, Copy)]
enum Expect {
    /// Exactly the OLD file's logical content. Every pre-rename boundary: the
    /// destination was never written to.
    Old,
    /// Exactly the NEW file's. Every post-rename boundary.
    New,
}

impl Expect {
    const fn describe(self) -> &'static str {
        match self {
            Self::Old => "the OLD file (the save had not renamed yet)",
            Self::New => "the NEW file (the rename went through)",
        }
    }
}

/// One on-disk state a kill can leave, and the step it lands after.
struct Crash {
    id: String,
    /// The pipeline step the kill follows, with the line that performs it.
    after: &'static str,
    shape: Shape,
    expect: Expect,
}

/// The files the crash left behind, described rather than materialized, so a
/// sweep does not hold every seed's artifacts in memory at once.
enum Shape {
    /// The destination alone.
    Untouched,
    /// The destination, plus a temp holding the first `bytes` of the finished
    /// new file.
    TornTemp { bytes: usize },
    /// The destination, plus a temp caught mid-transaction with its hot
    /// rollback journal.
    TempMidTransaction,
    /// The destination, plus the complete temp, waiting for a rename that never
    /// came.
    StagedTemp,
    /// The new destination, alone.
    Renamed,
    /// The new destination, plus the `-journal` of a temp two saves ago that
    /// `:482`'s cleanup does not remove.
    RenamedWithOrphanedJournal,
    /// The destination caught mid-transaction with its own hot journal - a kill
    /// during a write through the handle reopened at `:704`.
    HotJournal,
}

/// Where to truncate the temp file, for a database of `len` bytes.
///
/// Not arbitrary: 0 is a temp the `Database::open` at `:487` created and nothing
/// was written to; 1 is a single byte of the magic string; 100 is exactly
/// SQLite's file header, so the header is complete and every page is missing;
/// 4096 is one default page; then the midpoint and one byte short of the whole
/// file, which is the last state before the temp is complete.
fn truncation_points(len: usize) -> Vec<usize> {
    let mut points = vec![0, 1, 100, 4096, len / 2, len.saturating_sub(1)];
    points.retain(|point| *point < len);
    points.sort_unstable();
    points.dedup();
    points
}

/// Every crash state for one reference pair.
fn crashes(reference: &Reference) -> Vec<Crash> {
    let mut out = vec![Crash {
        id: "destination-only".to_string(),
        after: "before the temp database is created (project_io.rs:487), or after any of the \
                failure cleanups removed it (:491, :499, :588, :661)",
        shape: Shape::Untouched,
        expect: Expect::Old,
    }];
    for bytes in truncation_points(reference.new.len()) {
        out.push(Crash {
            id: format!("temp-truncated-at-{bytes}"),
            after: "part-way through writing the staged file (project_io.rs:496-:608)",
            shape: Shape::TornTemp { bytes },
            expect: Expect::Old,
        });
    }
    out.push(Crash {
        id: "temp-mid-transaction-with-a-hot-journal".to_string(),
        after: "a write into the already-committed staged file (project_io.rs:509-:608)",
        shape: Shape::TempMidTransaction,
        expect: Expect::Old,
    });
    out.push(Crash {
        id: "temp-complete-but-not-renamed".to_string(),
        after: "the staged database was closed (project_io.rs:611) and before the rename (:660)",
        shape: Shape::StagedTemp,
        expect: Expect::Old,
    });
    out.push(Crash {
        id: "renamed".to_string(),
        after: "the rename (project_io.rs:660), before the reopen (:704) or the recents update \
                (:748)",
        shape: Shape::Renamed,
        expect: Expect::New,
    });
    out.push(Crash {
        id: "renamed-over-an-orphaned-temp-journal".to_string(),
        after: "a save that followed an interrupted one: :482 removed the leftover temp but not \
                its -journal, and :660 renamed the fresh temp over the destination",
        shape: Shape::RenamedWithOrphanedJournal,
        expect: Expect::New,
    });
    out.push(Crash {
        id: "destination-mid-transaction-with-a-hot-journal".to_string(),
        after: "a write through the handle the save reopened at project_io.rs:704",
        shape: Shape::HotJournal,
        expect: Expect::Old,
    });
    out
}

/// A database caught mid-transaction, with the hot rollback journal a kill at
/// that instant would leave beside it.
///
/// The `cache_size` pragma is what makes this faithful rather than decorative.
/// With SQLite's default cache a transaction this small never touches the main
/// file before COMMIT, so the captured "interrupted" bytes would be identical to
/// the input and the recovery would have nothing to roll back. Capped at one
/// page, the transaction SPILLS its dirty pages into the database file, which is
/// precisely the state a kill has to be recoverable from.
///
/// Returns `(database bytes, journal bytes)`, or `None` if the scratch write
/// could not be staged. The scratch transaction is rolled back afterwards; the
/// captured bytes keep the interrupted state.
fn mid_transaction(bytes: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let scratch = TempDir::new().ok()?;
    let path = scratch.path().join("interrupted.stalx");
    std::fs::write(&path, bytes).ok()?;
    let connection = Connection::open(&path).ok()?;
    connection.execute_batch("PRAGMA cache_size = 1;").ok()?;
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             UPDATE project SET name = 'interrupted-write';
             UPDATE shots SET length = length + 1000.0;
             UPDATE sections SET name = 'interrupted-write';",
        )
        .ok()?;
    let interrupted = std::fs::read(&path).ok()?;
    let journal = std::fs::read(journal_path_of(&path)).ok()?;
    let _ = connection.execute_batch("ROLLBACK;");
    drop(connection);
    Some((interrupted, journal))
}

/// Materialize one crash state in `dir` and return the destination path.
fn stage(dir: &Path, reference: &Reference, shape: &Shape) -> Option<PathBuf> {
    let destination = dir.join("cave.stalx");
    let temp = temp_path_of(&destination);
    match shape {
        Shape::Untouched => std::fs::write(&destination, &reference.old).ok()?,
        Shape::TornTemp { bytes } => {
            std::fs::write(&destination, &reference.old).ok()?;
            std::fs::write(&temp, &reference.new[..*bytes]).ok()?;
        }
        Shape::TempMidTransaction => {
            let (interrupted, journal) = mid_transaction(&reference.new)?;
            std::fs::write(&destination, &reference.old).ok()?;
            std::fs::write(&temp, interrupted).ok()?;
            std::fs::write(journal_path_of(&temp), journal).ok()?;
        }
        Shape::StagedTemp => {
            std::fs::write(&destination, &reference.old).ok()?;
            std::fs::write(&temp, &reference.new).ok()?;
        }
        Shape::Renamed => std::fs::write(&destination, &reference.new).ok()?,
        Shape::RenamedWithOrphanedJournal => {
            let (_, journal) = mid_transaction(&reference.old)?;
            std::fs::write(&destination, &reference.new).ok()?;
            std::fs::write(journal_path_of(&temp), journal).ok()?;
        }
        Shape::HotJournal => {
            let (interrupted, journal) = mid_transaction(&reference.old)?;
            std::fs::write(&destination, interrupted).ok()?;
            std::fs::write(journal_path_of(&destination), journal).ok()?;
        }
    }
    Some(destination)
}

// ===========================================================================
// The oracle
// ===========================================================================

/// What one seed's crash sweep found.
struct Outcome {
    /// Crash states actually built and checked.
    checked: usize,
    problems: Vec<String>,
    /// Measurements a reviewer needs to know the run meant something, keyed by a
    /// short id.
    notes: BTreeMap<&'static str, usize>,
}

impl Outcome {
    fn skipped() -> Self {
        Self {
            checked: 0,
            problems: Vec::new(),
            notes: BTreeMap::new(),
        }
    }
}

/// The panic message from a `catch_unwind` payload, first line only.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_string())
        })
        .map_or_else(
            || "<non-string panic>".to_string(),
            |message| message.lines().next().unwrap_or_default().to_string(),
        )
}

/// Copy the staged file - and its rollback journal, if the crash left one -
/// into a sub-directory, under the same names, so it can be opened without
/// disturbing the originals.
///
/// The sub-directory is not scanned by [`stray_temps`], which reads `dir` and
/// does not descend, so the copy cannot be mistaken for a leftover.
fn probe_copy_of(dir: &Path, temp: &Path) -> Option<PathBuf> {
    let probe = dir.join("probe");
    std::fs::create_dir_all(&probe).ok()?;
    let name = temp.file_name()?;
    let copy = probe.join(name);
    std::fs::copy(temp, &copy).ok()?;
    let journal = journal_path_of(temp);
    if journal.exists() {
        std::fs::copy(&journal, journal_path_of(&copy)).ok()?;
    }
    Some(copy)
}

/// Every `.stalx.tmp` left in `dir`.
fn stray_temps(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".stalx.tmp"))
        .collect();
    out.sort();
    out
}

/// Check one injected crash state.
fn inspect(seed: u32, dir: &Path, reference: &Reference, crash: &Crash) -> Outcome {
    let Some(destination) = stage(dir, reference, &crash.shape) else {
        return Outcome::skipped();
    };
    let temp = temp_path_of(&destination);
    let temp_was_staged = temp.exists();
    let mut problems = Vec::new();
    let mut notes = BTreeMap::new();

    let label = |what: &str| -> String {
        format!(
            "seed {seed} [{}]: {what}\n  the kill lands: {}",
            crash.id, crash.after
        )
    };

    // ORACLE 1: no panic.
    let opened =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| open_fresh(&destination)));
    let state = match opened {
        Err(payload) => {
            problems.push(format!(
                "{}\n  panic: {}",
                label("opening the destination PANICKED"),
                panic_message(payload.as_ref())
            ));
            return Outcome {
                checked: 1,
                problems,
                notes,
            };
        }
        // ORACLE 3: it opens. Every state here leaves the destination either
        // byte-intact or recoverable by SQLite's own journal replay, so a
        // refusal means a crash cost the user a file that was never damaged.
        Ok(None) => {
            problems.push(label(
                "the destination would NOT open, but no write ever reached it (or the only write \
                 that did is covered by an intact rollback journal), so the crash destroyed a file \
                 the pipeline promises it cannot touch",
            ));
            return Outcome {
                checked: 1,
                problems,
                notes,
            };
        }
        Ok(Some(state)) => state,
    };

    // ORACLE 2: exactly old, or exactly new - and this engine knows which.
    let view = observe(&state);
    let against_old = load_differences(&reference.old_view, &view);
    let against_new = load_differences(&reference.new_view, &view);
    let matches = match crash.expect {
        Expect::Old => against_old.is_empty(),
        Expect::New => against_new.is_empty(),
    };
    if !matches {
        let (differences, other) = match crash.expect {
            Expect::Old => (&against_old, &against_new),
            Expect::New => (&against_new, &against_old),
        };
        let verdict = if other.is_empty() {
            "it is the OTHER reference file, so the crash landed on the wrong side of the rename"
        } else {
            "it is NEITHER reference file - a HYBRID, which is the outcome the temp-file-plus-\
             rename scheme exists to make impossible"
        };
        problems.push(format!(
            "{}\n  {verdict}\n  it differs from what it must be in {} way(s):\n    {}\n{}",
            label(&format!(
                "the destination is not {}",
                crash.expect.describe()
            )),
            differences.len(),
            differences.join("\n    "),
            dump_against_references(&destination, reference),
        ));
    }

    // ORACLE 5: the leftover staging file. Nothing above went looking for it -
    // the loader takes an explicit path, so a temp cannot BE the project by
    // accident - which leaves what happens when something opens one on purpose.
    // Something does: the recovery UI lists `.tmp` files as restore candidates
    // (`commands/recovery.rs:37`, `persistence/sqlite/repair.rs:528`) and
    // `restore_from_backup` copies the chosen one OVER the project.
    //
    // For a temp the crash left COMPLETE (whole, or committed-then-interrupted
    // with a journal to roll back), that is an assertion: it must load as
    // exactly the new project, because it IS the new project. For a TORN temp it
    // is a measurement - a half-written database is a half-written database, and
    // both "refused" and "opens" are defensible answers to it.
    //
    // Probed on a COPY, in a directory of its own, and that is load-bearing
    // rather than tidy: opening a database with a hot journal ROLLS IT BACK and
    // deletes the journal, so probing the staged file in place would disarm the
    // orphaned-journal hazard the recovery save below is there to walk into.
    if let Some(temp) = temp_was_staged.then(|| probe_copy_of(dir, &temp)).flatten() {
        let torn = matches!(crash.shape, Shape::TornTemp { .. });
        let probed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            open_fresh(&temp).map(|state| observe(&state))
        }));
        match probed {
            Err(payload) => problems.push(format!(
                "{}\n  panic: {}",
                label("opening the leftover STAGING FILE directly PANICKED"),
                panic_message(payload.as_ref())
            )),
            Ok(None) => {
                if torn {
                    *notes.entry("torn-temp-refused").or_insert(0) += 1;
                } else {
                    problems.push(label(
                        "the leftover staging file is a COMPLETE database holding the save the \
                         crash interrupted, and it would not open - so the one artifact that still \
                         carries the user's last edit is unreadable",
                    ));
                }
            }
            Ok(Some(view)) => {
                let differences = load_differences(&reference.new_view, &view);
                if torn {
                    *notes
                        .entry(if differences.is_empty() {
                            "torn-temp-opens-as-the-new-project"
                        } else {
                            "torn-temp-opens-with-content-of-its-own"
                        })
                        .or_insert(0) += 1;
                } else if !differences.is_empty() {
                    problems.push(format!(
                        "{}\n  it differs from the new project in {} way(s):\n    {}",
                        label(
                            "the leftover staging file opened, but it is NOT the save the crash \
                             interrupted - so restoring it would install something the app never \
                             wrote"
                        ),
                        differences.len(),
                        differences.join("\n    ")
                    ));
                }
            }
        }
    }

    // The recovery save below is only a walk through the orphaned-journal hazard
    // if the journal is still on disk when it runs. Anything that consumed it
    // earlier - a probe done in place, a helper that opened the staged file -
    // would retire that state silently, so check rather than assume.
    let staged_journal = journal_path_of(&temp);
    let armed = staged_journal.exists();
    if matches!(
        crash.shape,
        Shape::TempMidTransaction | Shape::RenamedWithOrphanedJournal
    ) && !armed
    {
        problems.push(label(
            "the staged file's rollback journal was consumed before the recovery save ran, so this \
             state no longer walks the next save onto an orphaned hot journal - the one thing it \
             exists to do",
        ));
    }

    // ORACLE 4: recovery forward. Saving again must work, and must clear the
    // leftover temp `:482` promises to remove.
    let saved = state.save_project(None);
    if saved.success {
        let strays = stray_temps(dir);
        if !strays.is_empty() {
            problems.push(label(&format!(
                "the recovery save succeeded but left a staging file behind: {strays:?} \
                 (project_io.rs:482 removes a leftover temp before starting, and :660 renames its \
                 own away)"
            )));
        }
        if open_fresh(&destination).is_none() {
            problems.push(label(
                "the file the recovery save wrote would not open, so the crash state poisoned the \
                 NEXT save rather than the current one",
            ));
        }
        // Measured, not asserted: what became of the orphaned journal. Nothing
        // promises it is cleaned up - `:482` removes the temp and says nothing
        // about its journal - so a survivor would be a leak rather than a broken
        // promise. Both outcomes are counted, so the summary distinguishes
        // "measured, and it is cleaned up" from "never looked".
        if armed {
            *notes
                .entry(if staged_journal.exists() {
                    "orphaned-journal-survives-the-next-save"
                } else {
                    "orphaned-journal-cleared-by-the-next-save"
                })
                .or_insert(0) += 1;
        }
    } else {
        problems.push(format!(
            "{}\n  error: {}",
            label(
                "the destination opened after the crash but could NOT be saved again, so the \
                 recovery is one-way"
            ),
            saved.error.unwrap_or_default()
        ));
    }
    drop(state);

    Outcome {
        checked: 1,
        problems,
        notes,
    }
}

/// A failure payload that says HOW the destination differs from each reference,
/// in the file-format engine's own table-and-row rendering.
///
/// Dumped only on failure, and deliberately only then: reading a `.stalx` opens
/// it read-write, which rolls back a hot journal - so dumping every destination
/// up front would recover the very states this engine injects before the app
/// ever saw them.
fn dump_against_references(destination: &Path, reference: &Reference) -> String {
    let scratch = || -> Option<(String, String, String)> {
        let dir = TempDir::new().ok()?;
        let here = dump_database(destination).ok()?;
        let old = dump_database(&write_copy(dir.path(), "old.stalx", &reference.old)?).ok()?;
        let new = dump_database(&write_copy(dir.path(), "new.stalx", &reference.new)?).ok()?;
        Some((here, old, new))
    };
    let Some((here, old, new)) = scratch() else {
        return "  (the destination could not be dumped for comparison)".to_string();
    };
    format!(
        "  against the OLD file:\n{}\n  against the NEW file:\n{}",
        dump_differences(&old, &here, ("old file", "on disk")).join("\n"),
        dump_differences(&new, &here, ("new file", "on disk")).join("\n"),
    )
}

/// One seed: build the reference pair, then sweep every crash state.
fn check_crash_states(seed: u32, moves: usize, extra: usize) -> Outcome {
    let Some((_stash, reference)) = build_reference(seed, moves, extra) else {
        return Outcome::skipped();
    };
    let Ok(arena) = TempDir::new() else {
        return Outcome::skipped();
    };
    let mut outcome = Outcome::skipped();
    for crash in crashes(&reference) {
        // A directory per state, so no leftover of one injection can reach the
        // next - the whole point being to control exactly what is on disk.
        let dir = arena.path().join(&crash.id);
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let found = inspect(seed, &dir, &reference, &crash);
        outcome.checked += found.checked;
        outcome.problems.extend(found.problems);
        for (id, count) in found.notes {
            *outcome.notes.entry(id).or_insert(0) += count;
        }
    }
    outcome
}

// ===========================================================================
// The gate
// ===========================================================================

/// PINNED. Clean today; `crash_property_sweep` re-derives the set.
const GATE_SEEDS: &[u32] = &[1, 2, 3];
/// Commands before the first save, and again before the second.
const GATE_MOVES: usize = 12;
const GATE_EXTRA: usize = 6;
/// The floor on states per seed. Six are fixed (untouched, the mid-transaction
/// temp, the complete temp, the two post-rename states and the mid-transaction
/// destination) and the truncation sweep adds up to six more, so a healthy seed
/// builds twelve. The floor is eleven rather than twelve so that a reference
/// file small enough to collapse two truncation points is a smaller sweep rather
/// than a red gate - anything below it means the reference pair is not being
/// built, which is the vacuous pass this guards against.
const STATES_PER_SEED: usize = 11;

/// THE CRASH-TIMING GATE. Not `#[ignore]`d.
#[test]
fn crash_properties_gate() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    let mut checked = 0;
    let mut notes: BTreeMap<&'static str, usize> = BTreeMap::new();
    for &seed in GATE_SEEDS {
        let outcome = check_crash_states(seed, GATE_MOVES, GATE_EXTRA);
        checked += outcome.checked;
        problems.extend(outcome.problems);
        for (id, count) in outcome.notes {
            *notes.entry(id).or_insert(0) += count;
        }
    }
    println!(
        "crash properties: {checked} injected crash state(s) over {} seed(s); measured: {notes:?}",
        GATE_SEEDS.len()
    );
    // A run that injected nothing passes vacuously - the failure mode a
    // skip-on-anything-unexpected design invites.
    assert!(
        checked >= GATE_SEEDS.len() * STATES_PER_SEED,
        "the crash gate injected only {checked} state(s), fewer than the {} it must; the reference \
         pair is not being built",
        GATE_SEEDS.len() * STATES_PER_SEED
    );
    assert!(
        problems.is_empty(),
        "{} crash-timing property failure(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// The engine injects at the path the PIPELINE stages into, not at one that
/// merely looks like it.
///
/// Squats a directory at `<stem>.stalx.tmp` and asserts the save fails: a
/// `Database::open` on a path held by a directory cannot succeed, so a save that
/// still succeeds is a save staging somewhere else - and every injected state in
/// this module would be placing files the pipeline never looks at, passing
/// forever while testing nothing.
#[test]
fn the_engine_injects_at_the_temp_path_the_pipeline_uses() {
    let _config = isolated_config();
    let Some(state) = build_state(1, false, 6) else {
        panic!("the builder produced no project");
    };
    let temp = TempDir::new().expect("no temp dir");
    let destination = temp.path().join("cave.stalx");
    std::fs::create_dir_all(temp_path_of(&destination)).expect("could not squat the temp path");

    let blocked = state.save_project(Some(destination.clone()));
    assert!(
        !blocked.success,
        "a save with a DIRECTORY squatting {} succeeded, so the pipeline no longer stages at the \
         path this engine injects into",
        temp_path_of(&destination).display()
    );

    // ...and with the squatter gone the same save works and leaves no temp
    // behind, which is the other half of the contract the oracle leans on.
    std::fs::remove_dir(temp_path_of(&destination)).expect("could not clear the squatter");
    assert!(
        state.save_project(Some(destination.clone())).success,
        "the save failed for a reason other than the squatted temp path"
    );
    assert!(
        stray_temps(temp.path()).is_empty(),
        "a successful save left a staging file behind: {:?}",
        stray_temps(temp.path())
    );
}

/// The hot-journal injection is a REAL interrupted write, not a decoration.
///
/// Two things have to be true of it, and neither is obvious from the code that
/// produces it. The journal has to exist - if SQLite ever defaults `.stalx` to
/// WAL, `<file>-journal` stops being the artifact and the injection silently
/// stages a file nobody reads. And the database bytes have to have MOVED: with
/// SQLite's ordinary cache a transaction this small commits straight out of
/// memory, the captured "interrupted" file is identical to its input, and the
/// recovery the oracle is checking has nothing to roll back. The `cache_size`
/// cap in `mid_transaction` is what forces the spill; this is what proves it
/// still does.
#[test]
fn the_hot_journal_injection_is_a_real_interrupted_write() {
    let _config = isolated_config();
    let Some(state) = build_state(1, false, 10) else {
        panic!("the builder produced no project");
    };
    let temp = TempDir::new().expect("no temp dir");
    let path = temp.path().join("cave.stalx");
    assert!(
        state.save_project(Some(path.clone())).success,
        "could not write the file to interrupt"
    );
    let committed = std::fs::read(&path).expect("could not read the saved file");
    drop(state);

    let (interrupted, journal) =
        mid_transaction(&committed).expect("the interrupted write could not be staged");
    assert!(
        !journal.is_empty(),
        "the interrupted transaction left an EMPTY rollback journal, so the injected hot-journal \
         states carry nothing for SQLite to recover from"
    );
    assert_ne!(
        interrupted, committed,
        "the interrupted transaction never reached the database file, so the injected hot-journal \
         states are byte-identical to the committed file and the recovery oracle is vacuous"
    );
}

/// Determinism, so a failing state is a repro.
///
/// `checked` and the problem count - the two things the ENGINE decides - are
/// compared exactly, and so is the TOTAL number of leftover-temp probes, which
/// is structural (one per state that stages a temp). The per-bucket split of
/// those probes is deliberately NOT compared, and the reason is the app's, not
/// this module's: two saves of one project can lay the same rows out in a
/// different PHYSICAL order, and a truncation at a fixed byte offset then cuts a
/// different row out of the file. Measured directly - the same 60-seed sweep run
/// twice moved one torn temp between "opens as the new project" and "opens with
/// content of its own" while every total stayed put. Comparing the split would
/// make this test fail for the writer's row order rather than for a regression
/// here, which is the only thing it is about.
///
/// This is NOT finding 21, which earlier revisions of this comment named. Finding
/// 21 was the least-squares solve wandering between runs, and it is fixed
/// (`adjust_least_squares` numbers from a sorted sequence). What is left is where
/// a row physically LANDS in the file, which no oracle in this programme asserts
/// and which the totals above are deliberately stated in spite of.
#[test]
fn the_crash_properties_are_deterministic() {
    let _config = isolated_config();
    let probes = |outcome: &Outcome| -> usize { outcome.notes.values().sum() };
    for &seed in &GATE_SEEDS[..2] {
        let first = check_crash_states(seed, GATE_MOVES, GATE_EXTRA);
        let second = check_crash_states(seed, GATE_MOVES, GATE_EXTRA);
        assert_eq!(
            (first.checked, first.problems.len(), probes(&first)),
            (second.checked, second.problems.len(), probes(&second)),
            "seed {seed} did not replay identically"
        );
    }
}

/// The SWEEP, and the tool for re-choosing `GATE_SEEDS`.
///
/// ```text
/// STALAGMIGHT_RUST_CRASH_SEEDS=40 STALAGMIGHT_RUST_CRASH_MOVES=30 \
///   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
///   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
///   --lib crash_property_sweep -- --ignored --nocapture
/// ```
#[test]
#[ignore = "sweep: prints per-seed status, used to choose the crash-property GATE_SEEDS"]
fn crash_property_sweep() {
    let _config = isolated_config();
    let seeds: u32 = std::env::var("STALAGMIGHT_RUST_CRASH_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20);
    let moves: usize = std::env::var("STALAGMIGHT_RUST_CRASH_MOVES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(GATE_MOVES);
    let extra: usize = std::env::var("STALAGMIGHT_RUST_CRASH_EXTRA")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(GATE_EXTRA);

    let mut dirty = Vec::new();
    let mut checked = 0;
    let mut skipped = 0;
    let mut notes: BTreeMap<&'static str, usize> = BTreeMap::new();
    for seed in sweep_seeds("STALAGMIGHT_RUST_CRASH_FIRST_SEED", seeds) {
        let outcome = check_crash_states(seed, moves, extra);
        checked += outcome.checked;
        if outcome.checked == 0 {
            skipped += 1;
        }
        for (id, count) in outcome.notes {
            *notes.entry(id).or_insert(0) += count;
        }
        if !outcome.problems.is_empty() {
            dirty.push(seed);
            for problem in &outcome.problems {
                println!("{problem}");
            }
        }
    }
    println!(
        "crash sweep: {seeds} seed(s), {checked} injected state(s), {skipped} seed(s) skipped; \
         measured: {notes:?}; seeds with a failure ({}): {dirty:?}",
        dirty.len()
    );
}
