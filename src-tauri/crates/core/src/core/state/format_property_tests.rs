//! FILE-FORMAT properties: what the `.stalx` writer and reader owe each other.
//!
//! # Why this is its own engine
//!
//! The roundtrip fuzzer crosses save/close/reopen because undo history is
//! persisted, and it compares the PROJECT either side of the crossing. Neither
//! it nor the metamorphic engine ever looks at the FILE, and three properties
//! live only there:
//!
//! 1. **The writer is a function of the project.** Save, reload, save again -
//!    the second file has to say the same thing as the first. Finding 21 proved
//!    that `HashMap` iteration order is load-bearing in this codebase, and the
//!    writer walks `station_offsets` and `station_comments`, which are
//!    `HashMap`s. A writer whose OUTPUT depends on iteration order makes every
//!    save a diff against itself - which matters directly for the SpeleoDB sync
//!    paths, where a file that differs is a file that uploads.
//! 2. **The reader is a function of the file.** Two loads of one file must
//!    produce the same project, verdict and adjusted geometry. The metamorphic
//!    engine ASSERTS this at pair setup as a precondition; here it is the
//!    property under test, over random projects.
//! 3. **The fallbacks work.** `read.rs`'s `has_gauge_columns` probe,
//!    `ensure_schema_version`'s migrations and the undo-table restore all exist
//!    for files written by older builds - files nobody in the repo has, because
//!    fixtures are forbidden. So they are BUILT here, by writing a current file
//!    and surgically degrading it with SQL.
//!
//! # Bytes, and why the comparison is logical instead
//!
//! A `.stalx` is a SQLite database. Byte-comparing two of them compares page
//! allocation, freelist layout and the header's change counter - none of which
//! is a property of the project, and all of which can differ between two saves
//! that say exactly the same thing. So the fixpoint is asserted on the
//! DATABASE'S LOGICAL CONTENT: every user table, every row, rendered in a
//! deterministic order (see `dump_database`).
//!
//! That is not a weakening. It is strictly what "the writer said the same
//! thing" means, and it is sensitive to precisely the failure this exists to
//! catch: a row whose values differ, a row that appears or disappears, a column
//! written in a different form. It is deliberately NOT sensitive to row ORDER
//! within a table, because SQLite's `rowid` allocation is not part of the
//! format's meaning - and the dump sorts, which is what makes a `HashMap`-order
//! difference show up as a VALUE difference (the interesting kind) rather than
//! as noise.
//!
//! The one thing masked is the wall clock: `project.modified_at` is stamped by
//! the save itself (`persistence/sqlite/write.rs`), so two saves of one project
//! disagree on it by construction. Masked narrowly, by value shape, and the
//! column's PRESENCE is still compared.
//!
//! # Data
//!
//! Everything is generated in code, including the degraded schemas. No fixture
//! is read, copied or derived from.

#![expect(
    clippy::indexing_slicing,
    reason = "test module: see docs/linting.md for the per-test-module expect idiom"
)]
#![expect(
    clippy::print_stdout,
    reason = "a property sweep's per-seed summary IS its output, and `cargo test -- --nocapture` is               the only channel a test has"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use tempfile::TempDir;

use super::fuzz_undo_tests::{
    adjusted_diffs, adjusted_geometry_of, build_project, canonical_snapshot, describe_diffs,
    diff_snapshots, integrity_verdict, random_offset, random_shot, read_targets, Rng, COLORS,
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
use crate::test_helpers::isolated_config;

// ===========================================================================
// Reading a .stalx as data
// ===========================================================================

/// Is this string the wall clock the save stamps rather than survey data?
///
/// `project.modified_at` is set by the writer on every save
/// (`persistence/sqlite/write.rs`), so two saves of ONE project disagree on it
/// by construction and comparing it compares the clock. Matched on the VALUE's
/// shape, so the column's presence, and every other timestamp-shaped column,
/// still round-trips: a save that DROPPED a timestamp still fails.
fn is_timestamp(text: &str) -> bool {
    text.len() >= 19
        && text.as_bytes().get(4) == Some(&b'-')
        && text
            .as_bytes()
            .get(10)
            .is_some_and(|ch| *ch == b'T' || *ch == b' ')
        && text
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, '-' | 'T' | ':' | '.' | 'Z' | '+' | ' '))
}

