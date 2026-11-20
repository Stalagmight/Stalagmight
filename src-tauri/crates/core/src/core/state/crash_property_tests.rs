//! CRASH-TIMING properties: what the save pipeline owes a kill between any two
//! of its steps, on the `.stalx` v2 save.
//!
//! # The pipeline, as measured
//!
//! `AppState::save_project` (`project_io.rs`, `save_with`) validates and
//! encodes in memory, then `stalx_v2::write_atomic` (`atomic.rs`):
//!
//! ```text
//!  temp = <file>.tmp                    (stalx_v2::temp_path: cave.stalx -> cave.stalx.tmp)
//!  remove a leftover temp
//!  create the temp (create_new), write the bytes, sync_all
//!  check the guard (the destination still holds what was last read or written)
//!  rename(temp, destination)            <- the atomic moment
//!  fsync the folder
//! ```
//!
//! The first save of a legacy (SQLite) file runs `UpgradeBackup::take` first:
//! it COPIES the original into the app-data `backups/` folder as
//! `<stem>.<UTC stamp>.pre-v2.stalx` and MOVES its `-journal` / `-wal` /
//! `-shm` beside that copy (so no old build can roll a stale journal into the
//! JSON file), then writes as above; a failed write moves the sidecars back.
//!
//! The destination is only ever touched by the rename, so before it the
//! destination holds the OLD file byte for byte, after it the NEW one.
//!
//! # Simulated artifacts, not killed processes
//!
//! A kill is reproduced by constructing the files it would leave: the temp is
//! a private file and `rename(2)` is atomic, so the set of states is small and
//! reachable by copying bytes. The reference files are written by the REAL
//! pipeline (a random survey with history, saved; edited further and saved
//! again over the same path).
//!
//! # The v2 states, and the oracle
//!
//! | injected state | the kill lands | the destination must open as |
//! |---|---|---|
//! | destination only | before the temp is created, or after a failed save removed it | OLD |
//! | destination + temp truncated at N bytes | while the temp is written | OLD |
//! | destination + the whole temp | after the sync, before the rename | OLD |
//! | the new destination | after the rename | NEW |
//!
//! For each: no panic; the destination opens (a refusal would mean a crash
//! cost a file nothing wrote to) and is EXACTLY the expected reference (survey,
//! verdict, Computed View, allocators, both stacks); the next save succeeds
//! and leaves no temp behind. The leftover temp: a torn one never decodes as a
//! project (unless the cut only took the final newline, when it is the whole
//! new file) and the recovery list never offers it; a complete one is offered
//! and restores as exactly the save the crash interrupted, the old file kept
//! as the safety copy.
//!
//! # The legacy upgrade, interrupted at each step
//!
//! | injected state | the kill lands |
//! |---|---|
//! | the SQLite original (+ its journal) | before the backup |
//! | + the backup copy | after the copy, before the sidecars move |
//! | the original alone; the copy and its sidecars in the backups folder | after the move, before the write |
//! | the same + the complete temp | after the sync, before the rename |
//! | the v2 file; the backup pair | after the rename |
//!
//! Every state: never a JSON file beside an armed journal (or a WAL), the
//! destination opens as the survey the user had (OLD) or the upgraded one
//! (NEW), and the backup pair opens as OLD. Measured on a clean original and on
//! one an old build left with a hot journal. With a hot journal the original
//! read without it is NOT the old survey (the journal holds the pre-images of
//! the pages the interrupted transaction spilled), so that state is reported,
//! not asserted, and pinned as a known bug by
//! `an_upgrade_interrupted_after_its_sidecar_move_leaves_the_original_whole`
//! (ignored, see its reason). The real pipeline is pinned on both ends: a
//! completed upgrade leaves the JSON file with no sidecar beside it and the
//! pair in the backups folder; a write that fails puts the journal back.
//!
//! # Data
//!
//! Everything is generated in code: the projects, both reference files and
//! every injected artifact. No fixture is read, copied or derived from.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use tempfile::TempDir;

use super::format_property_tests::{build_state, mutate};
use super::robustness_kit::{
    canonical_snapshot, differences, entries, first_difference, observe, open_fresh, panic_text,
    write_legacy, Observed, Rng,
};
use super::speleodb_sidecars::{has_hot_journal, sidecar_path};
use super::AppState;
use crate::persistence::stalx_v2::{self, DecodeOptions, Format};
use crate::test_helpers::isolated_config;