/// Every user table of a `.stalx`, every row, rendered deterministically.
///
/// Sorted twice over: the table list, and the rows within each table. Sorting
/// the rows is what makes this a statement about CONTENT rather than about
/// SQLite's rowid allocation - and it is what makes a `HashMap`-iteration-order
/// difference in the writer surface as a differing VALUE (which is real) rather
/// than as a differing order (which is not).
pub(super) fn dump_database(path: &Path) -> Result<String, String> {
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    let mut tables: Vec<String> = {
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?;
        rows.filter_map(Result::ok).collect()
    };
    tables.sort();

    let mut out = String::new();
    for table in &tables {
        // The table name comes from `sqlite_master`, not from a caller, so the
        // interpolation cannot be a parameter and cannot be hostile.
        let mut statement = connection
            .prepare(&format!("SELECT * FROM \"{table}\""))
            .map_err(|error| error.to_string())?;
        let columns: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(ToString::to_string)
            .collect();
        let mut rows: Vec<String> = statement
            .query_map([], |row| {
                let mut cells = Vec::with_capacity(columns.len());
                for (index, name) in columns.iter().enumerate() {
                    let value = row.get_ref(index).map_or_else(
                        |_| "<unreadable>".to_string(),
                        |raw| match raw {
                            rusqlite::types::ValueRef::Null => "null".to_string(),
                            rusqlite::types::ValueRef::Integer(value) => value.to_string(),
                            rusqlite::types::ValueRef::Real(value) => format!("{value:?}"),
                            rusqlite::types::ValueRef::Text(bytes) => {
                                let text = String::from_utf8_lossy(bytes).into_owned();
                                if is_timestamp(&text) {
                                    "<timestamp>".to_string()
                                } else {
                                    text
                                }
                            }
                            // The undo/redo payloads are gzipped JSON blobs.
                            // Compared by LENGTH and by content hash rather than
                            // rendered: a blob that changed is what matters, and
                            // a 200 kB hex dump in a failure payload is not
                            // readable by anyone.
                            rusqlite::types::ValueRef::Blob(bytes) => {
                                format!("blob[{}]:{:016x}", bytes.len(), fnv1a(bytes))
                            }
                        },
                    );
                    cells.push(format!("{name}={value}"));
                }
                // Joined with a unit separator rather than a space: a
                // `description` or a section `name` can contain spaces, and a
                // row split on the wrong character makes every later cell
                // compare against the wrong column. Rendered back as spaces
                // only when a failure payload prints it.
                Ok(cells.join("\u{1f}"))
            })
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .collect();
        rows.sort();
        use std::fmt::Write as _;
        let _ = writeln!(out, "== {table} ({} row(s))", rows.len());
        for row in rows {
            out.push_str(&row);
            out.push('\n');
        }
    }
    Ok(out)
}

/// FNV-1a, so a blob can be compared without being printed. Not a security
/// hash and not used as one: it is here to turn "these two gzipped undo
/// payloads differ" into one readable line.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The first few differing lines between two dumps, for a failure payload.
///
/// `labels` names the two sides. The crash engine
/// ([`super::crash_property_tests`]) renders the same payload against "the old
/// file" and "the new file" rather than two saves, and a shared differ that
/// hard-coded one engine's nouns would mislabel the other's failures.
pub(super) fn dump_differences(left: &str, right: &str, labels: (&str, &str)) -> Vec<String> {
    let mut out = Vec::new();
    let (mut a, mut b) = (left.lines(), right.lines());
    let width = labels.0.chars().count().max(labels.1.chars().count());
    let mut line = 0;
    loop {
        match (a.next(), b.next()) {
            (None, None) => break,
            (first, second) => {
                line += 1;
                if first != second {
                    out.push(format!(
                        "  line {line}:\n    {:<width$} {}\n    {:<width$} {}",
                        format!("{}:", labels.0),
                        first.unwrap_or("<end of dump>").replace('\u{1f}', " "),
                        format!("{}:", labels.1),
                        second.unwrap_or("<end of dump>").replace('\u{1f}', " "),
                        width = width + 1
                    ));
                }
            }
        }
        if out.len() >= 6 {
            out.push("  ...and more".to_string());
            break;
        }
    }
    out
}

// ===========================================================================
// The five oracles, on a loaded state
// ===========================================================================

/// Every id allocator in the open project. Same reasoning as the metamorphic
/// engine's: between two loads of ONE file there is no legitimate asymmetry,
/// and two loads that disagree hand out different ids from then on.
fn allocators(state: &AppState) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    let Some(project) = state.get_project_state() else {
        return out;
    };
    out.insert("next_section_id".to_string(), project.next_section_id);
    out.insert(
        "next_closure_id".to_string(),
        u32::try_from(project.next_closure_id).unwrap_or(u32::MAX),
    );
    for section in &project.sections {
        out.insert(
            format!("section {}.next_shot_id", section.id),
            section.next_shot_id,
        );
    }
    out
}

/// Everything two loaded states are compared on.
pub(super) struct Loaded {
    stored: String,
    verdict: String,
    adjusted: String,
    allocators: BTreeMap<String, u32>,
    undo_len: usize,
    redo_len: usize,
}

pub(super) fn observe(state: &AppState) -> Loaded {
    Loaded {
        stored: canonical_snapshot(state, false),
        verdict: integrity_verdict(state).describe(),
        adjusted: adjusted_geometry_of(state),
        allocators: allocators(state),
        undo_len: state.get_undo_stack().len(),
        redo_len: state.get_redo_stack().len(),
    }
}

/// Every way two loaded states differ. Empty means the loader is a function of
/// the file, as far as anything can see.
pub(super) fn load_differences(left: &Loaded, right: &Loaded) -> Vec<String> {
    let mut out = Vec::new();
    let diffs = diff_snapshots(&left.stored, &right.stored);
    if !diffs.is_empty() {
        out.push(format!(
            "the loaded projects differ in {} path(s):\n{}",
            diffs.len(),
            describe_diffs(&diffs)
        ));
    }
    if left.verdict != right.verdict {
        out.push(format!(
            "the integrity verdicts differ:\n    first:  {}\n    second: {}",
            left.verdict, right.verdict
        ));
    }
    let geometry = adjusted_diffs(&left.adjusted, &right.adjusted);
    if !geometry.is_empty() {
        out.push(format!(
            "the ADJUSTED geometry differs:\n    {}",
            geometry.join("\n    ")
        ));
    }
    if left.allocators != right.allocators {
        out.push(format!(
            "the id ALLOCATORS differ:\n    first:  {:?}\n    second: {:?}",
            left.allocators, right.allocators
        ));
    }
    if (left.undo_len, left.redo_len) != (right.undo_len, right.redo_len) {
        out.push(format!(
            "the restored history stacks differ: first undo={} redo={}, second undo={} redo={}",
            left.undo_len, left.redo_len, right.undo_len, right.redo_len
        ));
    }
    out
}

// ===========================================================================
// Building a project to save
// ===========================================================================

/// A random project with history on both stacks.
///
/// The undo stacks matter as much as the project does: they are persisted into
/// the same file, they are the part of it nothing outside this programme
/// exercises, and a run that saved an empty history would prove nothing about
/// the tables that carry them.
pub(super) fn build_state(seed: u32, closures: bool, moves: usize) -> Option<AppState> {
    let state = AppState::new().ok()?;
    let mut rng = Rng::new(seed);
    build_project(&state, &mut rng, closures);
    for _ in 0..moves {
        mutate(&state, &mut rng);
    }
    // Leave the REDO stack non-empty too: both stacks are persisted and only
    // one of them is ever exercised by a run that never steps back.
    for _ in 0..2 {
        state.undo();
    }
    Some(state)
}