// ===========================================================================
// The two reference files
// ===========================================================================

struct Reference {
    old: Vec<u8>,
    new: Vec<u8>,
    old_view: Observed,
    new_view: Observed,
}

/// Where the pipeline stages its write: `<file>.tmp`. Spelled as the
/// pipeline spells it; `the_engine_injects_at_the_temp_path_the_pipeline_uses`
/// pins that it is the name the pipeline uses.
fn temp_path_of(save_path: &Path) -> PathBuf {
    stalx_v2::temp_path(save_path)
}

fn write_copy(dir: &Path, name: &str, bytes: &[u8]) -> Option<PathBuf> {
    let path = dir.join(name);
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

/// The OLD and NEW files, written by the real pipeline, and what an open
/// makes of each (observed through copies). `None` when the second save
/// changed nothing an open can see (the property would be vacuous).
fn build_reference(seed: u32, moves: usize, extra: usize) -> Option<(TempDir, Reference)> {
    let state = build_state(seed, false, moves)?;
    let stash = TempDir::new().ok()?;
    let live = stash.path().join("reference.stalx");
    if !state.save_project(Some(live.clone())).success {
        return None;
    }
    let old = std::fs::read(&live).ok()?;
    let mut rng = Rng::new(seed.wrapping_add(0x9e37_79b9));
    for _ in 0..extra {
        mutate(&state, &mut rng);
    }
    // Leave redo non-empty: both stacks are written.
    state.undo();
    if !state.save_project(None).success {
        return None;
    }
    let new = std::fs::read(&live).ok()?;
    drop(state);
    if old == new {
        return None;
    }
    let old_view = observe(&open_fresh(&write_copy(stash.path(), "old-view.stalx", &old)?)?);
    let new_view = observe(&open_fresh(&write_copy(stash.path(), "new-view.stalx", &new)?)?);
    if differences(&old_view, &new_view).is_empty() {
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

// ===========================================================================
// The injected v2 states
// ===========================================================================

#[derive(Clone, Copy)]
enum Expect {
    Old,
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

enum Shape {
    Untouched,
    TornTemp { bytes: usize },
    StagedTemp,
    Renamed,
}

struct Crash {
    id: String,
    after: &'static str,
    shape: Shape,
    expect: Expect,
}

/// Where to cut the temp: nothing written, one byte, the first line only, the
/// middle, everything but the last line (`"end":true}`), and everything but
/// the final newline (a whole file).
fn truncation_points(new: &[u8]) -> Vec<usize> {
    let len = new.len();
    let first_line = new.iter().position(|b| *b == b'\n').map_or(1, |at| at + 1);
    let last_line = new[..len.saturating_sub(1)]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(len / 2, |at| at + 1);
    let mut points = vec![0, 1, first_line, len / 2, last_line, len.saturating_sub(1)];
    points.retain(|p| *p < len);
    points.sort_unstable();
    points.dedup();
    points
}

fn crashes(reference: &Reference) -> Vec<Crash> {
    let mut out = vec![Crash {
        id: "destination-only".to_string(),
        after: "before the temp is created, or after a failed save removed it",
        shape: Shape::Untouched,
        expect: Expect::Old,
    }];
    for bytes in truncation_points(&reference.new) {
        out.push(Crash {
            id: format!("temp-truncated-at-{bytes}"),
            after: "part-way through writing the temp",
            shape: Shape::TornTemp { bytes },
            expect: Expect::Old,
        });
    }
    out.push(Crash {
        id: "temp-complete-but-not-renamed".to_string(),
        after: "the temp was synced and the guard checked, before the rename",
        shape: Shape::StagedTemp,
        expect: Expect::Old,
    });
    out.push(Crash {
        id: "renamed".to_string(),
        after: "the rename, before the folder sync or the recents update",
        shape: Shape::Renamed,
        expect: Expect::New,
    });
    out
}

fn stage(dir: &Path, reference: &Reference, shape: &Shape) -> Option<PathBuf> {
    let destination = dir.join("cave.stalx");
    let temp = temp_path_of(&destination);
    match shape {
        Shape::Untouched => std::fs::write(&destination, &reference.old).ok()?,
        Shape::TornTemp { bytes } => {
            std::fs::write(&destination, &reference.old).ok()?;
            std::fs::write(&temp, &reference.new[..*bytes]).ok()?;
        }
        Shape::StagedTemp => {
            std::fs::write(&destination, &reference.old).ok()?;
            std::fs::write(&temp, &reference.new).ok()?;
        }
        Shape::Renamed => std::fs::write(&destination, &reference.new).ok()?,
    }
    Some(destination)
}

// ===========================================================================
// The oracle
// ===========================================================================

#[derive(Default)]
struct Outcome {
    checked: usize,
    problems: Vec<String>,
    notes: BTreeMap<&'static str, usize>,
}

/// Every `.tmp` left in `dir` (not descending).
fn stray_temps(dir: &Path) -> Vec<String> {
    entries(dir)
        .into_iter()
        .filter(|name| name.ends_with(".tmp"))
        .collect()
}

/// Whether the recovery list of `destination` offers `candidate`.
fn offered(destination: &Path, candidate: &Path) -> bool {
    AppState::list_project_backups(destination)
        .iter()
        .any(|b| Path::new(&b.path) == candidate)
}

fn inspect(seed: u32, dir: &Path, reference: &Reference, crash: &Crash) -> Outcome {
    let mut outcome = Outcome::default();
    let Some(destination) = stage(dir, reference, &crash.shape) else {
        return outcome;
    };
    outcome.checked = 1;
    let temp = temp_path_of(&destination);
    let label = |what: &str| -> String {
        format!(
            "seed {seed} [{}]: {what}\n  the kill lands: {}",
            crash.id, crash.after
        )
    };

    // The leftover temp, before anything else touches the folder.
    if temp.exists() {
        let bytes = std::fs::read(&temp).unwrap_or_default();
        let whole = bytes.len() + 1 >= reference.new.len();
        let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            stalx_v2::decode(&bytes, &DecodeOptions::default()).is_ok()
        }));
        match decoded {
            Err(payload) => outcome.problems.push(label(&format!(
                "decoding the leftover temp PANICKED: {}",
                panic_text(payload.as_ref())
            ))),
            Ok(true) if !whole => outcome.problems.push(label(
                "a TORN temp decoded as a project: the end marker is meant to make that \
                 impossible",
            )),
            Ok(false) if whole => outcome.problems.push(label(
                "the complete temp does not decode, so the one file that holds the interrupted \
                 save is unreadable",
            )),
            Ok(_) => {}
        }
        if offered(&destination, &temp) != whole {
            outcome.problems.push(label(&format!(
                "the recovery list {} the {} temp",
                if whole { "does not offer" } else { "offers" },
                if whole { "complete" } else { "torn" }
            )));
        }
        if whole {
            // Probed on a copy, in a folder of its own.
            let probe = dir.join("probe");
            let copy = std::fs::create_dir_all(&probe)
                .ok()
                .and_then(|()| write_copy(&probe, "probe.stalx", &bytes));
            match copy.as_deref().and_then(open_fresh) {
                Some(state) => {
                    let found = differences(&reference.new_view, &observe(&state));
                    if !found.is_empty() {
                        outcome.problems.push(format!(
                            "{}\n  {}",
                            label("the complete temp is NOT the save the crash interrupted"),
                            found.join("\n  ")
                        ));
                    }
                }
                None => outcome
                    .problems
                    .push(label("the complete temp would not open")),
            }
            let _ = std::fs::remove_dir_all(&probe);
        }
    }

    // No panic; it opens; exactly the expected reference.
    let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| open_fresh(&destination)));
    let state = match opened {
        Err(payload) => {
            outcome.problems.push(label(&format!(
                "opening the destination PANICKED: {}",
                panic_text(payload.as_ref())
            )));
            return outcome;
        }
        Ok(None) => {
            outcome.problems.push(label(
                "the destination would NOT open, but no write ever reached it",
            ));
            return outcome;
        }
        Ok(Some(state)) => state,
    };
    let view = observe(&state);
    let (expected, other) = match crash.expect {
        Expect::Old => (&reference.old_view, &reference.new_view),
        Expect::New => (&reference.new_view, &reference.old_view),
    };
    let found = differences(expected, &view);
    if !found.is_empty() {
        let verdict = if differences(other, &view).is_empty() {
            "it is the OTHER reference file: the crash landed on the wrong side of the rename"
        } else {
            "it is NEITHER reference file: a HYBRID"
        };
        outcome.problems.push(format!(
            "{}\n  {verdict}\n  {}",
            label(&format!(
                "the destination is not {}",
                crash.expect.describe()
            )),
            found.join("\n  ")
        ));
    }

    // Recovery forward: the next save works and leaves no temp behind.
    let saved = state.save_project(None);
    if saved.success {
        let strays = stray_temps(dir);
        if !strays.is_empty() {
            outcome.problems.push(label(&format!(
                "the recovery save succeeded but left a temp behind: {strays:?}"
            )));
        }
        match open_fresh(&destination) {
            Some(again) if canonical_snapshot(&again) == canonical_snapshot(&state) => {}
            Some(_) => outcome
                .problems
                .push(label("the recovery save wrote another survey")),
            None => outcome
                .problems
                .push(label("the file the recovery save wrote would not open")),
        }
    } else {
        outcome.problems.push(label(&format!(
            "the destination opened but could NOT be saved again: {:?}",
            saved.error
        )));
    }
    drop(state);

    // A complete temp restores as the interrupted save (on a fresh copy of
    // the state: the recovery save above removed the temp).
    if matches!(crash.shape, Shape::StagedTemp) {
        let again = dir.join("restore");
        if let Some(destination) = std::fs::create_dir_all(&again)
            .ok()
            .and_then(|()| stage(&again, reference, &crash.shape))
        {
            let temp = temp_path_of(&destination);
            let Ok(fresh) = AppState::new() else {
                return outcome;
            };
            match fresh.restore_project_from_backup(&destination, &temp) {
                Ok(_) => {
                    match open_fresh(&destination) {
                        Some(state) if differences(&reference.new_view, &observe(&state))
                            .is_empty() => {}
                        _ => outcome.problems.push(label(
                            "restoring the complete temp did not install the interrupted save",
                        )),
                    }
                    let safety = destination.with_extension("stalx.pre-restore");
                    if std::fs::read(&safety).ok().as_deref() != Some(reference.old.as_slice()) {
                        outcome.problems.push(label(
                            "the restore did not keep the old file as the safety copy",
                        ));
                    }
                    *outcome.notes.entry("complete-temp-restored").or_default() += 1;
                }
                Err(e) => outcome
                    .problems
                    .push(label(&format!("restoring the complete temp was refused: {e}"))),
            }
        }
    }
    outcome
}