/// One random command, from the same narrow set the metamorphic prelude uses -
/// nothing with an open finding against it, so a property failure here is about
/// the FILE rather than about a defect the project was built with.
pub(super) fn mutate(state: &AppState, rng: &mut Rng) {
    let targets = read_targets(state);
    if targets.sections.is_empty() {
        return;
    }
    match rng.below(6) {
        0 => {
            let count = 1 + rng.below(3);
            let shots: Vec<ShotData> = (0..count).map(|_| random_shot(rng)).collect();
            state.execute(Command::CreateSection {
                name: Some(format!("fmt-{}", rng.range(1000, 9999))),
                surveyor: None,
                date: None,
                color: (*rng.pick(COLORS)).to_string(),
                direction: None,
                shots,
            });
        }
        1 => {
            let (section_id, shots) = targets.sections[rng.below(targets.sections.len())].clone();
            let after =
                (!shots.is_empty() && rng.chance(50)).then(|| shots[rng.below(shots.len())]);
            state.execute(Command::CreateShot {
                section_id,
                shot: random_shot(rng),
                after_shot_id: after,
                before_shot_id: None,
            });
        }
        2 => {
            let Some((section_id, shot_id)) = pick_shot(state, rng) else {
                return;
            };
            state.execute(Command::UpdateShot {
                section_id,
                shot_id,
                updates: ShotUpdates {
                    length: Some(rng.float(1.0, 15.0)),
                    ..ShotUpdates::default()
                },
            });
        }
        3 => {
            state.execute(Command::MoveSection {
                section_id: targets.sections[rng.below(targets.sections.len())].0,
                offset: random_offset(rng),
            });
        }
        4 => {
            if targets.stations.is_empty() {
                return;
            }
            let (section_id, index) = targets.stations[rng.below(targets.stations.len())];
            state.execute(Command::SetStationComment {
                station_ref: StationRef::new(section_id, index),
                comment: Some(format!("fmt-note-{}", rng.range(0, 9999))),
            });
        }
        _ => {
            if targets.sections.len() < 2 || targets.stations.len() < 2 {
                return;
            }
            let a = targets.stations[rng.below(targets.stations.len())];
            let candidates: Vec<(SectionId, u32)> = targets
                .stations
                .iter()
                .copied()
                .filter(|station| station.0 != a.0)
                .collect();
            if candidates.is_empty() {
                return;
            }
            let b = candidates[rng.below(candidates.len())];
            state.execute(Command::MergeStations {
                station_a: StationRef::new(a.0, a.1),
                station_b: StationRef::new(b.0, b.1),
            });
        }
    }
}

fn pick_shot(state: &AppState, rng: &mut Rng) -> Option<(SectionId, ShotId)> {
    let targets = read_targets(state);
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
}

/// Open `path` into a fresh `AppState`.
pub(super) fn open_fresh(path: &Path) -> Option<AppState> {
    let state = AppState::new().ok()?;
    state
        .execute(Command::OpenProject {
            path: path.to_path_buf(),
        })
        .success
        .then_some(state)
}

// ===========================================================================
// PROPERTY 1: save -> load -> save is a fixpoint
// ===========================================================================

/// What one seed's fixpoint check found.
struct Fixpoint {
    checked: bool,
    problems: Vec<String>,
    /// Classified (known-defect) differences, by id. EMPTY as of finding 23's
    /// fix - this check has no classifier left, so every difference lands in
    /// `problems`. The field and the sweep's `classified: {...}` line stay
    /// because they are the reporting shape all three fuzz gates share, and
    /// because the next open finding is classified here or nowhere.
    hits: BTreeMap<&'static str, usize>,
}

impl Fixpoint {
    const fn skipped() -> Self {
        Self {
            checked: false,
            problems: Vec::new(),
            hits: BTreeMap::new(),
        }
    }
}

// FINDING 23 was classified here, by `is_finding_23`: "the two dumps agree
// everywhere except in float dust, and only in the two places the first
// save/load cycle can legitimately move one - the gzipped JSON history payloads
// and the DERIVED offset tables". FIXED, in two steps, and the classifier is
// deleted per the delete-when-fixed contract along with the dump-matching
// helpers it was the only caller of (`tables_of`, `cells_of`, `may_drift`,
// `row_key`, `within_a_few_ulps`). From here on ANY difference between two
// generations is a problem and fails the gate; see `check_save_load_save` for
// the two mechanisms and the measurement.