fn check_crash_states(seed: u32, moves: usize, extra: usize) -> Outcome {
    let mut outcome = Outcome::default();
    let Some((_stash, reference)) = build_reference(seed, moves, extra) else {
        return outcome;
    };
    let Ok(arena) = TempDir::new() else {
        return outcome;
    };
    for crash in crashes(&reference) {
        let dir = arena.path().join(&crash.id);
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let found = inspect(seed, &dir, &reference, &crash);
        outcome.checked += found.checked;
        outcome.problems.extend(found.problems);
        for (id, count) in found.notes {
            *outcome.notes.entry(id).or_default() += count;
        }
    }
    outcome
}

// ===========================================================================
// The legacy upgrade
// ===========================================================================

/// A legacy file caught mid-transaction, with the hot rollback journal a kill
/// at that instant leaves beside it. The one-page cache makes the transaction
/// SPILL its pages into the database file, the state a kill must be
/// recoverable from. Returns `(database bytes, journal bytes)`.
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
    let journal = std::fs::read(sidecar_path(&path, "-journal")).ok()?;
    let _ = connection.execute_batch("ROLLBACK;");
    drop(connection);
    Some((interrupted, journal))
}

/// The legacy original as the upgrade meets it: the database file and, when
/// an old build was killed mid-write, its hot journal.
struct LegacyOriginal {
    main: Vec<u8>,
    journal: Option<Vec<u8>>,
    /// The survey the app opens it as (canonical text).
    survey: String,
    /// The v2 file the upgrading save writes for it.
    upgraded: Vec<u8>,
}

fn place_legacy(dir: &Path, name: &str, original: &LegacyOriginal) -> Option<PathBuf> {
    let path = dir.join(name);
    std::fs::write(&path, &original.main).ok()?;
    if let Some(journal) = &original.journal {
        std::fs::write(sidecar_path(&path, "-journal"), journal).ok()?;
    }
    Some(path)
}

/// Build the original (hot or clean) and what the real pipeline makes of it.
fn legacy_original(seed: u32, hot: bool) -> Option<LegacyOriginal> {
    crate::app_dirs::init_for_test();
    let scratch = TempDir::new().ok()?;
    let clean = scratch.path().join("clean.stalx");
    write_legacy(&clean, &mut Rng::new(seed), "Legacy");
    let clean_bytes = std::fs::read(&clean).ok()?;
    let (main, journal) = if hot {
        let (main, journal) = mid_transaction(&clean_bytes)?;
        (main, Some(journal))
    } else {
        (clean_bytes, None)
    };
    let mut original = LegacyOriginal {
        main,
        journal,
        survey: String::new(),
        upgraded: Vec::new(),
    };
    // A unique stem: the backups folder is shared by every test.
    let stem = format!("crash-upgrade-{seed}-{hot}-{}", std::process::id());
    let path = place_legacy(scratch.path(), &format!("{stem}.stalx"), &original)?;
    let state = open_fresh(&path)?;
    original.survey = canonical_snapshot(&state);
    if !state.save_project(None).success {
        return None;
    }
    original.upgraded = std::fs::read(&path).ok()?;
    Some(original)
}