/// Save, load, save - three generations - and assert the one thing that is
/// actually true of them.
///
/// **What is NOT asserted, and why.** "The drift settles after one cycle" was
/// asserted first, on the strength of seed 1 measuring exactly that (generation
/// 1 vs 0 differed in 7 lines, 2 vs 1 and 3 vs 2 in none). It was false when it
/// was written: over eight runs of the gate the assertion held four times and
/// failed four times, on the same seeds and the same inputs - so the drift did
/// not converge, it RE-ROLLED, which put it in finding 21's class. Asserting it
/// would have been a coin flip in CI.
///
/// The re-roll has since been root-caused and fixed, and it was exactly that
/// class: `UndoData`'s `previous_offsets` is a `HashMap`, so re-serializing one
/// unchanged payload wrote its keys in a different order each time
/// (`{"3":null,"2":[..]}` one save, `{"2":[..],"3":null}` the next). It now
/// serializes in key order (`core/undo.rs`), and over eight runs of this gate
/// NO generation-1-to-2 pair differs at all - the cycle converges after one
/// generation, every time.
///
/// Finding 23's remaining generation-0-to-1 drift is fixed too, in two steps,
/// and this gate now records ZERO classified hits per run:
///
/// * the history payloads' decimal float text - the load reparsed it and
///   `serde_json`'s parser is not its printer's inverse
///   (`"right":2.7600000000000002` read back as the double that prints `2.76`)
///   - went with the `float_roundtrip` feature (src-tauri/Cargo.toml);
/// * the last one was a `section_offsets` value settling on reload: the
///   loader's from-scratch cascade did not bit-reproduce what the incremental
///   path had accumulated (`offset_y=2.58` saved, `2.579999999999999` after the
///   reload re-solved it, then stable). `cascade_bfs`
///   (models/project/connectivity.rs) now DECLINES a correction below
///   `Project::OFFSET_DUST_M` rather than applying one that lands under that
///   bound, so the re-solve leaves an already-correct offset exactly where the
///   file put it.
///
/// **What IS asserted:** no save/load cycle may change ANYTHING in the file's
/// logical content. That was already the shape of the property - "nothing may
/// change except bounded float dust in the derived and history tables" - and the
/// `except` clause is what has gone, because nothing lands in it any more:
/// `is_finding_23` is deleted per the delete-when-fixed contract, so a
/// generation pair that differs at all is now a problem and fails.
///
/// Deleting a tolerance is a strengthening, so it was measured before it was
/// made rather than assumed: consecutive runs of this gate classify nothing -
/// each run is a fresh process, so each re-rolls the `HashMap` seeds that made
/// the FIRST version of this a coin flip - and the sweep classifies nothing over
/// its seeds with closures and without. What is still NOT done is asserting
/// convergence as a separate claim ("the drift settles after one cycle"): there
/// is no drift left to settle, so the claim has no content, and the equality
/// check above already says everything true about it.
fn check_save_load_save(seed: u32, closures: bool, moves: usize) -> Fixpoint {
    let Some(state) = build_state(seed, closures, moves) else {
        return Fixpoint::skipped();
    };
    let Ok(temp) = TempDir::new() else {
        return Fixpoint::skipped();
    };
    let mut dumps: Vec<String> = Vec::new();
    let mut live = state;
    for generation in 0..3 {
        let path: PathBuf = temp.path().join(format!("gen{generation}.stalx"));
        if !live.save_project(Some(path.clone())).success {
            // A save-blocked project (an unresolvable cross-reference) is
            // refused by design, and a run that could not write a file has
            // nothing to compare.
            return Fixpoint::skipped();
        }
        let Ok(dump) = dump_database(&path) else {
            return Fixpoint::skipped();
        };
        dumps.push(dump);
        drop(live);
        let Some(reopened) = open_fresh(&path) else {
            return Fixpoint {
                checked: true,
                problems: vec![format!(
                    "seed {seed}: the file the writer just wrote would not open (generation \
                     {generation})"
                )],
                hits: BTreeMap::new(),
            };
        };
        live = reopened;
    }

    let mut problems = Vec::new();
    let hits = BTreeMap::new();
    for generation in 1..dumps.len() {
        if dumps[generation - 1] == dumps[generation] {
            continue;
        }
        problems.push(format!(
            "seed {seed}: save/load generation {generation} differs from generation {}:\n{}",
            generation - 1,
            dump_differences(
                &dumps[generation - 1],
                &dumps[generation],
                ("first save", "second save")
            )
            .join("\n")
        ));
    }
    Fixpoint {
        checked: true,
        problems,
        hits,
    }
}

// ===========================================================================
// PROPERTY 2: the loader is a function of the file
// ===========================================================================