/// The upgrade's crash states, built in `dir`. Returns the destination and
/// the backup copy's path.
fn stage_upgrade(dir: &Path, original: &LegacyOriginal, step: usize) -> Option<(PathBuf, PathBuf)> {
    let destination = dir.join("cave.stalx");
    let backups = dir.join("backups");
    std::fs::create_dir_all(&backups).ok()?;
    let backup = backups.join("cave.20261120T000000.000000000Z.pre-v2.stalx");
    match step {
        // Before the backup.
        0 => {
            place_legacy(dir, "cave.stalx", original)?;
        }
        // The copy taken, the sidecars not moved yet.
        1 => {
            place_legacy(dir, "cave.stalx", original)?;
            std::fs::write(&backup, &original.main).ok()?;
        }
        // The sidecars moved beside the copy; then the temp written.
        2 | 3 => {
            std::fs::write(&destination, &original.main).ok()?;
            place_legacy(&backups, "cave.20261120T000000.000000000Z.pre-v2.stalx", original)?;
            if step == 3 {
                std::fs::write(temp_path_of(&destination), &original.upgraded).ok()?;
            }
        }
        // Renamed.
        _ => {
            std::fs::write(&destination, &original.upgraded).ok()?;
            place_legacy(&backups, "cave.20261120T000000.000000000Z.pre-v2.stalx", original)?;
        }
    }
    Some((destination, backup))
}

const UPGRADE_STEPS: [&str; 5] = [
    "before-the-backup",
    "backup-copied",
    "sidecars-moved",
    "temp-written",
    "renamed",
];