fn check_load_is_a_function(seed: u32, closures: bool, moves: usize, loads: usize) -> Fixpoint {
    let Some(state) = build_state(seed, closures, moves) else {
        return Fixpoint::skipped();
    };
    let Ok(temp) = TempDir::new() else {
        return Fixpoint::skipped();
    };
    let path: PathBuf = temp.path().join("once.stalx");
    if !state.save_project(Some(path.clone())).success {
        return Fixpoint::skipped();
    }
    drop(state);

    let Some(first) = open_fresh(&path) else {
        return Fixpoint {
            checked: true,
            problems: vec![format!("seed {seed}: the file would not open")],
            hits: BTreeMap::new(),
        };
    };
    let baseline = observe(&first);
    drop(first);

    let mut problems = Vec::new();
    for load in 1..loads {
        let Some(again) = open_fresh(&path) else {
            problems.push(format!("seed {seed}: load {load} of the same file failed"));
            continue;
        };
        let differences = load_differences(&baseline, &observe(&again));
        if !differences.is_empty() {
            problems.push(format!(
                "seed {seed}: load {load} of the SAME file produced a different project:\n  {}",
                differences.join("\n  ")
            ));
        }
    }
    Fixpoint {
        checked: true,
        problems,
        hits: BTreeMap::new(),
    }
}

// ===========================================================================
// PROPERTY 4: the legacy fallbacks
// ===========================================================================

/// One surgical degradation of a current file, and what the loader owes it.
struct Degradation {
    id: &'static str,
    /// What the code promises about a file in this shape, and where it says so.
    promise: &'static str,
    /// SQL applied to a freshly written current file.
    surgery: fn(&Connection) -> Result<(), rusqlite::Error>,
    /// Must the load succeed?
    must_open: bool,
}

const DEGRADATIONS: &[Degradation] = &[
    // `read.rs:261-277` probes `pragma_table_info('sections')` for the gauge
    // columns and selects NULL literals when they are missing, so a file written
    // before they existed still loads - with no gauge anchors, which is exactly
    // what a file without the columns can mean.
    Degradation {
        id: "sections-without-the-gauge-columns",
        promise: "persistence/sqlite/read.rs:261-277 - `has_gauge_columns` selects NULL literals \
                  when the columns are absent, so a pre-gauge file still opens",
        surgery: |connection| {
            connection.execute_batch(
                "ALTER TABLE sections DROP COLUMN gauge_shot_uuid;
                 ALTER TABLE sections DROP COLUMN gauge_neighbour_uuid;",
            )
        },
        must_open: true,
    },
    // `ensure_schema_version` migrates a file whose `PRAGMA user_version` is
    // behind, so setting it to 1 runs EVERY migration in sequence.
    //
    // `must_open` is FALSE, and the reason is a correction worth keeping: this
    // surgery produces a file no build ever wrote - a CURRENT schema wearing an
    // ancient version marker - so the migrations run against tables that already
    // have their post-migration shape and some of them legitimately fail. "It
    // opens" is therefore not promised. What IS promised, and what this asserts,
    // is that the loader neither panics nor half-opens: it reports cleanly and
    // leaves no project behind. Measured: it declines, which is correct.
    Degradation {
        id: "an-ancient-user-version",
        promise: "persistence/sqlite/schema.rs:1129 - every migration runs in sequence; an \
                  impossible version/schema pairing must be REFUSED cleanly, not panic",
        surgery: |connection| connection.execute_batch("PRAGMA user_version = 1;"),
        must_open: false,
    },
    // The same, one column over: `get_project_schema_version` reads
    // `project.schema_version` and `ensure_schema_version` reconciles the two.
    // A file whose two version markers DISAGREE is the shape a partial migration
    // leaves behind, and it is the one nobody has a fixture for.
    Degradation {
        id: "disagreeing-version-markers",
        promise: "persistence/sqlite/schema.rs:1085-1132 - the project-table version and PRAGMA \
                  user_version are reconciled on open",
        surgery: |connection| connection.execute_batch("UPDATE project SET schema_version = 2;"),
        must_open: true,
    },
    // The undo tables are the newest part of the format and the part a
    // downgrade drops first. A file without them is a project with no history,
    // not a broken project.
    Degradation {
        id: "no-undo-tables-at-all",
        promise: "the undo stacks are an addition to the format; a file written before them is a \
                  project with an empty history",
        surgery: |connection| {
            connection.execute_batch(
                "DROP TABLE IF EXISTS undo_stack;
                 DROP TABLE IF EXISTS redo_stack;",
            )
        },
        must_open: true,
    },
    // `station_names` arrived WITHOUT a schema bump (outside readers pin v18),
    // so a v18 file from before it takes the version fast path and nothing
    // creates the table on open. It must load with no names and save again -
    // the save re-creates the table inside its transaction.
    Degradation {
        id: "v18-file-without-station-names",
        promise: "persistence/sqlite/read.rs `load_station_texts` reads an absent table as empty, \
                  and write.rs creates it before its DELETE, so a pre-names v18 file opens and \
                  saves",
        surgery: |connection| connection.execute_batch("DROP TABLE station_names;"),
        must_open: true,
    },
];