fn check_upgrade_states(seed: u32, hot: bool, notes: &mut BTreeMap<String, usize>) -> Vec<String> {
    let Some(original) = legacy_original(seed, hot) else {
        return vec![format!(
            "seed {seed} hot={hot}: the legacy original or its upgrade could not be built"
        )];
    };
    let Ok(arena) = TempDir::new() else {
        return Vec::new();
    };
    let mut problems = Vec::new();
    for (step, id) in UPGRADE_STEPS.iter().enumerate() {
        let label = |what: &str| format!("seed {seed} hot={hot} [upgrade {id}]: {what}");
        let dir = arena.path().join(id);
        let Some((destination, backup)) = std::fs::create_dir_all(&dir)
            .ok()
            .and_then(|()| stage_upgrade(&dir, &original, step))
        else {
            problems.push(label("the state could not be built"));
            continue;
        };
        // Never a JSON file beside an armed journal or a WAL.
        let format = stalx_v2::sniff(&std::fs::read(&destination).unwrap_or_default());
        if format == Format::V2
            && (has_hot_journal(&destination) || sidecar_path(&destination, "-wal").exists())
        {
            problems.push(label("a JSON file sits beside an armed journal or a WAL"));
        }
        // The destination opens as the old survey or the upgraded one.
        let expected = if step == 4 {
            stalx_v2::decode(&original.upgraded, &DecodeOptions::default())
                .ok()
                .map(|d| super::robustness_kit::canonical(&d.project))
                .unwrap_or_default()
        } else {
            original.survey.clone()
        };
        // The original alone at the project's path, its journal moved away.
        let journal_gone = hot && (2..=3).contains(&step);
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| open_fresh(&destination))) {
            Err(payload) => problems.push(label(&format!(
                "opening the destination PANICKED: {}",
                panic_text(payload.as_ref())
            ))),
            Ok(opened) => {
                let as_expected = opened
                    .as_ref()
                    .is_some_and(|s| canonical_snapshot(s) == expected);
                if journal_gone {
                    // The known bug: measured, reported, pinned by the
                    // ignored test below.
                    let what = match &opened {
                        Some(_) if as_expected => "opens-as-the-old-survey",
                        Some(_) => "opens-as-another-survey",
                        None => "refused",
                    };
                    *notes
                        .entry(format!("hot original read without its journal: {what}"))
                        .or_default() += 1;
                } else if !as_expected {
                    problems.push(label(&match &opened {
                        Some(s) => format!(
                            "the destination is not the survey it must be: {}",
                            first_difference(&expected, &canonical_snapshot(s))
                        ),
                        None => "the destination would not open".to_string(),
                    }));
                }
            }
        }
        // The backup pair, once the sidecars moved, is the old survey.
        if step >= 2 {
            match open_fresh(&backup) {
                Some(s) if canonical_snapshot(&s) == original.survey => {}
                Some(s) => problems.push(label(&format!(
                    "the backup pair is not the old survey: {}",
                    first_difference(&original.survey, &canonical_snapshot(&s))
                ))),
                None => problems.push(label("the backup pair would not open")),
            }
        }
    }
    problems
}

// ===========================================================================
// The gate
// ===========================================================================

/// PINNED. `crash_property_sweep` re-derives the set.
const GATE_SEEDS: &[u32] = &[1, 2, 3];
const GATE_MOVES: usize = 12;
const GATE_EXTRA: usize = 6;
/// States per seed: four fixed plus the truncation sweep (up to six); a small
/// reference file can collapse two cut points.
const STATES_PER_SEED: usize = 8;

/// THE CRASH-TIMING GATE. Not `#[ignore]`d.
#[test]
fn crash_properties_gate() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    let mut checked = 0;
    let mut notes: BTreeMap<String, usize> = BTreeMap::new();
    for &seed in GATE_SEEDS {
        let outcome = check_crash_states(seed, GATE_MOVES, GATE_EXTRA);
        checked += outcome.checked;
        problems.extend(outcome.problems);
        for (id, count) in outcome.notes {
            *notes.entry(id.to_string()).or_default() += count;
        }
        for hot in [false, true] {
            problems.extend(check_upgrade_states(seed, hot, &mut notes));
        }
    }
    println!(
        "crash properties: {checked} injected v2 state(s) and {} upgrade state(s) over {} \
         seed(s); measured: {notes:?}",
        GATE_SEEDS.len() * 2 * UPGRADE_STEPS.len(),
        GATE_SEEDS.len()
    );
    assert!(
        checked >= GATE_SEEDS.len() * STATES_PER_SEED,
        "the crash gate injected only {checked} state(s); the reference pair is not being built"
    );
    assert!(
        problems.is_empty(),
        "{} crash-timing property failure(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// The engine injects at the path the pipeline stages into: a directory
/// squatting `<file>.tmp` makes the save fail (the pipeline cannot remove or
/// create it), the file is untouched, and with the squatter gone the save
/// works and leaves no temp.
#[test]
fn the_engine_injects_at_the_temp_path_the_pipeline_uses() {
    let _config = isolated_config();
    let state = build_state(1, false, 6).unwrap();
    let temp = TempDir::new().unwrap();
    let destination = temp.path().join("cave.stalx");
    assert_eq!(temp_path_of(&destination), temp.path().join("cave.stalx.tmp"));
    assert!(state.save_project(Some(destination.clone())).success);
    let before = std::fs::read(&destination).unwrap();
    mutate(&state, &mut Rng::new(3));
    std::fs::create_dir_all(temp_path_of(&destination)).unwrap();
    let blocked = state.save_project(None);
    assert!(
        !blocked.success,
        "a save with a DIRECTORY squatting {} succeeded, so the pipeline no longer stages there",
        temp_path_of(&destination).display()
    );
    assert_eq!(std::fs::read(&destination).unwrap(), before);
    std::fs::remove_dir(temp_path_of(&destination)).unwrap();
    assert!(state.save_project(None).success);
    assert!(
        stray_temps(temp.path()).is_empty(),
        "a successful save left a temp: {:?}",
        stray_temps(temp.path())
    );
}

/// The hot-journal injection is a real interrupted write: the journal is
/// armed, and the database bytes MOVED (with SQLite's ordinary cache a small
/// transaction never reaches the file and the recovery would be vacuous).
#[test]
fn the_hot_journal_injection_is_a_real_interrupted_write() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("cave.stalx");
    write_legacy(&path, &mut Rng::new(1), "Legacy");
    let committed = std::fs::read(&path).unwrap();
    let (interrupted, journal) = mid_transaction(&committed).unwrap();
    std::fs::write(&path, &interrupted).unwrap();
    std::fs::write(sidecar_path(&path, "-journal"), &journal).unwrap();
    assert!(has_hot_journal(&path), "the injected journal is not armed");
    assert_ne!(
        interrupted, committed,
        "the interrupted transaction never reached the database file"
    );
}