/// Apply one degradation to a fresh file and report what the loader did.
fn check_degradation(seed: u32, degradation: &Degradation) -> Vec<String> {
    let Some(state) = build_state(seed, false, 14) else {
        return Vec::new();
    };
    let Ok(temp) = TempDir::new() else {
        return Vec::new();
    };
    let path: PathBuf = temp.path().join("degraded.stalx");
    if !state.save_project(Some(path.clone())).success {
        return Vec::new();
    }
    drop(state);

    // The surgery itself may be refused by SQLite (an old bundled version with
    // no DROP COLUMN, say). That is a SKIP, not a failure: the degradation was
    // never built, so nothing was tested.
    let Ok(connection) = Connection::open(&path) else {
        return Vec::new();
    };
    if (degradation.surgery)(&connection).is_err() {
        return Vec::new();
    }
    drop(connection);

    let mut problems = Vec::new();
    let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| open_fresh(&path)));
    match opened {
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_string())
                })
                .unwrap_or_else(|| "<non-string panic>".to_string());
            problems.push(format!(
                "seed {seed} [{}]: opening the degraded file PANICKED - a file shape the code has \
                 a documented fallback for must not take the process down\n  promise: {}\n  \
                 panic: {}",
                degradation.id,
                degradation.promise,
                message.lines().next().unwrap_or_default()
            ));
        }
        Ok(None) => {
            if degradation.must_open {
                problems.push(format!(
                    "seed {seed} [{}]: the degraded file did not open, but the code promises a \
                     fallback for exactly this shape\n  promise: {}",
                    degradation.id, degradation.promise
                ));
            }
        }
        Ok(Some(state)) => {
            // It opened. The project then has to be USABLE: a load that returns
            // a project the app cannot integrity-check or draw is a load that
            // failed loudly enough to matter.
            let verdict = integrity_verdict(&state);
            if verdict.describe().is_empty() {
                problems.push(format!(
                    "seed {seed} [{}]: the degraded file opened but produced no integrity verdict",
                    degradation.id
                ));
            }
            if state.get_project_state().is_none() {
                problems.push(format!(
                    "seed {seed} [{}]: the degraded file opened but left no project",
                    degradation.id
                ));
            }
            // And it has to SAVE again, which is the whole point of a migration:
            // a file the app can open but not write is a file the user loses.
            let round = temp.path().join("resaved.stalx");
            if !state.save_project(Some(round)).success {
                problems.push(format!(
                    "seed {seed} [{}]: the degraded file opened but could not be saved again, so \
                     the migration is one-way",
                    degradation.id
                ));
            }
        }
    }
    problems
}

// ===========================================================================
// The gate
// ===========================================================================

/// PINNED. Clean today; `format_property_sweep` re-derives the set.
const GATE_SEEDS: &[u32] = &[1, 2, 3, 4, 5, 6];
/// Commands before the save. Enough for several sections, a merge or two, some
/// station comments and an offset - the shapes the writer's `HashMap` walks
/// touch.
const GATE_MOVES: usize = 16;
/// Loads of one file, for the loader-is-a-function property.
const GATE_LOADS: usize = 4;

/// THE FILE-FORMAT GATE. Not `#[ignore]`d.
#[test]
fn format_properties_gate() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    let mut checked = (0, 0, 0);
    let mut total: BTreeMap<&'static str, usize> = BTreeMap::new();
    for &seed in GATE_SEEDS {
        let fixpoint = check_save_load_save(seed, false, GATE_MOVES);
        checked.0 += usize::from(fixpoint.checked);
        problems.extend(fixpoint.problems);
        for (id, count) in fixpoint.hits {
            *total.entry(id).or_insert(0) += count;
        }

        let function = check_load_is_a_function(seed, false, GATE_MOVES, GATE_LOADS);
        checked.1 += usize::from(function.checked);
        problems.extend(function.problems);
    }
    for degradation in DEGRADATIONS {
        let found = check_degradation(GATE_SEEDS[0], degradation);
        checked.2 += 1;
        problems.extend(found);
    }
    println!(
        "format properties: {} fixpoint check(s), {} loader check(s) x {GATE_LOADS} loads, {} \
         degradation(s); classified: {total:?}",
        checked.0, checked.1, checked.2
    );
    // A run that checked nothing passes vacuously - the failure mode a
    // skip-on-anything-unexpected design invites.
    assert!(
        checked.0 >= GATE_SEEDS.len() && checked.1 >= GATE_SEEDS.len(),
        "the format gate checked only {:?} of {} seed(s); the builder is not producing saveable \
         projects",
        checked,
        GATE_SEEDS.len()
    );
    assert!(
        problems.is_empty(),
        "{} file-format property failure(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// Determinism, so a failing seed is a repro.
///
/// The hit COUNT is deliberately not part of the shape - only the set of ids
/// is. Finding 23 is itself nondeterministic (the same seed classifies 1 or 2
/// generation pairs depending on how the writer's `HashMap` walks fall), so
/// comparing counts would make a determinism test fail for the app's
/// nondeterminism rather than the engine's, which is the only thing it is
/// about. `checked` and the problem count - the two things the ENGINE decides -
/// are compared exactly. When finding 23 is fixed, compare the counts too.
#[test]
fn the_format_properties_are_deterministic() {
    let _config = isolated_config();
    for &seed in &GATE_SEEDS[..3] {
        let first = check_save_load_save(seed, false, GATE_MOVES);
        let second = check_save_load_save(seed, false, GATE_MOVES);
        let ids =
            |fixpoint: &Fixpoint| -> Vec<&'static str> { fixpoint.hits.keys().copied().collect() };
        assert_eq!(
            (first.checked, first.problems.len(), ids(&first)),
            (second.checked, second.problems.len(), ids(&second)),
            "seed {seed} did not replay identically"
        );
    }
}

/// The SWEEP, and the tool for re-choosing `GATE_SEEDS`.
///
/// ```text
/// STALAGMIGHT_RUST_FORMAT_SEEDS=60 STALAGMIGHT_RUST_FORMAT_CLOSURES=1 \
///   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
///   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
///   --lib format_property_sweep -- --ignored --nocapture
/// ```
#[test]
#[ignore = "sweep: prints per-seed status, used to choose the format-property GATE_SEEDS"]
fn format_property_sweep() {
    let _config = isolated_config();
    let seeds: u32 = std::env::var("STALAGMIGHT_RUST_FORMAT_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(30);
    let moves: usize = std::env::var("STALAGMIGHT_RUST_FORMAT_MOVES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(GATE_MOVES);
    let loads: usize = std::env::var("STALAGMIGHT_RUST_FORMAT_LOADS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(GATE_LOADS);
    let closures = std::env::var("STALAGMIGHT_RUST_FORMAT_CLOSURES").is_ok();

    let mut dirty = Vec::new();
    let mut classified: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut skipped = 0;
    for seed in 1..=seeds {
        let mut seed_problems = Vec::new();
        let fixpoint = check_save_load_save(seed, closures, moves);
        if !fixpoint.checked {
            skipped += 1;
        }
        for (id, count) in fixpoint.hits {
            *classified.entry(id).or_insert(0) += count;
        }
        seed_problems.extend(fixpoint.problems);
        let function = check_load_is_a_function(seed, closures, moves, loads);
        seed_problems.extend(function.problems);
        if !seed_problems.is_empty() {
            dirty.push(seed);
            for problem in &seed_problems {
                println!("{problem}");
            }
        }
    }
    for degradation in DEGRADATIONS {
        for seed in 1..=seeds.min(4) {
            for problem in check_degradation(seed, degradation) {
                println!("{problem}");
                dirty.push(seed);
            }
        }
    }
    println!(
        "format sweep: {seeds} seed(s), closures={closures}, {skipped} skipped; classified: \
         {classified:?}; seeds with an UNEXPLAINED failure ({}): {dirty:?}",
        dirty.len()
    );
}