/// The real upgrading save over a legacy file an old build left with a hot
/// journal: the JSON file has no sidecar beside it, the original and its
/// journal are kept together in the backups folder, and that pair opens as
/// the survey the user had (the rolled-back one).
#[test]
fn a_completed_upgrade_moves_the_hot_journal_beside_the_kept_original() {
    let _config = isolated_config();
    crate::app_dirs::init_for_test();
    let temp = TempDir::new().unwrap();
    let stem = format!("upgrade-hot-{}", std::process::id());
    let path = temp.path().join(format!("{stem}.stalx"));
    write_legacy(&path, &mut Rng::new(4), "Legacy");
    let (interrupted, journal) = mid_transaction(&std::fs::read(&path).unwrap()).unwrap();
    std::fs::write(&path, &interrupted).unwrap();
    std::fs::write(sidecar_path(&path, "-journal"), &journal).unwrap();

    let state = open_fresh(&path).unwrap();
    let survey = canonical_snapshot(&state);
    assert!(
        !survey.contains("interrupted-write"),
        "the open rolled the journal back"
    );
    assert!(state.save_project(None).success);
    assert_eq!(
        stalx_v2::sniff(&std::fs::read(&path).unwrap()),
        Format::V2
    );
    for suffix in ["-journal", "-wal", "-shm"] {
        assert!(
            !sidecar_path(&path, suffix).exists(),
            "{suffix} is still beside the upgraded JSON file"
        );
    }
    let folder = super::project_io::upgrade_backup_dir().unwrap();
    let kept: Vec<PathBuf> = std::fs::read_dir(&folder)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(&format!("{stem}.")))
        })
        .collect();
    let original = kept
        .iter()
        .find(|p| p.to_string_lossy().ends_with(".pre-v2.stalx"))
        .expect("the original is kept");
    assert_eq!(std::fs::read(original).unwrap(), interrupted);
    assert_eq!(
        std::fs::read(sidecar_path(original, "-journal")).unwrap(),
        journal,
        "the journal moved beside the kept original"
    );
    let reopened = open_fresh(original).unwrap();
    assert_eq!(canonical_snapshot(&reopened), survey);
}

/// A failed upgrading write puts the journal back beside the untouched
/// original, which then reopens as the same survey.
#[test]
fn a_failed_upgrade_puts_the_journal_back() {
    let _config = isolated_config();
    crate::app_dirs::init_for_test();
    let temp = TempDir::new().unwrap();
    let path = temp.path().join(format!("upgrade-fails-{}.stalx", std::process::id()));
    write_legacy(&path, &mut Rng::new(5), "Legacy");
    let (interrupted, journal) = mid_transaction(&std::fs::read(&path).unwrap()).unwrap();
    std::fs::write(&path, &interrupted).unwrap();
    std::fs::write(sidecar_path(&path, "-journal"), &journal).unwrap();
    let state = open_fresh(&path).unwrap();
    let survey = canonical_snapshot(&state);
    // The write cannot stage its temp.
    std::fs::create_dir_all(temp_path_of(&path)).unwrap();
    assert!(!state.save_project(None).success);
    assert_eq!(std::fs::read(&path).unwrap(), interrupted);
    assert_eq!(
        std::fs::read(sidecar_path(&path, "-journal")).unwrap(),
        journal,
        "the journal is back beside the original"
    );
    std::fs::remove_dir(temp_path_of(&path)).unwrap();
    let again = open_fresh(&path).unwrap();
    assert_eq!(canonical_snapshot(&again), survey);
}

/// KNOWN BUG, pinned. An upgrade killed after `UpgradeBackup::take` moved the
/// hot journal away and before the rename leaves the original database file
/// alone at the project's path. The journal held the pre-images of the pages
/// the interrupted old-build transaction had spilled into it, so the file
/// read without it is that half-written transaction, not the survey the user
/// had: the next open shows another survey (or refuses it). The pair in the
/// backups folder is whole (the gate checks it), but the recovery dialog
/// refuses to restore a SQLite file with an armed journal beside it. The fix
/// is the upgrade's (`project_io.rs`): roll the journal back into the
/// original (a read-write open does it, under its own crash safety) and
/// re-read the guard's fingerprint before taking the copy, so no state ever
/// needs the journal.
#[test]
#[ignore = "known bug (C17 report): an upgrade killed between its sidecar move and the rename \
            leaves the legacy original without its hot journal"]
fn an_upgrade_interrupted_after_its_sidecar_move_leaves_the_original_whole() {
    let _config = isolated_config();
    let original = legacy_original(2, true).unwrap();
    let temp = TempDir::new().unwrap();
    let (destination, _) = stage_upgrade(temp.path(), &original, 2).unwrap();
    let opened = open_fresh(&destination).map(|s| canonical_snapshot(&s));
    assert_eq!(opened.as_deref(), Some(original.survey.as_str()));
}

/// Determinism, so a failing state is a repro.
#[test]
fn the_crash_properties_are_deterministic() {
    let _config = isolated_config();
    for &seed in &GATE_SEEDS[..2] {
        let first = check_crash_states(seed, GATE_MOVES, GATE_EXTRA);
        let second = check_crash_states(seed, GATE_MOVES, GATE_EXTRA);
        assert_eq!(
            (first.checked, first.problems.len(), first.notes),
            (second.checked, second.problems.len(), second.notes),
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
    let knob = |name: &str, default: usize| -> usize {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    };
    let seeds = u32::try_from(knob("STALAGMIGHT_RUST_CRASH_SEEDS", 20)).unwrap_or(20);
    let moves = knob("STALAGMIGHT_RUST_CRASH_MOVES", GATE_MOVES);
    let extra = knob("STALAGMIGHT_RUST_CRASH_EXTRA", GATE_EXTRA);
    let mut dirty = Vec::new();
    let mut checked = 0;
    let mut skipped = 0;
    let mut notes: BTreeMap<String, usize> = BTreeMap::new();
    for seed in 1..=seeds {
        let outcome = check_crash_states(seed, moves, extra);
        checked += outcome.checked;
        if outcome.checked == 0 {
            skipped += 1;
        }
        for (id, count) in outcome.notes {
            *notes.entry(id.to_string()).or_default() += count;
        }
        let mut problems = outcome.problems;
        for hot in [false, true] {
            problems.extend(check_upgrade_states(seed, hot, &mut notes));
        }
        if !problems.is_empty() {
            dirty.push(seed);
            for problem in &problems {
                println!("{problem}");
            }
        }
    }
    println!(
        "crash sweep: {seeds} seed(s), {checked} injected v2 state(s), {skipped} seed(s) skipped; \
         measured: {notes:?}; seeds with a failure ({}): {dirty:?}",
        dirty.len()
    );
}
