//! A seeded undo/redo fuzzer that runs INSIDE the Rust test suite, on the
//! station-graph pipeline, and the CI gates built on it.
//!
//! # What it drives, and what it holds the app to
//!
//! Random `AppState` commands - every command family (lines, shots, stations,
//! merges and detaches, closures, datums, the move tools, comments, names,
//! credit, Sections, settings), the bypass recipes (split shot, delete and
//! split, range delete shift and split, delete last shot and line, Split Line
//! Here, Combine Lines, create and merge, imports, Add .stalx) and batches -
//! interleaved with undo, redo, multi-step jumps and save/close/reopen
//! crossings. After EVERY move:
//!
//! | oracle | what it compares |
//! |---|---|
//! | INVARIANTS | `Project::validate()` is empty (plan section 2) |
//! | ROUNDTRIP | at every recorded depth, the canonical `.stalx` v2 bytes of the survey (`stalx_v2::encode`, `modified_at` masked, counters clamped to the recorded ones after checking they did not go DOWN) are the bytes recorded there, BIT FOR BIT, plus the testkit's bitwise `same_state` |
//! | SESSION IDS | exact (`testkit::session_ids`) while every step since the depth was recorded carried in-session ids; always sound (`testkit::session_ids_sound`): unique, inside the line's window, below its allocator, also after a reopen, where persisted entries mint |
//! | COUNTERS | the station, closure and Section counters never go down, across any move |
//! | DERIVED VIEWS | the memoised raw layout and Computed View (`AppState::raw_positions_of`, `adjusted_positions`) are the fresh ones, bit for bit, and each depth's views are the ones recorded there |
//! | ENTRY CONTENT | every `UndoHistoryItem` on both stacks (label, kind, timestamp, preview) is the one recorded when its entry was pushed, at the depth the stacks put it, and survives a save and a reopen unchanged |
//! | REFUSALS | a refused command, a refused batch, an undo past the floor or a redo past the top, and a step through an entry whose preconditions a tamper broke (`testkit::tamper`) change NOTHING: not the survey, not its session ids or `modified_at`, not the stacks, and the refused entry stays where it was |
//! | NO-OPS | a command that succeeds without recording anything changed nothing and kept the redo branch |
//! | JUMPS | `undo_to` / `redo_to` of N land where N single steps do, with the same reversed lines |
//!
//! There is NO tolerance and NO classifier anywhere in this module. The old
//! engine compared JSON snapshots within 1e-9 m and kept a table of known-bug
//! classifiers (issue #413); the station-graph engine restores a state by its
//! id-keyed changeset to the bit, so every difference fails the run with the
//! seed, the depth, the move log and the first differing lines of the v2 text.
//!
//! # The tiers around it
//!
//! | tier | drives | catches |
//! |---|---|---|
//! | this module | `AppState` directly | undo/redo, history persistence, derived-view memo regressions |
//! | `models::survey::changeset` property tests | the engine on a model walk | the changeset itself |
//! | `metamorphic_undo_tests.rs` | two `AppState`s | two routes to the same edit disagreeing |
//! | `adversarial_tests.rs`, `concurrency_tests.rs`, `crash_property_tests.rs` | hostile arguments, threads, kills | refusals that mutate, races, torn saves |
//! | agent harness | the real app over IPC | the frontend store and the IPC boundary |
//!
//! `fuzz_shrink_tests.rs` minimizes what this module finds (ddmin over a
//! recorded trace); `fuzz_rarity_tests.rs` is a measurement of corpus guidance.
//!
//! # Modes and knobs
//!
//! Every env knob keeps its name: the farm drives them.
//! Prefix `STALAGMIGHT_RUST_FUZZ_` on all of them.
//!
//! | env | what it does |
//! |---|---|
//! | `SEEDS=n` | seeds `1..=n` for the sweeps (each seed runs in-memory AND persistence) |
//! | `SEED_START=s` | the sweeps start at seed `s` instead of 1 (rolling windows) |
//! | `ITERATIONS=n` | moves per run |
//! | `THREADS=n` | sweep workers; the output does not depend on it |
//! | `TRACE=1`, `VERBOSE=1` | stream every move; print whole failure payloads |
//! | `JUMPS=1` | multi-step `undo_to` / `redo_to`, each checked against N single steps |
//! | `CLOSURES=1` | a genuine loop in the starting project |
//! | `DIVERGENCE=1` | build session ids out of positional order, push the id-addressed entries on top, then cross |
//! | `CLOSURE_SHAPES=1` | two closures (list order) and, at the first crossing, the orphan shape (a datum removed under a closure between two placed pieces) |
//! | `HORIZON=1` | thousands of moves past the history limit: the FIFO window slides |
//! | `CROSSINGS=n` | save/close/reopen crossings per persistence run |
//! | `CONFIG=1` | draw `undo_history_limit` from a ladder (second generator) |
//! | `COMBINED=1` | horizon, closures, closure shapes, jumps, three crossings and a drawn limit at once |
//! | `LEGACY_ORIGIN=1` | start from a migrated legacy (SQLite) project: a seeded 0.6.0 survey written by the frozen 0.6.0 save and opened through the migration |
//! | `FORMAT_CORRUPT=1` | damage one persisted history entry before a reopen (`fuzz_corrupt_tests.rs`) |
//! | `TRIM_CROSSINGS=1` | cross even when the save keeps only the newest `undo_persist_limit` entries per stack, and model that window |
//! | `SAVES=1` | saves in place, and back-to-back crossings |
//! | `EXTENDED=1` | a tenth more of the rarer actions (`fuzz_extended_tests.rs`); the default draw already includes every one |
//! | `LARGE=1` | start from a project with long lists (an 80-line junction chain, 78 closures, 70 placed islands, 70 import hashes of each kind) |
//! | `PERSISTENCE=1` | horizon sweep only: cross as well |
//! | `CORPUS_BUDGET=n` | `fuzz_corpus_sweep` |
//! | `SHRINK=`, `REPLAY=`, `SHRINK_TARGET=`, `SHRINK_OUT=`, `SHRINK_PROBES=` | `fuzz_shrink_tests.rs` |
//! | `RARITY_*` | `fuzz_rarity_tests.rs` |
//!
//! A run is a pure function of `(seed, Plan)`: the generator is integer-only
//! (mulberry32), every draw a knob makes comes from a second generator seeded
//! `seed ^ COVERAGE_STREAM` (and the config draw from `seed ^ CONFIG_STREAM`),
//! and each run builds its own `AppState` and temp dir. A seed replays bit for
//! bit: `the_fuzzer_is_deterministic` pins that over the gates, and a trace
//! (`TraceMove`, `Op::describe` strings) replays through the same `apply_op`.
//!
//! # References
//!
//! A drawn move names everything by what survives a reload and the removal
//! of the moves around it: lines by id, shots by `Shot::uuid` (session
//! `ShotId`s are renumbered by the loader and never persisted), stations by
//! `StationId` plus the line whose slot was picked (`StationT`, resolved to
//! that slot or the station's first one), closures by id. A replay pairs the
//! recorded run's ids with its own through what each move created (`Refs`).
//!
//! # What was retired with the old model, and why
//!
//! The #413 bug-identification matrix, every classifier and the tolerances
//! pinned machinery the redesign deleted: line offsets, merge records and
//! their alignment, gauges, uuid remaps of keys, the delta engine's whole-list
//! threshold, positional re-keying and the save/reload heals. Their
//! regression guards are the bit-exact oracles above. Retired individually:
//!
//! * the ALIGNMENT oracle (raw and resolved merge distances) and the app's
//!   own merge-alignment assert: a junction is one station, so there is no
//!   distance to measure. `finding_14_no_insert_leaves_a_merge_out_of_alignment`
//!   went with it.
//! * the id-agnostic snapshot rewrite after a reload: session ids are not in
//!   the v2 bytes, and their own oracle says when they must be exact.
//! * the accumulation tolerance (finding 19's reserved id): nothing is
//!   tolerated, so nothing can accumulate.
//! * `the_diff_cap_says_when_it_hid_something` became
//!   `the_line_diff_says_when_it_hid_something`, on the v2 line diff.
//!
//! # Data
//!
//! Everything is generated in code. The legacy origin is a seeded synthetic
//! 0.6.0 survey written at run time; no fixture is read, copied or derived
//! from.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: see docs/linting.md for the per-test-module expect idiom"
)]
#![expect(
    clippy::print_stdout,
    reason = "a fuzzer's per-seed summary IS its output: the move, oracle and coverage counts \
              are what a reviewer reads to see the gate is still gating"
)]
#![expect(
    clippy::panic,
    reason = "the fuzzer's failure payload (seed, depth, move log, first differing lines) is \
              far more useful than any assert! message, and a panic IS how a test fails"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use tempfile::TempDir;

use super::AppState;
use crate::core::{Command, CommandResult, ReversedLine, UndoHistoryItem};
use crate::models::config::ClosureAdjustmentConfig;
use crate::models::survey::changeset::testkit;
use crate::models::survey::{Counters, Project};
use crate::models::{
    ClosureStrategy, Coordinates, GpsCoordinates, LineId, LineUpdates, ShotData, ShotId, ShotKind,
    ShotUpdates, StationId, StationRef, SurveyDirection,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};
// CONFIG ISOLATION. `AppState::new()` loads the global config and every save
// reaches `note_recent_project` -> `Config::save()`. Config resolution is
// per-thread (`crate::test_helpers`), so every test opens with this guard and
// each sweep worker resolves to a fresh dir of its own.
use crate::test_helpers::isolated_config;

// The coverage knobs' engines, split out of this file by concern. Children of
// this module, so they see its private model directly.
#[path = "fuzz_corrupt_tests.rs"]
mod fuzz_corrupt_tests;
#[path = "fuzz_extended_tests.rs"]
mod fuzz_extended_tests;
#[path = "fuzz_reload_tests.rs"]
mod fuzz_reload_tests;

use fuzz_extended_tests::ExtOp;

// ===========================================================================
// Deterministic PRNG
// ===========================================================================

/// mulberry32, integer-only - the generator the TypeScript harness uses, so
/// the two tiers share a mental model. Every decision is integer arithmetic;
/// `f64` values are generated as data and never used to branch, which keeps
/// a run bit-identical across platforms and optimization levels.
pub(super) struct Rng {
    state: u32,
}

impl Rng {
    pub(super) const fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    fn next_u32(&mut self) -> u32 {
        self.state = self.state.wrapping_add(0x6d2b_79f5);
        let mut t = self.state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 0b11_1101));
        t ^ (t >> 14)
    }

    /// Uniform in `0..bound` (`bound` > 0); 0 for an empty range.
    pub(super) fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        (self.next_u32() as usize) % bound
    }

    /// Inclusive integer range.
    pub(super) fn range(&mut self, min: i64, max: i64) -> i64 {
        if max <= min {
            return min;
        }
        let span = (max - min + 1) as usize;
        min + self.below(span) as i64
    }

    /// True with probability `percent`/100.
    pub(super) fn chance(&mut self, percent: u32) -> bool {
        (self.next_u32() % 100) < percent
    }

    /// A float in `[min, max]`, quantized to 1/1000 so the value is a
    /// function of an INTEGER draw.
    pub(super) fn float(&mut self, min: f64, max: f64) -> f64 {
        let steps = self.range(0, 1000) as f64;
        min + (max - min) * (steps / 1000.0)
    }

    pub(super) fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// The whole state, carried on a recorded move so a replay can put the
    /// generator back where the recorded run had it.
    pub(super) const fn state(&self) -> u32 {
        self.state
    }

    pub(super) const fn restore(&mut self, state: u32) {
        self.state = state;
    }
}

/// Largest magnitude any generated value that feeds coordinate arithmetic may
/// take (an INPUT bound, not an oracle tolerance: #413 finding 9).
pub(super) const GEOMETRY_EXTREME: f64 = 1e3;

pub(super) const COLORS: &[&str] = &["#3388FF", "#FF8833", "#33CC66", "#CC33AA", "#FFCC00"];

// ===========================================================================
// The state oracles
// ===========================================================================

/// The survey as the format writes it, with what a step may legitimately
/// leave different masked: `modified_at` (every step stamps it) and the
/// counters, set to `counters` (the caller checks they did not go down).
pub(super) fn canonical_bytes(project: &Project, counters: Counters) -> Vec<u8> {
    let mut masked = project.clone();
    masked.modified_at = masked.created_at;
    masked.counters = counters;
    masked.file_path = None;
    stalx_v2::encode(&masked, &Extras::default(), Mode::Normal)
        .unwrap_or_else(|e| panic!("a survey that validates must encode: {e}"))
}

/// The derived views of `project`, solved fresh: the raw layout and the
/// Computed View, as their `Debug` text (shortest round-trip floats, so equal
/// text is equal bits).
pub(super) fn derived_text(project: &Project, config: &ClosureAdjustmentConfig) -> String {
    let raw = project.raw_positions();
    let adjusted = project.adjusted_positions_from(&raw, config);
    format!("{raw:?}\n{adjusted:?}")
}

/// The same views through the app's memo (per revision).
fn memo_derived_text(state: &AppState) -> Option<String> {
    let raw = {
        let guard = state.project.lock().ok()?;
        let project = guard.as_ref()?;
        state.raw_positions_of(project)
    };
    let (_, adjusted) = state.adjusted_positions()?;
    Some(format!("{raw:?}\n{adjusted:?}"))
}

/// Where in the v2 file a line sits: the member it belongs to, and for the
/// `lines` member whether it is a line header or a shot row.
fn member_of(lines: &[&str], at: usize) -> String {
    let mut member = "<prefix>".to_string();
    for line in lines.iter().take(at + 1) {
        if let Some(rest) = line.strip_prefix('"') {
            if let Some((key, _)) = rest.split_once('"') {
                member = key.to_string();
            }
        }
    }
    let row = lines.get(at).copied().unwrap_or_default();
    if member == "lines" && row.starts_with("[\"") {
        "lines.shot".to_string()
    } else if member == "lines" && row.starts_with('{') {
        "lines.header".to_string()
    } else {
        member
    }
}

/// The first `max` lines on which two v2 texts differ (the format is one
/// record per line), and whether more differences were left unreported.
pub(super) fn line_diff(expected: &[u8], actual: &[u8], max: usize) -> (Vec<String>, bool) {
    let expected = String::from_utf8_lossy(expected);
    let actual = String::from_utf8_lossy(actual);
    let left: Vec<&str> = expected.lines().collect();
    let right: Vec<&str> = actual.lines().collect();
    let mut out = Vec::new();
    let mut hidden = false;
    for at in 0..left.len().max(right.len()) {
        let (a, b) = (left.get(at).copied(), right.get(at).copied());
        if a == b {
            continue;
        }
        if out.len() == max {
            hidden = true;
            break;
        }
        out.push(format!(
            "at {} (line {at}):\n      recorded {}\n      live     {}",
            member_of(&left, at),
            a.unwrap_or("<absent>"),
            b.unwrap_or("<absent>")
        ));
    }
    (out, hidden)
}

/// What the fuzzer records for one undo depth.
#[derive(Clone)]
pub(super) struct Depth {
    /// The survey there, as the app held it when the depth was recorded.
    pub(super) project: Project,
    /// `canonical_bytes(project, project.counters)`.
    pub(super) bytes: Vec<u8>,
    /// `derived_text` there.
    pub(super) derived: String,
    /// The session ids there, valid while `ids_gen` is the run's current one.
    pub(super) ids: Vec<(LineId, Vec<ShotId>)>,
    pub(super) ids_gen: u64,
    /// The history item of the entry INTO this depth, index normalized
    /// (empty at the depth a run or a dropped history starts at).
    pub(super) item: String,
    /// That entry was captured in this session, so it carries session ids.
    pub(super) session: bool,
}

/// One history item, as the frontend receives it, with its position (which
/// legitimately moves as the stacks move) normalized.
pub(super) fn item_text(item: &UndoHistoryItem) -> String {
    let mut item = item.clone();
    item.index = 0;
    serde_json::to_string(&item).unwrap_or_else(|e| format!("<unserializable: {e}>"))
}

// ===========================================================================
// Synthetic starting project
// ===========================================================================

/// A valid, unremarkable shot. Every value comes from an integer draw; the
/// uuid is minted by the app, as for every user-created shot.
pub(super) fn random_shot(rng: &mut Rng) -> ShotData {
    let azimuth = rng.float(0.0, 359.9);
    let depth_in = rng.float(0.0, 30.0);
    ShotData {
        uuid: None,
        azimuth_in: azimuth,
        azimuth_out: (azimuth + rng.float(-1.5, 1.5) + 360.0) % 360.0,
        length: rng.float(1.0, 12.0),
        depth_in,
        depth_out: (depth_in + rng.float(-3.0, 3.0)).max(0.0),
        pitch_in: rng.chance(40).then(|| rng.float(-60.0, 60.0)),
        pitch_out: rng.chance(40).then(|| rng.float(-60.0, 60.0)),
        left: rng.chance(70).then(|| rng.float(0.0, 5.0)),
        right: rng.chance(70).then(|| rng.float(0.0, 5.0)),
        up: rng.chance(70).then(|| rng.float(0.0, 5.0)),
        down: rng.chance(70).then(|| rng.float(0.0, 5.0)),
        comment: rng
            .chance(20)
            .then(|| format!("shot-{}", rng.range(0, 9999))),
        kind: *rng.pick(&[
            ShotKind::Normal,
            ShotKind::Normal,
            ShotKind::Jump,
            ShotKind::Reference,
        ]),
        marker_index: rng.chance(15).then(|| rng.range(0, 9) as u32),
        temperature: rng.chance(30).then(|| rng.float(4.0, 30.0)),
        credit: None,
    }
}

fn expect_ok(what: &str, result: &CommandResult) {
    assert!(result.success, "setup {what} failed: {:?}", result.error);
}

/// A starting project through the app's own commands: lines of varied length
/// INCLUDING empty and single-shot ones, a datum, station comments, lines
/// with null metadata and, with `closures`, junctions and a genuine loop.
pub(super) fn build_project(state: &AppState, rng: &mut Rng, closures: bool) -> Vec<LineId> {
    expect_ok(
        "CreateProject",
        &state.execute(Command::CreateProject {
            name: "RustFuzz".to_string(),
            location: None,
        }),
    );
    let shape: &[usize] = &[3, 0, 1, 4, 2, 1, 3];
    let mut ids = Vec::new();
    for (index, count) in shape.iter().enumerate() {
        let shots: Vec<ShotData> = (0..*count).map(|_| random_shot(rng)).collect();
        let null_metadata = index % 3 == 0;
        let result = state.execute(Command::CreateLine {
            name: (!null_metadata).then(|| format!("sec-{index}")),
            surveyor: (!null_metadata).then(|| format!("surveyor-{}", rng.range(1, 9))),
            date: None,
            color: (*rng.pick(COLORS)).to_string(),
            direction: Some(if rng.chance(50) {
                SurveyDirection::In
            } else {
                SurveyDirection::Out
            }),
            shots,
            cave_section_id: None,
        });
        expect_ok("CreateLine", &result);
        ids.push(result.line_id.expect("CreateLine returned no id"));
    }
    expect_ok(
        "SetZeroDatum",
        &state.execute(Command::SetZeroDatum {
            station_ref: StationRef::new(ids[0], 0),
            coordinates: GpsCoordinates::new(20.4, -87.3, 0.0).unwrap(),
            accuracy_m: None,
            entrance: None,
        }),
    );
    for index in [0_usize, 5] {
        expect_ok(
            "SetStationComment",
            &state.execute(Command::SetStationComment {
                station_ref: StationRef::new(ids[index], 0),
                comment: Some(format!("station-note-{index}")),
            }),
        );
    }
    // One junction in every starting project: line 6 starts on line 3's
    // middle station, so detach and unmerge have something to act on.
    expect_ok(
        "MergeStations",
        &state.execute(Command::MergeStations {
            station_a: StationRef::new(ids[3], 2),
            station_b: StationRef::new(ids[6], 0),
        }),
    );
    if !closures {
        return ids;
    }
    // A - bridge - B chain on junctions plus a closure straight from A to B:
    // a genuine loop with two independent paths.
    for (left, right) in [(0_usize, 3_usize), (3, 4)] {
        expect_ok(
            "MergeStations",
            &state.execute(Command::MergeStations {
                station_a: StationRef::new(ids[left], shape[left] as u32),
                station_b: StationRef::new(ids[right], 0),
            }),
        );
    }
    expect_ok(
        "CreateLoopClosure",
        &state.execute(Command::CreateLoopClosure {
            station_a: StationRef::new(ids[0], 0),
            station_b: StationRef::new(ids[4], shape[4] as u32),
            closure_strategy: ClosureStrategy::Move,
        }),
    );
    ids
}

// ===========================================================================
// Live-state targeting
// ===========================================================================

/// Everything the generators pick from, read out of the live survey in a
/// deterministic order.
pub(super) struct Targets {
    /// `(line id, its shots' session ids in chain order)`, ascending line id.
    pub(super) lines: Vec<(LineId, Vec<ShotId>)>,
    /// `(line, shot id, uuid)` for every shot, sorted.
    pub(super) shot_uuids: Vec<(LineId, ShotId, String)>,
    /// Every slot: `(line, index, station)`, ascending line id then index.
    pub(super) slots: Vec<(LineId, u32, StationId)>,
    /// Stations on two or more lines, ascending, with their slots.
    pub(super) junctions: Vec<(StationId, Vec<(LineId, u32)>)>,
    /// Datum stations with their `via` line, in list order.
    pub(super) datums: Vec<(StationId, Option<LineId>)>,
    /// Closure ids, ascending.
    pub(super) closures: Vec<u64>,
    /// Section ids, in panel order.
    pub(super) sections: Vec<u32>,
    /// Stations carrying a comment, ascending.
    pub(super) commented: Vec<StationId>,
    /// The import hashes the project tracks.
    pub(super) file_hashes: Vec<String>,
    pub(super) line_hashes: Vec<String>,
    /// Each station's connected component (closures do not connect).
    pub(super) component: BTreeMap<StationId, usize>,
    /// The components holding a datum.
    pub(super) held: BTreeSet<usize>,
}

impl Targets {
    pub(super) fn of(project: &Project) -> Self {
        let mut lines: Vec<(LineId, Vec<ShotId>)> = project
            .lines()
            .iter()
            .map(|line| (line.id, line.shots.iter().map(|shot| shot.id).collect()))
            .collect();
        lines.sort_by_key(|entry| entry.0);
        let mut shot_uuids: Vec<(LineId, ShotId, String)> = project
            .lines()
            .iter()
            .flat_map(|line| {
                line.shots
                    .iter()
                    .map(move |shot| (line.id, shot.id, shot.uuid.clone()))
            })
            .collect();
        shot_uuids.sort();
        let mut slots = Vec::new();
        for (line_id, _) in &lines {
            if let Some(line) = project.line(*line_id) {
                for (index, station) in line.stations().enumerate() {
                    slots.push((*line_id, index as u32, station));
                }
            }
        }
        let mut by_station: BTreeMap<StationId, Vec<(LineId, u32)>> = BTreeMap::new();
        for &(line, index, station) in &slots {
            by_station.entry(station).or_default().push((line, index));
        }
        let junctions = by_station
            .into_iter()
            .filter(|(_, at)| {
                at.iter()
                    .map(|(line, _)| line)
                    .collect::<BTreeSet<_>>()
                    .len()
                    > 1
            })
            .collect();
        let mut closures: Vec<u64> = project.closures.iter().map(|c| c.id).collect();
        closures.sort_unstable();
        let commented = project
            .stations
            .values()
            .filter(|station| station.comment.is_some())
            .map(|station| station.id)
            .collect();
        let topology = project.topology();
        let component: BTreeMap<StationId, usize> = slots
            .iter()
            .filter_map(|&(_, _, station)| Some((station, topology.component_of(station)?)))
            .collect();
        let held = project
            .datums
            .iter()
            .filter_map(|d| component.get(&d.station).copied())
            .collect();
        Self {
            lines,
            shot_uuids,
            slots,
            junctions,
            datums: project.datums.iter().map(|d| (d.station, d.via)).collect(),
            closures,
            sections: project.sections.iter().map(|s| s.id).collect(),
            commented,
            file_hashes: project.imported_file_hashes.clone(),
            line_hashes: project.imported_line_hashes.clone(),
            component,
            held,
        }
    }

    /// The uuid of the live shot `line_id`/`shot_id`.
    pub(super) fn uuid_of(&self, line_id: LineId, shot_id: ShotId) -> Option<String> {
        self.shot_uuids
            .iter()
            .find(|(line, shot, _)| *line == line_id && *shot == shot_id)
            .map(|(_, _, uuid)| uuid.clone())
    }

    /// Which live shot carries `uuid`.
    pub(super) fn by_uuid(&self, uuid: &str) -> Option<(LineId, ShotId)> {
        self.shot_uuids
            .iter()
            .find(|(_, _, candidate)| candidate == uuid)
            .map(|(line, shot, _)| (*line, *shot))
    }

    /// One line's shot ids in chain order, or `None` when the line is gone.
    pub(super) fn shots_of(&self, line_id: LineId) -> Option<&[ShotId]> {
        self.lines
            .iter()
            .find(|(id, _)| *id == line_id)
            .map(|(_, shots)| shots.as_slice())
    }

    /// The slot of `station` on `line`, else the station's first slot.
    pub(super) fn slot(&self, station: StationId, line: LineId) -> Option<StationRef> {
        self.slots
            .iter()
            .find(|(l, _, s)| *l == line && *s == station)
            .or_else(|| self.slots.iter().find(|(_, _, s)| *s == station))
            .map(|(l, index, _)| StationRef::new(*l, *index))
    }

    /// A shot as a trace reference.
    pub(super) fn shot_ref(&self, line_id: LineId, shot_id: ShotId) -> ShotRefT {
        ShotRefT {
            line: line_id,
            id: shot_id,
            uuid: self.uuid_of(line_id, shot_id).unwrap_or_default(),
        }
    }

    /// The slot `slots[index]` as a trace reference.
    pub(super) fn station_at(&self, index: usize) -> StationT {
        let (line, _, station) = self.slots[index];
        StationT { station, line }
    }
}

pub(super) fn read_targets(state: &AppState) -> Targets {
    Targets::of(&state.get_project_state().expect("no project open"))
}

pub(super) fn random_offset(rng: &mut Rng) -> Coordinates {
    let extreme = |rng: &mut Rng| *rng.pick(&[0.0, GEOMETRY_EXTREME, -GEOMETRY_EXTREME, 1e-9]);
    if rng.chance(15) {
        Coordinates::new(extreme(rng), extreme(rng), extreme(rng))
    } else {
        Coordinates::new(
            rng.float(-3.0, 3.0),
            rng.float(-3.0, 3.0),
            rng.float(-1.5, 1.5),
        )
    }
}

// ===========================================================================
// The replayable move trace
// ===========================================================================

/// A shot, named by its `Shot::uuid` (persisted, project-unique), with the
/// session id it had when drawn for the log.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct ShotRefT {
    pub(super) line: LineId,
    pub(super) id: ShotId,
    pub(super) uuid: String,
}

/// A station, by its `StationId`, and the line whose slot was picked: a
/// command addresses that slot, or the station's first slot once the station
/// has left that line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct StationT {
    pub(super) station: StationId,
    pub(super) line: LineId,
}

impl StationT {
    pub(super) fn describe(self) -> String {
        format!("{}@{}", self.station, self.line)
    }
}

/// One mutation, as a value: everything a generator drew, every reference
/// symbolic. [`Fuzzer::apply_op`] is the only thing that executes one, for a
/// drawn move and a replayed one alike.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) enum Op {
    CreateLine {
        name: Option<String>,
        surveyor: Option<String>,
        color: String,
        direction: Option<SurveyDirection>,
        shots: Vec<ShotData>,
        /// Joins the Section at this position (modulo the count) when set.
        section: Option<u32>,
    },
    CreateShot {
        line: LineId,
        shot: Box<ShotData>,
        after: Option<ShotRefT>,
        before: Option<ShotRefT>,
    },
    UpdateShot {
        shot: ShotRefT,
        updates: Box<ShotUpdates>,
    },
    SetShotKind {
        shot: ShotRefT,
        kind: ShotKind,
    },
    UpdateLine {
        line: LineId,
        updates: Box<LineUpdates>,
    },
    DeleteShot {
        shot: ShotRefT,
    },
    DeleteLine {
        line: LineId,
    },
    ReverseLine {
        line: LineId,
        swap_left_right: bool,
    },
    Move {
        line: LineId,
        offset: Coordinates,
        segment: bool,
    },
    MoveStation {
        station: StationT,
        offset: Coordinates,
    },
    StationComment {
        station: StationT,
        comment: Option<String>,
    },
    Merge {
        a: StationT,
        b: StationT,
    },
    /// `UnmergeStationPair`: detaches `b`'s line from the junction.
    Detach {
        a: StationT,
        b: StationT,
    },
    CreateClosure {
        a: StationT,
        b: StationT,
        strategy: ClosureStrategy,
    },
    RemoveClosure {
        closure: u64,
    },
    SetZeroDatum {
        station: StationT,
        coordinates: GpsCoordinates,
        /// Millimetres, an integer draw.
        accuracy_mm: Option<u32>,
        entrance: Option<bool>,
    },
    RemoveZeroDatum {
        station: StationT,
    },
    SplitShotAtDistance {
        shot: ShotRefT,
        distance: f64,
    },
    DeleteShotAndSplit {
        shot: ShotRefT,
        /// The two halves are named `split-<names>a` / `-b`.
        names: u32,
    },
    /// Split Line Here at the shot's end station, or `distance` along it.
    SplitLine {
        shot: ShotRefT,
        along: bool,
        distance: f64,
        side: u8,
        names: u32,
    },
    /// Combine Lines at a line end: the `pick`-th candidate there (modulo
    /// their count), keeping its `b` line when `keep_b`. A blocked candidate
    /// still runs, so the refusal is exercised too.
    CombineLines {
        station: StationT,
        candidate: u8,
        keep_b: bool,
        swap_lr: bool,
        pick: u8,
    },
    DeleteLastShotAndLine {
        shot: ShotRefT,
    },
    DeleteShotRange {
        line: LineId,
        from: ShotRefT,
        to: ShotRefT,
        split: bool,
        names: u32,
    },
    Batch {
        commands: Vec<BatchOp>,
        poisoned: bool,
    },
    /// One of the rarer actions - see `fuzz_extended_tests.rs`.
    Extended(Box<ExtOp>),
}

/// The sub-commands a batch carries, each with its own references.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) enum BatchOp {
    UpdateShotLength {
        shot: ShotRefT,
        length: f64,
    },
    MoveLine {
        line: LineId,
        offset: Coordinates,
    },
    MoveSegment {
        line: LineId,
        offset: Coordinates,
    },
    MoveStation {
        station: StationT,
        offset: Coordinates,
    },
    ReverseLine {
        line: LineId,
        swap_left_right: bool,
    },
    RenameLine {
        line: LineId,
        name: u32,
    },
    RenameStation {
        station: StationT,
        name: u32,
    },
    /// A command the batch allowlist refuses: the whole batch must refuse.
    Disallowed {
        shot: ShotRefT,
    },
}

impl Op {
    /// The command family, for the coverage table.
    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::CreateLine { .. } => "create_line",
            Self::CreateShot { .. } => "create_shot",
            Self::UpdateShot { .. } => "update_shot",
            Self::SetShotKind { .. } => "set_shot_kind",
            Self::UpdateLine { .. } => "update_line",
            Self::DeleteShot { .. } => "delete_shot",
            Self::DeleteLine { .. } => "delete_line",
            Self::ReverseLine { .. } => "reverse_line",
            Self::Move { segment: true, .. } => "move_segment",
            Self::Move { .. } => "move_line",
            Self::MoveStation { .. } => "move_station",
            Self::StationComment { .. } => "station_comment",
            Self::Merge { .. } => "merge_stations",
            Self::Detach { .. } => "detach_pair",
            Self::CreateClosure { .. } => "create_closure",
            Self::RemoveClosure { .. } => "remove_closure",
            Self::SetZeroDatum { .. } => "set_datum",
            Self::RemoveZeroDatum { .. } => "remove_datum",
            Self::SplitShotAtDistance { .. } => "split_shot_at_distance",
            Self::DeleteShotAndSplit { .. } => "delete_shot_and_split",
            Self::SplitLine { .. } => "split_line",
            Self::CombineLines { .. } => "combine_lines",
            Self::DeleteLastShotAndLine { .. } => "delete_last_shot_and_line",
            Self::DeleteShotRange { split: true, .. } => "delete_range_split",
            Self::DeleteShotRange { .. } => "delete_range_shift",
            Self::Batch { .. } => "batch",
            Self::Extended(ext) => ext.kind(),
        }
    }

    /// One line, readable, naming the RECORDED ids - the replay string a
    /// minimized trace prints.
    pub(super) fn describe(&self) -> String {
        let shot = |s: &ShotRefT| format!("{}:{}", s.line, s.id);
        match self {
            Self::CreateLine {
                name,
                shots,
                section,
                ..
            } => format!(
                "createLine(shots={},nullMeta={},section={section:?})",
                shots.len(),
                name.is_none()
            ),
            Self::CreateShot {
                line,
                after,
                before,
                ..
            } => {
                let how = match (after, before) {
                    (Some(anchor), _) => format!("after {}", shot(anchor)),
                    (None, Some(anchor)) => format!("before {}", shot(anchor)),
                    (None, None) => "append".to_string(),
                };
                format!("createShot(s={line},{how})")
            }
            Self::UpdateShot { shot: target, .. } => format!("updateShot({})", shot(target)),
            Self::SetShotKind { shot: target, kind } => {
                format!("setShotKind({},{kind:?})", shot(target))
            }
            Self::UpdateLine { line, .. } => format!("updateLine(s={line})"),
            Self::DeleteShot { shot: target } => format!("deleteShot({})", shot(target)),
            Self::DeleteLine { line } => format!("deleteLine(s={line})"),
            Self::ReverseLine {
                line,
                swap_left_right,
            } => format!("reverseLine(s={line},swapLR={swap_left_right})"),
            Self::Move { line, segment, .. } => format!(
                "{}(s={line})",
                if *segment { "moveSegment" } else { "moveLine" }
            ),
            Self::MoveStation { station, .. } => format!("moveStation({})", station.describe()),
            Self::StationComment { station, comment } => format!(
                "stationComment({},{})",
                station.describe(),
                comment.is_some()
            ),
            Self::Merge { a, b } => format!("mergeStations({},{})", a.describe(), b.describe()),
            Self::Detach { a, b } => format!("detachPair({},{})", a.describe(), b.describe()),
            Self::CreateClosure { a, b, strategy } => format!(
                "createLoopClosure({},{},{strategy:?})",
                a.describe(),
                b.describe()
            ),
            Self::RemoveClosure { closure } => format!("removeLoopClosure({closure})"),
            Self::SetZeroDatum {
                station, entrance, ..
            } => format!("setZeroDatum({},entrance={entrance:?})", station.describe()),
            Self::RemoveZeroDatum { station } => {
                format!("removeZeroDatum({})", station.describe())
            }
            Self::SplitShotAtDistance {
                shot: target,
                distance,
            } => format!("splitShotAtDistance({},{distance})", shot(target)),
            Self::DeleteShotAndSplit { shot: target, .. } => {
                format!("deleteShotAndSplit({})", shot(target))
            }
            Self::SplitLine {
                shot: target,
                along,
                distance,
                side,
                ..
            } => format!(
                "splitLine({},along={along},{distance},side={side})",
                shot(target)
            ),
            Self::CombineLines {
                station,
                candidate,
                keep_b,
                swap_lr,
                pick,
            } => format!(
                "combineLines({},candidate={candidate},keep_b={keep_b},swap_lr={swap_lr},pick={pick})",
                station.describe()
            ),
            Self::DeleteLastShotAndLine { shot: target } => {
                format!("deleteLastShotAndLine({})", shot(target))
            }
            Self::DeleteShotRange {
                line,
                from,
                to,
                split,
                ..
            } => format!(
                "{}(s={line},{}..{})",
                if *split {
                    "deleteShotRangeSplit"
                } else {
                    "deleteShotRangeShift"
                },
                from.id,
                to.id
            ),
            Self::Batch { commands, poisoned } => {
                format!("batch(n={},poisoned={poisoned})", commands.len())
            }
            Self::Extended(ext) => ext.describe(),
        }
    }
}

/// Everything one move brought into existence, in project order.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct Created {
    pub(super) lines: Vec<LineId>,
    pub(super) shots: Vec<String>,
    #[serde(default)]
    pub(super) stations: Vec<StationId>,
    pub(super) closures: Vec<u64>,
}

/// One move of a recorded run. The prologues and the closing walk are
/// functions of the `Plan`, so a replay re-runs them and a shrink step can
/// never delete them.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct TraceMove {
    pub(super) kind: MoveKind,
    pub(super) created: Created,
    /// The generator's state immediately before this move.
    pub(super) rng: u32,
    /// The coverage stream's state, when a knob is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) aux: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) enum MoveKind {
    Mutate(Box<Op>),
    Undo(usize),
    Redo(usize),
    Cross,
    Save,
}

impl TraceMove {
    pub(super) fn describe(&self) -> String {
        match &self.kind {
            MoveKind::Mutate(op) => op.describe(),
            MoveKind::Undo(steps) => format!("undo x{steps}"),
            MoveKind::Redo(steps) => format!("redo x{steps}"),
            MoveKind::Cross => "save/close/reopen".to_string(),
            MoveKind::Save => "save in place".to_string(),
        }
    }
}

/// Every line with its shots' uuids, every station and every closure, in
/// project order. Taken before and after a move: the difference is what it
/// created.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct Inventory {
    pub(super) lines: Vec<(LineId, Vec<String>)>,
    #[serde(default)]
    pub(super) stations: Vec<StationId>,
    pub(super) closures: Vec<u64>,
}

impl Inventory {
    pub(super) fn of(project: &Project) -> Self {
        Self {
            lines: project
                .lines()
                .iter()
                .map(|line| {
                    (
                        line.id,
                        line.shots.iter().map(|shot| shot.uuid.clone()).collect(),
                    )
                })
                .collect(),
            stations: project.stations.keys().copied().collect(),
            closures: project.closures.iter().map(|c| c.id).collect(),
        }
    }

    fn shots(&self) -> Vec<&String> {
        self.lines
            .iter()
            .flat_map(|(_, uuids)| uuids.iter())
            .collect()
    }

    pub(super) fn created_since(&self, before: &Self) -> Created {
        let old_lines: BTreeSet<LineId> = before.lines.iter().map(|(id, _)| *id).collect();
        let old_shots: BTreeSet<&String> = before.shots().into_iter().collect();
        let old_stations: BTreeSet<StationId> = before.stations.iter().copied().collect();
        let old_closures: BTreeSet<u64> = before.closures.iter().copied().collect();
        Created {
            lines: self
                .lines
                .iter()
                .map(|(id, _)| *id)
                .filter(|id| !old_lines.contains(id))
                .collect(),
            shots: self
                .shots()
                .into_iter()
                .filter(|uuid| !old_shots.contains(*uuid))
                .cloned()
                .collect(),
            stations: self
                .stations
                .iter()
                .copied()
                .filter(|id| !old_stations.contains(id))
                .collect(),
            closures: self
                .closures
                .iter()
                .copied()
                .filter(|id| !old_closures.contains(id))
                .collect(),
        }
    }
}

/// What a recorded reference means in THIS run: the identity for a drawn run
/// (nothing is ever skipped), a pairing for a replay, seeded from the two
/// starting states and extended by each move's [`Created`]. A reference the
/// pairing cannot resolve makes its move inapplicable: skipped, not fatal.
pub(super) struct Refs {
    identity: bool,
    /// Pair a kind only when both sides created the same number of it (a
    /// replay of a plan whose moves create any number of entities).
    checked: bool,
    lines: BTreeMap<LineId, LineId>,
    shots: BTreeMap<String, String>,
    stations: BTreeMap<StationId, StationId>,
    closures: BTreeMap<u64, u64>,
    skipped: Vec<String>,
}

/// Pair the unknown recorded keys with the fresh live values, in order.
fn pair_fresh<K: Ord + Clone, V: PartialEq + Clone>(
    map: &mut BTreeMap<K, V>,
    recorded: &[K],
    live: &[V],
) {
    let fresh: Vec<V> = live
        .iter()
        .filter(|value| !map.values().any(|known| known == *value))
        .cloned()
        .collect();
    let unknown: Vec<K> = recorded
        .iter()
        .filter(|key| !map.contains_key(*key))
        .cloned()
        .collect();
    for (key, value) in unknown.into_iter().zip(fresh) {
        map.insert(key, value);
    }
}

/// `(unknown recorded, fresh live)` for one kind.
fn arity<K: Ord, V: PartialEq>(map: &BTreeMap<K, V>, recorded: &[K], live: &[V]) -> (usize, usize) {
    (
        recorded
            .iter()
            .filter(|key| !map.contains_key(*key))
            .count(),
        live.iter()
            .filter(|value| !map.values().any(|known| known == *value))
            .count(),
    )
}

impl Refs {
    pub(super) fn identity() -> Self {
        Self {
            identity: true,
            checked: false,
            lines: BTreeMap::new(),
            shots: BTreeMap::new(),
            stations: BTreeMap::new(),
            closures: BTreeMap::new(),
            skipped: Vec::new(),
        }
    }

    /// A replay's resolver: the trace's starting state paired, position by
    /// position, with this run's. Checked: a mismatch means the trace was
    /// recorded under another plan.
    pub(super) fn replay(recorded: &Inventory, live: &Inventory) -> Result<Self, String> {
        if recorded.lines.len() != live.lines.len()
            || recorded.stations.len() != live.stations.len()
            || recorded.closures.len() != live.closures.len()
        {
            return Err(format!(
                "the trace starts with {}/{}/{} line(s)/station(s)/closure(s); this run with \
                 {}/{}/{} - the plan does not match the trace",
                recorded.lines.len(),
                recorded.stations.len(),
                recorded.closures.len(),
                live.lines.len(),
                live.stations.len(),
                live.closures.len()
            ));
        }
        let mut refs = Self {
            identity: false,
            ..Self::identity()
        };
        for ((recorded_id, recorded_shots), (live_id, live_shots)) in
            recorded.lines.iter().zip(&live.lines)
        {
            if recorded_shots.len() != live_shots.len() {
                return Err(format!(
                    "the trace starts with {} shot(s) in line {recorded_id}; this run with {}",
                    recorded_shots.len(),
                    live_shots.len()
                ));
            }
            refs.lines.insert(*recorded_id, *live_id);
            for (a, b) in recorded_shots.iter().zip(live_shots) {
                refs.shots.insert(a.clone(), b.clone());
            }
        }
        for (a, b) in recorded.stations.iter().zip(&live.stations) {
            refs.stations.insert(*a, *b);
        }
        for (a, b) in recorded.closures.iter().zip(&live.closures) {
            refs.closures.insert(*a, *b);
        }
        Ok(refs)
    }

    pub(super) const fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }

    /// Pair what the recorded move created with what the replayed move
    /// created. Only entities neither side already knows are paired: an undo
    /// of a delete brings back what is already mapped.
    pub(super) fn learn(&mut self, recorded: &Created, live: &Created) {
        if self.identity {
            return;
        }
        if self.checked {
            let counts = [
                arity(&self.lines, &recorded.lines, &live.lines),
                arity(&self.shots, &recorded.shots, &live.shots),
                arity(&self.stations, &recorded.stations, &live.stations),
                arity(&self.closures, &recorded.closures, &live.closures),
            ];
            if counts.iter().any(|(a, b)| a != b) {
                self.skipped.push(format!(
                    "a move created {counts:?} (recorded, here) line(s)/shot(s)/station(s)/\
                     closure(s); left unpaired"
                ));
                return;
            }
        }
        pair_fresh(&mut self.lines, &recorded.lines, &live.lines);
        pair_fresh(&mut self.shots, &recorded.shots, &live.shots);
        pair_fresh(&mut self.stations, &recorded.stations, &live.stations);
        pair_fresh(&mut self.closures, &recorded.closures, &live.closures);
    }

    pub(super) fn line(&self, recorded: LineId, targets: &Targets) -> Option<LineId> {
        let live = if self.identity {
            recorded
        } else {
            *self.lines.get(&recorded)?
        };
        targets.shots_of(live).map(|_| live)
    }

    pub(super) fn shot(&self, recorded: &ShotRefT, targets: &Targets) -> Option<(LineId, ShotId)> {
        if self.identity {
            return Some((recorded.line, recorded.id));
        }
        targets.by_uuid(self.shots.get(&recorded.uuid)?)
    }

    /// The slot a recorded station reference addresses now.
    pub(super) fn station(&self, recorded: StationT, targets: &Targets) -> Option<StationRef> {
        let station = if self.identity {
            recorded.station
        } else {
            *self.stations.get(&recorded.station)?
        };
        let line = if self.identity {
            recorded.line
        } else {
            self.lines
                .get(&recorded.line)
                .copied()
                .unwrap_or(LineId::MAX)
        };
        targets.slot(station, line)
    }

    pub(super) fn closure(&self, recorded: u64, targets: &Targets) -> Option<u64> {
        let live = if self.identity {
            recorded
        } else {
            *self.closures.get(&recorded)?
        };
        targets.closures.contains(&live).then_some(live)
    }

    pub(super) fn skipped(&self) -> &[String] {
        &self.skipped
    }

    pub(super) fn skip(&mut self, what: &str) -> Option<Drawn> {
        self.skipped.push(what.to_string());
        None
    }
}

/// A recorded run, and everything a replay of it needs.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct Trace {
    pub(super) seed: u32,
    pub(super) plan: Plan,
    /// The project the trace starts in, after the plan's prologues.
    pub(super) prelude: Inventory,
    pub(super) moves: Vec<TraceMove>,
    /// TEST-ONLY, and `None` for every recorded run - see [`Fault`].
    #[serde(default)]
    pub(super) fault: Option<Fault>,
}

/// A failure INJECTED into a replay, so the shrinker's own gate has a failing
/// run to minimize while the app has none: the replay fails once the project
/// holds `at_lines` lines, a monotone property of the moves a candidate kept.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct Fault {
    pub(super) at_lines: usize,
}

/// The recorder, per thread, so a run that fails and unwinds still leaves
/// its trace behind. Off unless armed: no sweep pays for it.
struct Recording {
    active: bool,
    prelude: Inventory,
    moves: Vec<TraceMove>,
}

thread_local! {
    static RECORDER: std::cell::RefCell<Option<Recording>> =
        const { std::cell::RefCell::new(None) };
}

pub(super) fn recorder_arm() {
    RECORDER.with_borrow_mut(|slot| {
        *slot = Some(Recording {
            active: false,
            prelude: Inventory::default(),
            moves: Vec::new(),
        });
    });
}

fn recorder_armed() -> bool {
    RECORDER.with_borrow(Option::is_some)
}

fn recorder_active() -> bool {
    RECORDER.with_borrow(|slot| slot.as_ref().is_some_and(|recording| recording.active))
}

fn recorder_begin(prelude: Inventory) {
    RECORDER.with_borrow_mut(|slot| {
        if let Some(recording) = slot.as_mut() {
            recording.active = true;
            recording.prelude = prelude;
        }
    });
}

fn recorder_end() {
    RECORDER.with_borrow_mut(|slot| {
        if let Some(recording) = slot.as_mut() {
            recording.active = false;
        }
    });
}

fn recorder_push(mv: TraceMove) {
    RECORDER.with_borrow_mut(|slot| {
        if let Some(recording) = slot.as_mut() {
            if recording.active {
                recording.moves.push(mv);
            }
        }
    });
}

fn recorder_seal(created: Created) {
    RECORDER.with_borrow_mut(|slot| {
        if let Some(recording) = slot.as_mut() {
            if recording.active {
                if let Some(last) = recording.moves.last_mut() {
                    last.created = created;
                }
            }
        }
    });
}

pub(super) fn recorder_take() -> Option<(Inventory, Vec<TraceMove>)> {
    RECORDER.with_borrow_mut(|slot| {
        slot.take()
            .map(|recording| (recording.prelude, recording.moves))
    })
}

// ===========================================================================
// Topology signature, for corpus guidance
// ===========================================================================

/// A count, bucketed, so crossing a bucket means the project acquired a
/// structural possibility rather than one more of something it had.
pub(super) const fn bucket(count: usize) -> u8 {
    match count {
        0 => 0,
        1 => 1,
        2 => 2,
        3..=4 => 3,
        5..=8 => 4,
        9..=16 => 5,
        17..=32 => 6,
        _ => 7,
    }
}

/// The feature letters [`topology_signature`] prints, in its order.
pub(super) const SIGNATURE_FEATURES: &[u8; 12] = b"seohjdctzpka";

/// A compact, deterministic fingerprint of a survey's SHAPE on the station
/// graph: counts and structure only, never a float. Letters, in order: lines,
/// empty lines, one-shot lines, shots, junction stations, most lines at one
/// junction, closures, `Tie` closures (migrated merges), datums, placements
/// (datum-less pieces), commented stations, aliases (merged-away ids).
pub(super) fn topology_signature(state: &AppState) -> String {
    let Some(project) = state.get_project_state() else {
        return "no-project".to_string();
    };
    let topology = project.topology();
    let mut junctions = 0;
    let mut star = 0;
    for station in project.stations.keys() {
        let lines = topology.lines_of(*station).len();
        if lines > 1 {
            junctions += 1;
            star = star.max(lines);
        }
    }
    let lines = project.lines();
    format!(
        "s{}e{}o{}h{}j{}d{}c{}t{}z{}p{}k{}a{}",
        bucket(lines.len()),
        bucket(lines.iter().filter(|l| l.shots.is_empty()).count()),
        bucket(lines.iter().filter(|l| l.shots.len() == 1).count()),
        bucket(lines.iter().map(|l| l.shots.len()).sum()),
        bucket(junctions),
        bucket(star),
        bucket(project.closures.len()),
        bucket(
            project
                .closures
                .iter()
                .filter(|c| c.strategy == crate::models::survey::ClosureStrategy::Tie)
                .count()
        ),
        bucket(project.datums.len()),
        bucket(project.placements.len()),
        bucket(
            project
                .stations
                .values()
                .filter(|s| s.comment.is_some())
                .count()
        ),
        bucket(project.stations.values().map(|s| s.aliases.len()).sum()),
    )
}

/// The topology signature at every prefix of a seed's MUTATE-ONLY walk -
/// the exact trajectory a [`Plan::prologue`] replays, state by state.
pub(super) fn prologue_signatures(seed: u32, closures: bool, moves: usize) -> Vec<String> {
    let mut fuzzer = Fuzzer::new(seed, closures);
    let mut signatures = Vec::with_capacity(moves);
    for _ in 0..moves {
        if fuzzer.stopped() {
            break;
        }
        fuzzer.step_mutate();
        signatures.push(topology_signature(&fuzzer.state));
    }
    signatures
}

// ===========================================================================
// The fuzzer
// ===========================================================================

/// What one command family did over a run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct KindCount {
    /// Drawn and applied (whatever the app answered).
    pub(super) applied: usize,
    /// Recorded an undo entry.
    pub(super) pushed: usize,
    /// Refused by the app.
    pub(super) rejected: usize,
    /// Succeeded without recording anything.
    pub(super) noops: usize,
    /// Inapplicable in a replay (its target was shrunk away).
    pub(super) skipped: usize,
}

/// What the coverage knobs did in one run.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct CoverageCounts {
    pub(super) corrupted: usize,
    pub(super) dropped: usize,
    pub(super) kept_corrupted: usize,
    /// Steps refused through an entry a corruption left fitting nothing.
    pub(super) refused_corrupted: usize,
    pub(super) window_cut: usize,
    pub(super) trimmed_crossings: usize,
    pub(super) saves_in_place: usize,
    pub(super) double_crossings: usize,
    pub(super) extended: usize,
}

/// The largest each unbounded-in-principle collection got during a run.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Peak {
    lines: usize,
    stations: usize,
    closures: usize,
    datums: usize,
    placements: usize,
    sections: usize,
}

/// How a run ended.
#[derive(Debug, Default)]
pub(super) struct Outcome {
    pub(super) mutations: usize,
    pub(super) rejected: usize,
    pub(super) noops: usize,
    pub(super) skipped: usize,
    pub(super) undos: usize,
    pub(super) redos: usize,
    pub(super) jumps: usize,
    pub(super) saves: usize,
    pub(super) reloads: usize,
    pub(super) max_depth: usize,
    /// Undo entries the app FIFO-trimmed out of reach.
    pub(super) trimmed: usize,
    /// Refused steps checked to be strict no-ops (the edges of the window and
    /// the tamper probes).
    pub(super) refusals: usize,
    /// Of those, steps refused through a tampered precondition.
    pub(super) tampered: usize,
    /// Depth comparisons made.
    pub(super) compared: usize,
    /// True when the app panicked inside a move; the run stops there and
    /// fails with the move log.
    pub(super) app_panic: bool,
    /// Wall time in the oracles, microseconds: (state, derived, entries).
    oracle_us: (u128, u128, u128),
    peak: Peak,
    /// The topology signature of the state the run ended in.
    pub(super) signature: String,
    pub(super) config: Option<DrawnConfig>,
    pub(super) knobs: Knobs,
    pub(super) coverage: CoverageCounts,
    /// Per command family.
    pub(super) kinds: BTreeMap<&'static str, KindCount>,
}

/// The part of an [`Outcome`] a replay has to reproduce exactly.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Shape<'a> {
    pub(super) mutations: usize,
    pub(super) rejected: usize,
    pub(super) noops: usize,
    pub(super) undos: usize,
    pub(super) redos: usize,
    pub(super) max_depth: usize,
    pub(super) signature: &'a str,
    pub(super) kinds: &'a BTreeMap<&'static str, KindCount>,
}

impl Outcome {
    pub(super) fn shape(&self) -> Shape<'_> {
        Shape {
            mutations: self.mutations,
            rejected: self.rejected,
            noops: self.noops,
            undos: self.undos,
            redos: self.redos,
            max_depth: self.max_depth,
            signature: &self.signature,
            kinds: &self.kinds,
        }
    }
}

/// What applying one `Op` produced.
pub(super) struct Drawn {
    pub(super) label: String,
    pub(super) result: CommandResult,
}

impl Drawn {
    pub(super) fn new(label: String, result: CommandResult) -> Self {
        Self { label, result }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum StepOutcome {
    Applied,
    Skipped,
    Stopped,
}

/// The state of the app before a mutation, for the no-op and refusal checks.
struct Before {
    project: Project,
    undo: usize,
    redo: usize,
    newest: Option<String>,
    dirty: bool,
}

pub(super) struct Fuzzer {
    pub(super) state: AppState,
    pub(super) rng: Rng,
    /// The COVERAGE stream, seeded `seed ^ COVERAGE_STREAM`: every draw a
    /// knob makes comes from here, so the primary stream is the same with
    /// every knob on or off.
    pub(super) aux: Rng,
    pub(super) seed: u32,
    /// `depths[d]` is what was recorded at depth `d`; `None` below the floor
    /// (FIFO-trimmed or dropped: unreachable for good).
    pub(super) depths: Vec<Option<Depth>>,
    pub(super) cursor: usize,
    pub(super) top: usize,
    /// The lowest depth the history can still reach.
    pub(super) floor: usize,
    /// Depths whose entry INTO them a reopen kept although it fits nothing
    /// (`FORMAT_CORRUPT`'s `line-hash`): a step through it must refuse.
    pub(super) walls: BTreeSet<usize>,
    /// The app's own count limit, read from the app.
    pub(super) history_limit: usize,
    /// The app's `undo_persist_limit`, read from the app.
    pub(super) persist_limit: usize,
    drawn: Option<DrawnConfig>,
    /// The session-id generation: bumped whenever ids are re-minted (a
    /// reopen, a step through a decoded entry). A depth's ids are compared
    /// exactly only within the generation they were recorded in.
    pub(super) id_gen: u64,
    /// A step through a DECODED entry (no session ids) happened since the
    /// last reopen. It minted ids for the shots it re-created; an in-session
    /// entry stepped after that gives the lines it touches the ids it was
    /// captured with, so the ids of a depth are a function of the path, not
    /// of the depth, and only soundness is checked until the next reopen
    /// (where the loader assigns every id afresh).
    pub(super) id_taint: bool,
    last_counters: Option<Counters>,
    adjust: ClosureAdjustmentConfig,
    log: Vec<String>,
    /// The move running right now, so a move that never returns is the one
    /// the failure names.
    in_flight: Option<String>,
    pub(super) outcome: Outcome,
    /// Kept alive for the run: dropping it deletes the files.
    pub(super) temp: Option<TempDir>,
    /// Build the orphan-closure shape at the next crossing.
    orphan_pending: bool,
    /// The join a successful Split Line Here just made, so the combine
    /// generator can put it back together.
    pub(super) last_split_join: Option<StationT>,
    pub(super) knobs: Knobs,
}

impl Fuzzer {
    pub(super) fn new(seed: u32, closures: bool) -> Self {
        Self::new_with(seed, closures, None, Knobs::OFF, false)
    }

    /// A run's app, configured the way it drew, holding its starting
    /// project. The history limit is armed through
    /// `UndoStack::set_history_limit` (what a settings change calls), never
    /// by writing the config, which a save would persist.
    pub(super) fn new_with(
        seed: u32,
        closures: bool,
        drawn: Option<DrawnConfig>,
        knobs: Knobs,
        legacy_origin: bool,
    ) -> Self {
        let state = AppState::new().unwrap();
        if let Some(config) = drawn {
            state
                .undo_stack
                .lock()
                .unwrap()
                .set_history_limit(config.history_limit);
        }
        let history_limit = state.undo_stack.lock().unwrap().history_limit();
        let config = state.get_config();
        let mut rng = Rng::new(seed);
        let mut aux = Rng::new(seed ^ COVERAGE_STREAM);
        let mut temp = None;
        if legacy_origin {
            let dir = TempDir::new().unwrap();
            let path = fuzz_extended_tests::legacy_origin_file(seed, dir.path());
            let opened = state.execute(Command::OpenProject { path });
            assert!(
                opened.success,
                "the legacy origin did not open: {:?}",
                opened.error
            );
            temp = Some(dir);
        } else {
            build_project(&state, &mut rng, closures);
        }
        if knobs.large {
            temp = Some(fuzz_extended_tests::grow_large(&state, &mut aux));
        }
        state.undo_stack.lock().unwrap().clear();
        let mut fuzzer = Self {
            state,
            rng,
            aux,
            seed,
            depths: Vec::new(),
            cursor: 0,
            top: 0,
            floor: 0,
            walls: BTreeSet::new(),
            history_limit,
            persist_limit: config.general.undo_persist_limit,
            drawn,
            id_gen: 0,
            id_taint: false,
            last_counters: None,
            adjust: config.closure_adjustment,
            log: Vec::new(),
            in_flight: None,
            outcome: Outcome {
                config: drawn,
                knobs,
                ..Outcome::default()
            },
            temp,
            orphan_pending: false,
            last_split_join: None,
            knobs,
        };
        assert_eq!(fuzzer.undo_len(), 0, "setup left undo entries behind");
        let base = fuzzer.record_depth(String::new(), false);
        fuzzer.depths.push(Some(base));
        fuzzer.check_live("the starting project");
        fuzzer
    }

    pub(super) fn stopped(&self) -> bool {
        self.outcome.app_panic
    }

    pub(super) fn undo_len(&self) -> usize {
        self.state.undo_stack.lock().unwrap().len()
    }

    pub(super) fn redo_len(&self) -> usize {
        self.state.undo_stack.lock().unwrap().redo_len()
    }

    pub(super) fn live(&self) -> Project {
        self.state.get_project_state().expect("a project is open")
    }

    pub(super) fn inventory(&self) -> Inventory {
        self.state
            .get_project_state()
            .map(|project| Inventory::of(&project))
            .unwrap_or_default()
    }

    /// The newest undo entry's history item, the one thing that sees a push
    /// at the history limit (where the push and its trim cancel in a length).
    fn newest_item(&self) -> Option<String> {
        self.state.get_undo_stack().first().map(item_text)
    }

    pub(super) fn note(&mut self, line: String) {
        if fuzz_trace() {
            println!("  [seed {} depth {}] {line}", self.seed, self.cursor);
        }
        self.log.push(line);
    }

    pub(super) fn starting_move(&mut self, label: String) {
        self.in_flight = Some(label);
    }

    pub(super) fn finished_move(&mut self) {
        self.in_flight = None;
    }

    /// Move the in-flight label into the log, marked as never returned.
    fn flush_in_flight(&mut self) {
        if let Some(label) = self.in_flight.take() {
            self.note(format!("{label} -> PANICKED (did not return)"));
        }
    }

    pub(super) fn fail(&self, headline: &str, detail: &str) -> ! {
        let mut tail: Vec<String> = self
            .log
            .iter()
            .rev()
            .take(14)
            .rev()
            .map(|line| format!("    {line}"))
            .collect();
        if let Some(label) = &self.in_flight {
            tail.push(format!("    {label} -> IN FLIGHT (had not returned)"));
        }
        panic!(
            "{headline}\n  seed={} depth={} floor={} top={} saves={} reloads={}{}\n{detail}\n  \
             last {} move(s):\n{}",
            self.seed,
            self.cursor,
            self.floor,
            self.top,
            self.outcome.saves,
            self.outcome.reloads,
            self.drawn.map(DrawnConfig::describe).unwrap_or_default(),
            tail.len(),
            tail.join("\n"),
        );
    }

    /// Run `body`, which drives the app; a panic from inside the app stops
    /// the run (its locks may be poisoned) with the move that panicked named
    /// in the log. `None` when it panicked.
    pub(super) fn catching<T>(&mut self, body: impl FnOnce(&mut Self) -> T) -> Option<T> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(self))) {
            Ok(value) => Some(value),
            Err(payload) => {
                let message = panic_message(&*payload);
                self.flush_in_flight();
                self.note(format!("  ^ the app panicked: {message}"));
                self.outcome.app_panic = true;
                None
            }
        }
    }

    // -----------------------------------------------------------------------
    // The oracles
    // -----------------------------------------------------------------------

    /// What to record for the live state, the entry into it described by
    /// `item`.
    pub(super) fn record_depth(&mut self, item: String, session: bool) -> Depth {
        let started = std::time::Instant::now();
        let project = self.live();
        let bytes = canonical_bytes(&project, project.counters);
        let ids = testkit::session_ids(&project);
        self.outcome.oracle_us.0 += started.elapsed().as_micros();
        let started = std::time::Instant::now();
        let derived = derived_text(&project, &self.adjust);
        self.outcome.oracle_us.1 += started.elapsed().as_micros();
        Depth {
            project,
            bytes,
            derived,
            ids,
            ids_gen: self.id_gen,
            item,
            session,
        }
    }

    pub(super) fn depth(&self, depth: usize) -> &Depth {
        self.depths
            .get(depth)
            .and_then(Option::as_ref)
            .unwrap_or_else(|| self.fail(&format!("depth {depth} has no record"), ""))
    }

    /// The checks every move ends with, whatever it was: the invariants,
    /// sound session ids, counters that never went down, and the derived
    /// memo agreeing with a fresh solve.
    pub(super) fn check_live(&mut self, what: &str) -> Project {
        let project = self.live();
        let violations = project.validate();
        if !violations.is_empty() {
            let shown: Vec<String> = violations
                .iter()
                .take(8)
                .map(|v| format!("  {v}"))
                .collect();
            self.fail(
                &format!("{what} left the survey INVALID"),
                &shown.join("\n"),
            );
        }
        if let Err(e) = testkit::session_ids_sound(&project) {
            self.fail(
                &format!("{what} left unsound session ids"),
                &format!("  {e}"),
            );
        }
        if let Some(last) = self.last_counters {
            let now = project.counters;
            if now.next_station_id < last.next_station_id
                || now.next_closure_id < last.next_closure_id
                || now.next_section_id < last.next_section_id
            {
                self.fail(
                    &format!("{what} moved a counter DOWN"),
                    &format!("  {last:?} -> {now:?}: an id handed out could be handed out again"),
                );
            }
        }
        self.last_counters = Some(project.counters);
        let started = std::time::Instant::now();
        let memo = memo_derived_text(&self.state);
        let fresh = derived_text(&project, &self.adjust);
        self.outcome.oracle_us.1 += started.elapsed().as_micros();
        if memo.as_deref() != Some(fresh.as_str()) {
            self.fail(
                &format!("after {what} the memoised derived views are STALE"),
                &format!(
                    "  memo:  {}\n  fresh: {}",
                    memo.as_deref()
                        .unwrap_or("<none>")
                        .lines()
                        .next()
                        .unwrap_or(""),
                    fresh.lines().next().unwrap_or("")
                ),
            );
        }
        self.record_peak(&project);
        project
    }

    fn record_peak(&mut self, project: &Project) {
        let peak = &mut self.outcome.peak;
        peak.lines = peak.lines.max(project.lines().len());
        peak.stations = peak.stations.max(project.stations.len());
        peak.closures = peak.closures.max(project.closures.len());
        peak.datums = peak.datums.max(project.datums.len());
        peak.placements = peak.placements.max(project.placements.len());
        peak.sections = peak.sections.max(project.sections.len());
    }

    /// The live survey must be the one recorded at `depth`, to the bit.
    /// `exact_ids`: every step that brought the run here carried in-session
    /// ids, so the session ids must be the recorded ones too (when the
    /// record is of this generation); otherwise the record is re-anchored.
    pub(super) fn expect_state(&mut self, depth: usize, what: &str, exact_ids: bool) {
        let project = self.check_live(what);
        let started = std::time::Instant::now();
        let recorded = self.depth(depth).clone();
        let (live_c, rec_c) = (project.counters, recorded.project.counters);
        if live_c.next_station_id < rec_c.next_station_id
            || live_c.next_closure_id < rec_c.next_closure_id
            || live_c.next_section_id < rec_c.next_section_id
        {
            self.fail(
                &format!("{what} did not restore the counters recorded at depth {depth}"),
                &format!("  recorded {rec_c:?}, live {live_c:?}: a counter is BELOW its record"),
            );
        }
        let bytes = canonical_bytes(&project, rec_c);
        if bytes != recorded.bytes {
            let (diffs, hidden) =
                line_diff(&recorded.bytes, &bytes, if fuzz_verbose() { 60 } else { 6 });
            self.fail(
                &format!("{what} did not restore the state recorded at depth {depth}"),
                &format!(
                    "    {}{}",
                    diffs.join("\n    "),
                    if hidden {
                        "\n    (more differences not shown)"
                    } else {
                        ""
                    }
                ),
            );
        }
        // The edit time is the command layer's stamp, not the engine's.
        let mut stamped = project.clone();
        stamped.modified_at = recorded.project.modified_at;
        if let Err(e) = testkit::same_state(&stamped, &recorded.project) {
            self.fail(
                &format!("{what} restored the bytes recorded at depth {depth} but not the model"),
                &format!("  {e}"),
            );
        }
        self.outcome.compared += 1;
        self.outcome.oracle_us.0 += started.elapsed().as_micros();
        let derived = derived_text(&project, &self.adjust);
        if derived != recorded.derived {
            let at = derived
                .bytes()
                .zip(recorded.derived.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or(0);
            let window = |text: &str| {
                text.get(at.saturating_sub(60)..(at + 60).min(text.len()))
                    .unwrap_or_default()
                    .to_string()
            };
            self.fail(
                &format!("{what} restored the survey of depth {depth} but its derived views moved"),
                &format!(
                    "  at byte {at}:\n    recorded ...{}\n    live     ...{}",
                    window(&recorded.derived),
                    window(&derived)
                ),
            );
        }
        let ids = testkit::session_ids(&project);
        if exact_ids && !self.id_taint && recorded.ids_gen == self.id_gen {
            if ids != recorded.ids {
                self.fail(
                    &format!(
                        "{what} did not give the shots their session ids back (depth {depth})"
                    ),
                    &format!("  recorded {:?}\n  live     {ids:?}", recorded.ids),
                );
            }
        } else if let Some(Some(slot)) = self.depths.get_mut(depth) {
            slot.ids = ids;
            slot.ids_gen = self.id_gen;
        }
    }

    /// The stacks hold exactly the window the model expects, and every item
    /// on them is the one recorded when its entry was pushed.
    pub(super) fn expect_stacks(&mut self, what: &str) {
        let started = std::time::Instant::now();
        let undo = self.state.get_undo_stack();
        let redo = self.state.get_redo_stack();
        let (want_undo, want_redo) = (self.cursor - self.floor, self.top - self.cursor);
        if undo.len() != want_undo || redo.len() != want_redo {
            self.fail(
                &format!("after {what} the stacks do not hold the expected window"),
                &format!(
                    "  undo {} (expected {want_undo}), redo {} (expected {want_redo})",
                    undo.len(),
                    redo.len()
                ),
            );
        }
        for (i, item) in undo.iter().enumerate() {
            let depth = self.cursor - i;
            let text = item_text(item);
            if text != self.depth(depth).item {
                self.fail(
                    &format!("after {what} the undo entry at depth {depth} reads differently"),
                    &format!("  recorded {}\n  live     {text}", self.depth(depth).item),
                );
            }
        }
        for (j, item) in redo.iter().enumerate() {
            let depth = self.cursor + 1 + j;
            let text = item_text(item);
            if text != self.depth(depth).item {
                self.fail(
                    &format!("after {what} the redo entry into depth {depth} reads differently"),
                    &format!("  recorded {}\n  live     {text}", self.depth(depth).item),
                );
            }
        }
        self.outcome.oracle_us.2 += started.elapsed().as_micros();
    }

    /// Forget every depth above `depth`.
    pub(super) fn truncate_above(&mut self, depth: usize) {
        self.top = depth;
        self.depths.truncate(depth + 1);
        self.walls.retain(|&wall| wall <= depth);
    }

    /// Make everything below `floor` unreachable for good.
    pub(super) fn raise_floor(&mut self, floor: usize) {
        for slot in self.depths.iter_mut().take(floor) {
            *slot = None;
        }
        self.floor = floor;
        self.walls.retain(|&wall| wall > floor);
    }

    // -----------------------------------------------------------------------
    // Mutations
    // -----------------------------------------------------------------------

    /// One drawn mutation.
    pub(super) fn step_mutate(&mut self) {
        let targets = read_targets(&self.state);
        let op = if self.knobs.extended && self.aux.chance(EXTENDED_PERCENT) {
            fuzz_extended_tests::draw_extended(self, &targets)
        } else {
            self.draw_op(&targets)
        };
        let Some(op) = op else {
            return;
        };
        self.step_apply(&op, &mut Refs::identity(), &targets);
    }

    /// Execute one `Op` and hold the app to what it did: the path a drawn
    /// move and a replayed one share.
    pub(super) fn step_apply(
        &mut self,
        op: &Op,
        refs: &mut Refs,
        targets: &Targets,
    ) -> StepOutcome {
        let before = Before {
            project: self.live(),
            undo: self.undo_len(),
            redo: self.redo_len(),
            newest: self.newest_item(),
            dirty: self.state.has_unsaved_changes(),
        };
        let mark = self.recorder_mark(MoveKind::Mutate(Box::new(op.clone())));
        self.starting_move(op.describe());
        let applied = self.catching(|fuzzer| fuzzer.apply_op(op, refs, targets));
        self.finished_move();
        let Some(applied) = applied else {
            return StepOutcome::Stopped;
        };
        self.recorder_seal(mark);
        let kind = op.kind();
        let Some(drawn) = applied else {
            self.outcome.skipped += 1;
            self.outcome.kinds.entry(kind).or_default().skipped += 1;
            return StepOutcome::Skipped;
        };
        self.outcome.kinds.entry(kind).or_default().applied += 1;
        self.record_mutation(kind, &drawn, &before);
        StepOutcome::Applied
    }

    fn recorder_mark(&self, kind: MoveKind) -> Option<Inventory> {
        if !recorder_active() {
            return None;
        }
        let before = self.inventory();
        recorder_push(TraceMove {
            kind,
            created: Created::default(),
            rng: self.rng.state(),
            aux: self.knobs.any().then(|| self.aux.state()),
        });
        Some(before)
    }

    fn recorder_seal(&self, mark: Option<Inventory>) {
        if let Some(before) = mark {
            recorder_seal(self.inventory().created_since(&before));
        }
    }

    /// The case analysis on what one command did to the stacks and the
    /// survey: refused, recorded, or a no-op.
    fn record_mutation(&mut self, kind: &'static str, drawn: &Drawn, before: &Before) {
        let accepted = drawn.result.success;
        let (undo, redo) = (self.undo_len(), self.redo_len());
        let newest = self.newest_item();
        let pushed = undo > before.undo
            || (undo == before.undo && newest.is_some() && newest != before.newest);
        self.note(format!(
            "{} -> {} (undo {}->{undo}, redo {}->{redo})",
            drawn.label,
            if accepted {
                "ok".to_string()
            } else {
                format!("rejected [{}]", drawn.result.error.as_deref().unwrap_or(""))
            },
            before.undo,
            before.redo,
        ));
        if !accepted || !pushed {
            // Refused, or a success that recorded nothing: either way NOTHING
            // may have changed - the survey to the bit, its session ids and
            // its edit time, the dirty flag, both stacks.
            let what = if accepted {
                self.outcome.noops += 1;
                self.outcome.kinds.entry(kind).or_default().noops += 1;
                "a no-op command"
            } else {
                self.outcome.rejected += 1;
                self.outcome.kinds.entry(kind).or_default().rejected += 1;
                "a REJECTED command"
            };
            if undo != before.undo || redo != before.redo || newest != before.newest {
                self.fail(
                    &format!("{what} changed the history stacks"),
                    &format!(
                        "  {} -> undo {}->{undo}, redo {}->{redo}; error {:?}",
                        drawn.label, before.undo, before.redo, drawn.result.error
                    ),
                );
            }
            let live = self.live();
            if live != before.project
                || live.modified_at != before.project.modified_at
                || testkit::session_ids(&live) != testkit::session_ids(&before.project)
                || canonical_bytes(&live, live.counters)
                    != canonical_bytes(&before.project, before.project.counters)
            {
                self.fail(
                    &format!("{what} changed the survey"),
                    &format!("  {}: error {:?}", drawn.label, drawn.result.error),
                );
            }
            if self.state.has_unsaved_changes() != before.dirty {
                self.fail(
                    &format!("{what} changed the unsaved-changes flag"),
                    &format!("  {}", drawn.label),
                );
            }
            self.expect_stacks(what);
            self.expect_state(self.cursor, what, true);
            return;
        }
        if undo > before.undo + 1 {
            self.fail(
                &format!("one command pushed {} undo entries", undo - before.undo),
                &format!("  after {}", drawn.label),
            );
        }
        if redo != 0 {
            self.fail(
                "a recorded command did not clear the redo stack",
                &format!("  after {}", drawn.label),
            );
        }
        let live = self.live();
        if live.modified_at <= before.project.modified_at {
            self.fail(
                "a recorded command did not stamp the edit time",
                &format!(
                    "  after {}: modified_at {} -> {}",
                    drawn.label, before.project.modified_at, live.modified_at
                ),
            );
        }
        if !self.state.has_unsaved_changes() {
            self.fail(
                "a recorded command left the project clean",
                &format!("  after {}", drawn.label),
            );
        }
        self.outcome.kinds.entry(kind).or_default().pushed += 1;
        self.outcome.mutations += 1;
        self.cursor += 1;
        self.truncate_above(self.cursor - 1);
        self.top = self.cursor;
        self.check_live(&drawn.label);
        let item = newest.unwrap_or_default();
        let depth = self.record_depth(item, true);
        self.depths.push(Some(depth));
        self.outcome.max_depth = self.outcome.max_depth.max(self.cursor);
        self.absorb_trim(&drawn.label);
        self.expect_stacks(&drawn.label);
    }

    /// After a push: the app trimmed exactly what its count limit says, from
    /// the oldest end, and the model's floor follows.
    fn absorb_trim(&mut self, what: &str) {
        let held = self.cursor - self.floor;
        let undo = self.undo_len();
        if undo == held {
            return;
        }
        let overflow = held.saturating_sub(self.history_limit);
        if undo > held || held - undo != overflow {
            self.fail(
                &format!("after {what} the history kept the wrong number of entries"),
                &format!(
                    "  {held} reachable, limit {}, the app holds {undo} (expected {})",
                    self.history_limit,
                    held - overflow
                ),
            );
        }
        self.outcome.trimmed += overflow;
        self.raise_floor(self.floor + overflow);
        self.note(format!(
            "  ^ the history limit trimmed {overflow} entr(ies); floor -> {}",
            self.floor
        ));
    }

    // -----------------------------------------------------------------------
    // History moves
    // -----------------------------------------------------------------------

    pub(super) fn step_recorded_history(&mut self, back: bool, steps: usize) -> bool {
        let mark = self.recorder_mark(if back {
            MoveKind::Undo(steps)
        } else {
            MoveKind::Redo(steps)
        });
        let alive = self.step_history(back, steps);
        self.recorder_seal(mark);
        alive
    }

    /// One history move of `steps` (clamped to what is reachable); more than
    /// one goes through `undo_to` / `redo_to`. Returns false when the app
    /// panicked.
    pub(super) fn step_history(&mut self, back: bool, steps: usize) -> bool {
        let reach = if back {
            self.cursor - self.reach_floor()
        } else {
            self.reach_top() - self.cursor
        };
        if reach == 0 || steps == 0 {
            return true;
        }
        let steps = steps.min(reach);
        let jumped = steps > 1;
        let target = if back {
            self.cursor - steps
        } else {
            self.cursor + steps
        };
        // The entries this move crosses: into depths cursor..target+1 (undo)
        // or cursor+1..=target (redo).
        let crossed: Vec<usize> = if back {
            (target + 1..=self.cursor).collect()
        } else {
            (self.cursor + 1..=target).collect()
        };
        let session = crossed.iter().all(|&d| self.depth(d).session);
        let label = format!(
            "{}{} x{steps}",
            if back { "undo" } else { "redo" },
            if jumped { "To" } else { "" }
        );
        self.starting_move(label.clone());
        let result = self.catching(|fuzzer| match (back, jumped) {
            (true, true) => fuzzer.state.undo_to(steps - 1),
            (true, false) => fuzzer.state.undo(),
            (false, true) => fuzzer.state.redo_to(steps - 1),
            (false, false) => fuzzer.state.redo(),
        });
        self.finished_move();
        let Some(result) = result else {
            return false;
        };
        if !result.success {
            let error = result.error.clone().unwrap_or_default();
            self.note(format!("{label} -> FAILED [{error}]"));
            self.fail(
                &format!("{label} at depth {} failed outright", self.cursor),
                &format!("  error: {error}"),
            );
        }
        self.cursor = target;
        self.note(format!("{label} -> depth {}", self.cursor));
        if back {
            self.outcome.undos += 1;
        } else {
            self.outcome.redos += 1;
        }
        if !session {
            // A decoded entry mints the ids of the shots it re-creates.
            self.id_gen += 1;
            self.id_taint = true;
        }
        self.expect_stacks(&label);
        self.expect_state(self.cursor, &label, session);
        if jumped {
            self.outcome.jumps += 1;
            self.probe_jump_equivalence(back, steps, result.reversed_lines.as_deref());
        }
        true
    }

    /// `undo_to(N)` / `redo_to(N)` are N single steps: walk the same distance
    /// back and forth in single steps (every arrival checked against the
    /// record), and the reversed lines the jump reported must be the single
    /// steps' in order.
    fn probe_jump_equivalence(
        &mut self,
        back: bool,
        steps: usize,
        jumped: Option<&[ReversedLine]>,
    ) {
        let pairs = |lines: Option<&[ReversedLine]>| -> Vec<(LineId, u32)> {
            lines
                .unwrap_or_default()
                .iter()
                .map(|r| (r.line_id, r.shot_count))
                .collect()
        };
        let jumped = pairs(jumped);
        for _ in 0..steps {
            if !self.step_history(!back, 1) {
                return;
            }
        }
        let mut singles = Vec::new();
        for _ in 0..steps {
            let reach_before = self.cursor;
            let result = {
                self.starting_move(format!(
                    "{} x1 (jump probe)",
                    if back { "undo" } else { "redo" }
                ));
                let r = self.catching(|f| if back { f.state.undo() } else { f.state.redo() });
                self.finished_move();
                match r {
                    Some(r) => r,
                    None => return,
                }
            };
            if !result.success {
                self.fail(
                    "a single step of a jump's distance failed",
                    &format!("  at depth {reach_before}: {:?}", result.error),
                );
            }
            singles.extend(pairs(result.reversed_lines.as_deref()));
            self.cursor = if back {
                self.cursor - 1
            } else {
                self.cursor + 1
            };
            let session = self
                .depth(if back { self.cursor + 1 } else { self.cursor })
                .session;
            if !session {
                self.id_gen += 1;
                self.id_taint = true;
            }
            self.expect_stacks("a jump probe step");
            self.expect_state(self.cursor, "a jump probe step", session);
        }
        if singles != jumped {
            self.fail(
                "a jump reported other reversed lines than its single steps",
                &format!("  jump {jumped:?}\n  singles {singles:?}"),
            );
        }
        self.note(format!(
            "  ^ {steps} single step(s) back and forth land where the jump did"
        ));
    }

    /// The closing guarantee: unwind every reachable entry to the floor and
    /// replay every one back to the top, each arrival checked, with the
    /// refusal probes at both edges and a tamper probe every few depths.
    pub(super) fn finish(&mut self) {
        let summit = self.top;
        while self.cursor > self.reach_floor() {
            if (self.cursor - self.floor).is_multiple_of(TAMPER_EVERY) {
                self.probe_tamper(true);
            }
            if !self.step_history(true, 1) {
                return;
            }
        }
        self.probe_wall(true);
        self.probe_refusal(true);
        while self.cursor < summit.min(self.reach_top()) {
            if (summit - self.cursor).is_multiple_of(TAMPER_EVERY) {
                self.probe_tamper(false);
            }
            if !self.step_history(false, 1) {
                return;
            }
        }
        self.probe_wall(false);
        self.probe_refusal(false);
    }

    /// Would a save/close/reopen keep fewer entries than the model holds?
    /// Only the newest `undo_persist_limit` of each stack ride the file.
    pub(super) fn crossing_would_truncate(&self) -> bool {
        self.cursor - self.floor > self.persist_limit || self.top - self.cursor > self.persist_limit
    }

    /// Save, close, reopen - once, or twice back to back under `SAVES`.
    pub(super) fn cross_boundary(&mut self) -> bool {
        if self.orphan_pending {
            self.orphan_pending = false;
            fuzz_extended_tests::expose_orphan_closure(self);
        }
        if self.top == self.floor || (self.crossing_would_truncate() && !self.knobs.trim_crossings)
        {
            return false;
        }
        let passes = if self.knobs.saves && self.aux.chance(DOUBLE_CROSSING_PERCENT) {
            self.outcome.coverage.double_crossings += 1;
            2
        } else {
            1
        };
        for pass in 0..passes {
            if pass > 0 {
                self.note("  ^ crossing again, with nothing in between".to_string());
            }
            self.cross_once();
        }
        true
    }
}

/// One mutation in ten is an extended action under `EXTENDED`.
const EXTENDED_PERCENT: u32 = 10;
/// A quarter of `SAVES` crossings cross twice.
const DOUBLE_CROSSING_PERCENT: u32 = 25;
/// A `SAVES` save in place, per move.
const SAVE_PERCENT: u32 = 3;
/// The closing walk probes a tampered step every this many depths.
const TAMPER_EVERY: usize = 4;

// ===========================================================================
// The generators: they DRAW, `apply_op` executes
// ===========================================================================

impl Fuzzer {
    /// Draw one mutation; `None` when the project momentarily has no target
    /// of the drawn kind. Every family is in the table, the rarer actions in
    /// the last block (`fuzz_extended_tests.rs`).
    pub(super) fn draw_op(&mut self, targets: &Targets) -> Option<Op> {
        match self.rng.below(120) {
            0..=8 => Some(self.gen_create_line(targets)),
            9..=18 => self.gen_create_shot(targets),
            19..=30 => self.gen_update_shot(targets),
            31..=35 => self.gen_set_shot_kind(targets),
            36..=41 => self.gen_update_line(targets),
            42..=45 => self.gen_delete_shot(targets),
            46..=47 => self.gen_delete_line(targets),
            48..=51 => self.gen_reverse_line(targets),
            52..=56 => self.gen_move(targets),
            57..=60 => self.gen_move_station(targets),
            61..=64 => self.gen_station_comment(targets),
            65..=69 => self.gen_merge(targets),
            70..=72 => self.gen_detach(targets),
            73..=76 => self.gen_closure(targets),
            77..=80 => self.gen_zero_datum(targets),
            81..=87 => self.gen_split_family(targets),
            88..=91 => self.gen_batch(targets),
            _ => fuzz_extended_tests::draw_rare(self, targets),
        }
    }

    pub(super) fn pick_shot(&mut self, targets: &Targets) -> Option<(LineId, ShotId)> {
        let populated: Vec<&(LineId, Vec<ShotId>)> = targets
            .lines
            .iter()
            .filter(|(_, shots)| !shots.is_empty())
            .collect();
        if populated.is_empty() {
            return None;
        }
        let (line_id, shots) = populated[self.rng.below(populated.len())];
        Some((*line_id, shots[self.rng.below(shots.len())]))
    }

    pub(super) fn pick_line(&mut self, targets: &Targets) -> Option<LineId> {
        (!targets.lines.is_empty()).then(|| targets.lines[self.rng.below(targets.lines.len())].0)
    }

    pub(super) fn pick_station(&mut self, targets: &Targets) -> Option<StationT> {
        (!targets.slots.is_empty()).then(|| targets.station_at(self.rng.below(targets.slots.len())))
    }

    fn gen_create_line(&mut self, targets: &Targets) -> Op {
        let count = self.rng.below(4);
        let shots: Vec<ShotData> = (0..count).map(|_| random_shot(&mut self.rng)).collect();
        let null_metadata = self.rng.chance(30);
        let direction = match self.rng.below(3) {
            0 => None,
            1 => Some(SurveyDirection::In),
            _ => Some(SurveyDirection::Out),
        };
        let section =
            (!targets.sections.is_empty() && self.rng.chance(25)).then(|| self.rng.below(8) as u32);
        Op::CreateLine {
            name: (!null_metadata).then(|| format!("fuzz-{}", self.rng.range(1000, 9999))),
            surveyor: (!null_metadata).then(|| "fuzz".to_string()),
            color: (*self.rng.pick(COLORS)).to_string(),
            direction,
            shots,
            section,
        }
    }

    fn gen_create_shot(&mut self, targets: &Targets) -> Option<Op> {
        let line_id = self.pick_line(targets)?;
        let shots = targets.shots_of(line_id).unwrap_or_default().to_vec();
        let shot = random_shot(&mut self.rng);
        let (after, before) = if shots.is_empty() {
            (None, None)
        } else {
            match self.rng.below(3) {
                0 => (Some(shots[self.rng.below(shots.len())]), None),
                1 => (None, Some(shots[self.rng.below(shots.len())])),
                _ => (None, None),
            }
        };
        Some(Op::CreateShot {
            line: line_id,
            shot: Box::new(shot),
            after: after.map(|id| targets.shot_ref(line_id, id)),
            before: before.map(|id| targets.shot_ref(line_id, id)),
        })
    }

    fn gen_update_shot(&mut self, targets: &Targets) -> Option<Op> {
        let (line_id, shot_id) = self.pick_shot(targets)?;
        let mut updates = ShotUpdates::default();
        if self.rng.chance(50) {
            updates.length = Some(self.rng.float(0.5, 20.0));
        }
        if self.rng.chance(40) {
            updates.azimuth_in = Some(self.rng.float(0.0, 359.9));
        }
        if self.rng.chance(20) {
            updates.azimuth_out = Some(self.rng.float(0.0, 359.9));
        }
        if self.rng.chance(30) {
            updates.depth_out = Some(self.rng.float(0.0, 40.0));
        }
        if self.rng.chance(20) {
            updates.depth_in = Some(self.rng.float(0.0, 40.0));
        }
        if self.rng.chance(30) {
            updates.pitch_in = Some(self.rng.chance(50).then(|| self.rng.float(-60.0, 60.0)));
        }
        if self.rng.chance(30) {
            updates.left = Some(self.rng.chance(50).then(|| self.rng.float(0.0, 5.0)));
        }
        if self.rng.chance(15) {
            updates.down = Some(self.rng.chance(50).then(|| self.rng.float(0.0, 5.0)));
        }
        if self.rng.chance(25) {
            updates.comment = Some(
                self.rng
                    .chance(50)
                    .then(|| format!("c{}", self.rng.range(0, 999))),
            );
        }
        if self.rng.chance(5) {
            // Refused: a negative length, or nothing at all (a no-op).
            updates.length = Some(-1.0);
        }
        Some(Op::UpdateShot {
            shot: targets.shot_ref(line_id, shot_id),
            updates: Box::new(updates),
        })
    }

    fn gen_set_shot_kind(&mut self, targets: &Targets) -> Option<Op> {
        let (line_id, shot_id) = self.pick_shot(targets)?;
        let kind = *self
            .rng
            .pick(&[ShotKind::Normal, ShotKind::Jump, ShotKind::Reference]);
        Some(Op::SetShotKind {
            shot: targets.shot_ref(line_id, shot_id),
            kind,
        })
    }

    fn gen_update_line(&mut self, targets: &Targets) -> Option<Op> {
        let line_id = self.pick_line(targets)?;
        let mut updates = LineUpdates::default();
        if self.rng.chance(45) {
            updates.name = Some(
                self.rng
                    .chance(60)
                    .then(|| format!("r{}", self.rng.range(0, 999))),
            );
        }
        if self.rng.chance(40) {
            updates.surveyor = Some(
                self.rng
                    .chance(60)
                    .then(|| format!("s{}", self.rng.range(0, 9))),
            );
        }
        if self.rng.chance(20) {
            updates.date = Some(self.rng.chance(60).then(|| {
                chrono::NaiveDate::from_ymd_opt(2025, 1 + self.rng.below(12) as u32, 1)
                    .unwrap_or_default()
            }));
        }
        if self.rng.chance(40) {
            updates.color = Some((*self.rng.pick(COLORS)).to_string());
        }
        if self.rng.chance(20) {
            updates.direction = Some(if self.rng.chance(50) {
                SurveyDirection::In
            } else {
                SurveyDirection::Out
            });
        }
        if self.rng.chance(30) {
            updates.quality = Some(self.rng.range(1, 5) as f64);
        }
        Some(Op::UpdateLine {
            line: line_id,
            updates: Box::new(updates),
        })
    }

    fn gen_delete_shot(&mut self, targets: &Targets) -> Option<Op> {
        let (line_id, shot_id) = self.pick_shot(targets)?;
        Some(Op::DeleteShot {
            shot: targets.shot_ref(line_id, shot_id),
        })
    }

    fn gen_delete_line(&mut self, targets: &Targets) -> Option<Op> {
        // Keep a few lines so cross-line edits stay reachable.
        if targets.lines.len() <= 3 {
            return None;
        }
        Some(Op::DeleteLine {
            line: self.pick_line(targets)?,
        })
    }

    fn gen_reverse_line(&mut self, targets: &Targets) -> Option<Op> {
        Some(Op::ReverseLine {
            line: self.pick_line(targets)?,
            swap_left_right: self.rng.chance(50),
        })
    }

    fn gen_move(&mut self, targets: &Targets) -> Option<Op> {
        let line = self.pick_line(targets)?;
        Some(Op::Move {
            line,
            offset: random_offset(&mut self.rng),
            segment: self.rng.chance(50),
        })
    }

    fn gen_move_station(&mut self, targets: &Targets) -> Option<Op> {
        let station = self.pick_station(targets)?;
        Some(Op::MoveStation {
            station,
            offset: random_offset(&mut self.rng),
        })
    }

    fn gen_station_comment(&mut self, targets: &Targets) -> Option<Op> {
        let station = self.pick_station(targets)?;
        let comment = self
            .rng
            .chance(70)
            .then(|| format!("note-{}", self.rng.range(0, 9999)));
        Some(Op::StationComment { station, comment })
    }

    /// Two slots on DIFFERENT lines.
    pub(super) fn pick_cross_pair(&mut self, targets: &Targets) -> Option<(StationT, StationT)> {
        if targets.lines.len() < 2 || targets.slots.len() < 2 {
            return None;
        }
        let a = targets.station_at(self.rng.below(targets.slots.len()));
        let others: Vec<usize> = (0..targets.slots.len())
            .filter(|&i| targets.slots[i].0 != a.line)
            .collect();
        if others.is_empty() {
            return None;
        }
        Some((a, targets.station_at(others[self.rng.below(others.len())])))
    }

    fn gen_merge(&mut self, targets: &Targets) -> Option<Op> {
        let (a, b) = self.pick_cross_pair(targets)?;
        Some(Op::Merge { a, b })
    }

    /// Detach one line from a junction (the pair form of Unmerge): any two
    /// slots of one station are a pair.
    fn gen_detach(&mut self, targets: &Targets) -> Option<Op> {
        if targets.junctions.is_empty() {
            return None;
        }
        let (station, slots) = &targets.junctions[self.rng.below(targets.junctions.len())];
        let first = self.rng.below(slots.len());
        let mut second = self.rng.below(slots.len());
        if second == first {
            second = (first + 1) % slots.len();
        }
        Some(Op::Detach {
            a: StationT {
                station: *station,
                line: slots[first].0,
            },
            b: StationT {
                station: *station,
                line: slots[second].0,
            },
        })
    }

    fn gen_closure(&mut self, targets: &Targets) -> Option<Op> {
        if !targets.closures.is_empty() && self.rng.chance(40) {
            return Some(Op::RemoveClosure {
                closure: targets.closures[self.rng.below(targets.closures.len())],
            });
        }
        // Mostly two stations a closure may tie: in one component (a loop),
        // or in two datum-held ones; else any pair, which is refused.
        let a = self.pick_station(targets)?;
        let comp = |s: StationId| targets.component.get(&s).copied();
        let tieable: Vec<usize> = (0..targets.slots.len())
            .filter(|&i| {
                let (_, _, b) = targets.slots[i];
                b != a.station
                    && (comp(b) == comp(a.station)
                        || (comp(b).is_some_and(|c| targets.held.contains(&c))
                            && comp(a.station).is_some_and(|c| targets.held.contains(&c))))
            })
            .collect();
        let b = if !tieable.is_empty() && self.rng.chance(80) {
            targets.station_at(tieable[self.rng.below(tieable.len())])
        } else {
            self.pick_cross_pair(targets)?.1
        };
        // The legacy strategies are refused at creation: drawn now and then,
        // so the refusal is exercised.
        let strategy = *self.rng.pick(&[
            ClosureStrategy::Move,
            ClosureStrategy::Move,
            ClosureStrategy::Batch,
            ClosureStrategy::LeastSquaresWeighted,
            ClosureStrategy::ProportionalByLength,
        ]);
        Some(Op::CreateClosure { a, b, strategy })
    }

    fn gen_zero_datum(&mut self, targets: &Targets) -> Option<Op> {
        if !targets.datums.is_empty() && self.rng.chance(40) {
            let (station, via) = targets.datums[self.rng.below(targets.datums.len())];
            let line = via.or_else(|| {
                targets
                    .slots
                    .iter()
                    .find(|(_, _, s)| *s == station)
                    .map(|(l, _, _)| *l)
            })?;
            return Some(Op::RemoveZeroDatum {
                station: StationT { station, line },
            });
        }
        let station = self.pick_station(targets)?;
        let coordinates = GpsCoordinates::new(
            self.rng.float(20.2, 20.5),
            self.rng.float(-87.5, -87.2),
            self.rng.float(0.0, 5.0),
        )
        .ok()?;
        let accuracy_mm = self
            .rng
            .chance(30)
            .then(|| self.rng.range(1, 20_000) as u32);
        let entrance = match self.rng.below(4) {
            0 => Some(true),
            1 => Some(false),
            _ => None,
        };
        Some(Op::SetZeroDatum {
            station,
            coordinates,
            accuracy_mm,
            entrance,
        })
    }

    /// Combine Lines at a line end: the join the last Split Line Here made,
    /// most of the time while there is one, else either end of a line.
    fn gen_combine_lines(&mut self, targets: &Targets) -> Option<Op> {
        let station = match self.last_split_join {
            Some(join) if self.rng.chance(60) => join,
            _ => {
                let (line, shots) = &targets.lines[self.rng.below(targets.lines.len().max(1))];
                let index = if self.rng.chance(50) {
                    0
                } else {
                    shots.len() as u32
                };
                let station = targets
                    .slots
                    .iter()
                    .find(|(l, i, _)| l == line && *i == index)
                    .map(|(_, _, s)| *s)?;
                StationT {
                    station,
                    line: *line,
                }
            }
        };
        Some(Op::CombineLines {
            station,
            candidate: self.rng.below(4) as u8,
            keep_b: self.rng.chance(30),
            swap_lr: self.rng.chance(50),
            pick: self.rng.below(3) as u8,
        })
    }

    /// The recipes that bypass `execute`: split shot, delete and split, the
    /// last shot and its line, Split Line Here, Combine, the range deletes.
    fn gen_split_family(&mut self, targets: &Targets) -> Option<Op> {
        let (line_id, shot_id) = self.pick_shot(targets)?;
        let shots = targets.shots_of(line_id).unwrap_or_default().to_vec();
        let shot = targets.shot_ref(line_id, shot_id);
        let names = self.rng.below(1000) as u32;
        // A mid-line shot of a line long enough to have one: an end shot is
        // refused by the split recipes ("delete the shot instead").
        let inner = |rng: &mut Rng| -> Option<ShotRefT> {
            let long: Vec<&(LineId, Vec<ShotId>)> =
                targets.lines.iter().filter(|(_, s)| s.len() >= 3).collect();
            if long.is_empty() {
                return None;
            }
            let (line, ids) = long[rng.below(long.len())];
            Some(targets.shot_ref(*line, ids[1 + rng.below(ids.len() - 2)]))
        };
        match self.rng.below(7) {
            0 => {
                let distance = if self.rng.chance(20) {
                    *self.rng.pick(&[0.0, 1e-9, GEOMETRY_EXTREME])
                } else {
                    self.rng.float(0.1, 8.0)
                };
                Some(Op::SplitShotAtDistance { shot, distance })
            }
            1 => Some(Op::DeleteShotAndSplit {
                shot: if self.rng.chance(80) {
                    inner(&mut self.rng).unwrap_or(shot)
                } else {
                    shot
                },
                names,
            }),
            2 => {
                // The recipe is for a line's ONLY shot; any other is refused.
                let single: Vec<&(LineId, Vec<ShotId>)> =
                    targets.lines.iter().filter(|(_, s)| s.len() == 1).collect();
                let shot = if !single.is_empty() && self.rng.chance(80) {
                    let (line, ids) = single[self.rng.below(single.len())];
                    targets.shot_ref(*line, ids[0])
                } else {
                    shot
                };
                Some(Op::DeleteLastShotAndLine { shot })
            }
            3 => Some(Op::SplitLine {
                shot: if self.rng.chance(80) {
                    inner(&mut self.rng).unwrap_or(shot)
                } else {
                    shot
                },
                along: self.rng.chance(50),
                distance: if self.rng.chance(20) {
                    *self.rng.pick(&[0.0, 1e-9, 0.005])
                } else {
                    self.rng.float(0.1, 1.0)
                },
                side: self.rng.below(6) as u8,
                names,
            }),
            4 => self.gen_combine_lines(targets),
            _ => {
                let split = self.rng.chance(50);
                if split {
                    // Strictly inside the line most of the time: a prefix or
                    // a suffix is refused ("use the shift").
                    let long: Vec<&(LineId, Vec<ShotId>)> =
                        targets.lines.iter().filter(|(_, s)| s.len() >= 3).collect();
                    if !long.is_empty() && self.rng.chance(80) {
                        let (line, ids) = long[self.rng.below(long.len())];
                        let lo = 1 + self.rng.below(ids.len() - 2);
                        let hi = lo + self.rng.below(ids.len() - 1 - lo);
                        return Some(Op::DeleteShotRange {
                            line: *line,
                            from: targets.shot_ref(*line, ids[lo]),
                            to: targets.shot_ref(*line, ids[hi]),
                            split,
                            names,
                        });
                    }
                }
                if shots.len() < 2 {
                    return None;
                }
                let first = self.rng.below(shots.len());
                let second = self.rng.below(shots.len());
                let (lo, hi) = (first.min(second), first.max(second));
                Some(Op::DeleteShotRange {
                    line: line_id,
                    from: targets.shot_ref(line_id, shots[lo]),
                    to: targets.shot_ref(line_id, shots[hi]),
                    split,
                    names,
                })
            }
        }
    }

    /// A batch is ONE undo entry over several sub-commands, all or nothing.
    fn gen_batch(&mut self, targets: &Targets) -> Option<Op> {
        let count = self.rng.range(2, 4);
        let mut commands = Vec::new();
        for _ in 0..count {
            let built = match self.rng.below(8) {
                0 => self
                    .pick_shot(targets)
                    .map(|(line_id, shot_id)| BatchOp::UpdateShotLength {
                        shot: targets.shot_ref(line_id, shot_id),
                        length: self.rng.float(1.0, 15.0),
                    }),
                1 => self.pick_line(targets).map(|line| BatchOp::MoveLine {
                    line,
                    offset: random_offset(&mut self.rng),
                }),
                2 => self.pick_line(targets).map(|line| BatchOp::MoveSegment {
                    line,
                    offset: random_offset(&mut self.rng),
                }),
                3 => self
                    .pick_station(targets)
                    .map(|station| BatchOp::MoveStation {
                        station,
                        offset: random_offset(&mut self.rng),
                    }),
                4 => self.pick_line(targets).map(|line| BatchOp::ReverseLine {
                    line,
                    swap_left_right: self.rng.chance(50),
                }),
                5 => self.pick_line(targets).map(|line| BatchOp::RenameLine {
                    line,
                    name: self.rng.below(6) as u32,
                }),
                6 => self
                    .pick_station(targets)
                    .map(|station| BatchOp::RenameStation {
                        station,
                        name: self.rng.below(6) as u32,
                    }),
                _ => {
                    if self.rng.chance(30) {
                        self.pick_shot(targets)
                            .map(|(line_id, shot_id)| BatchOp::Disallowed {
                                shot: targets.shot_ref(line_id, shot_id),
                            })
                    } else {
                        None
                    }
                }
            };
            if let Some(command) = built {
                commands.push(command);
            }
        }
        if commands.is_empty() {
            return None;
        }
        // The case the rollback exists for: a last sub-command naming a line
        // that cannot exist, so the whole batch is refused and changes nothing.
        let poisoned = self.rng.chance(20);
        Some(Op::Batch { commands, poisoned })
    }

    // -----------------------------------------------------------------------
    // The one execution path
    // -----------------------------------------------------------------------

    /// Execute one [`Op`]. EVERY mutation goes through here: a drawn one with
    /// [`Refs::identity`], a replayed one with a mapping resolver. `None`
    /// means INAPPLICABLE (a reference the resolver could not map).
    pub(super) fn apply_op(
        &mut self,
        op: &Op,
        refs: &mut Refs,
        targets: &Targets,
    ) -> Option<Drawn> {
        let label = op.describe();
        let result = match op {
            Op::CreateLine {
                name,
                surveyor,
                color,
                direction,
                shots,
                section,
            } => {
                let cave_section_id = section.and_then(|at| {
                    let len = targets.sections.len() as u32;
                    (len > 0).then(|| targets.sections[(at % len) as usize])
                });
                self.state.execute(Command::CreateLine {
                    name: name.clone(),
                    surveyor: surveyor.clone(),
                    date: None,
                    color: color.clone(),
                    direction: *direction,
                    shots: shots.clone(),
                    cave_section_id,
                })
            }
            Op::CreateShot {
                line,
                shot,
                after,
                before,
            } => {
                let Some(line_id) = refs.line(*line, targets) else {
                    return refs.skip(&label);
                };
                let anchor = match (after, before) {
                    (Some(anchor), _) | (None, Some(anchor)) => match refs.shot(anchor, targets) {
                        Some((_, shot_id)) => Some(shot_id),
                        None => return refs.skip(&label),
                    },
                    (None, None) => None,
                };
                let (after_shot_id, before_shot_id) = if after.is_some() {
                    (anchor, None)
                } else {
                    (None, anchor)
                };
                self.state.execute(Command::CreateShot {
                    line_id,
                    shot: (**shot).clone(),
                    after_shot_id,
                    before_shot_id,
                })
            }
            Op::UpdateShot { shot, updates } => {
                let Some((line_id, shot_id)) = refs.shot(shot, targets) else {
                    return refs.skip(&label);
                };
                self.state.execute(Command::UpdateShot {
                    line_id,
                    shot_id,
                    updates: (**updates).clone(),
                })
            }
            Op::SetShotKind { shot, kind } => {
                let Some((line_id, shot_id)) = refs.shot(shot, targets) else {
                    return refs.skip(&label);
                };
                self.state.execute(Command::SetShotKind {
                    line_id,
                    shot_id,
                    kind: *kind,
                })
            }
            Op::UpdateLine { line, updates } => {
                let Some(line_id) = refs.line(*line, targets) else {
                    return refs.skip(&label);
                };
                self.state.execute(Command::UpdateLineMetadata {
                    line_id,
                    updates: (**updates).clone(),
                })
            }
            Op::DeleteShot { shot } => {
                let Some((line_id, shot_id)) = refs.shot(shot, targets) else {
                    return refs.skip(&label);
                };
                self.state.execute(Command::DeleteShot { line_id, shot_id })
            }
            Op::DeleteLine { line } => {
                let Some(line_id) = refs.line(*line, targets) else {
                    return refs.skip(&label);
                };
                self.state.execute(Command::DeleteLine { line_id })
            }
            Op::ReverseLine {
                line,
                swap_left_right,
            } => {
                let Some(line_id) = refs.line(*line, targets) else {
                    return refs.skip(&label);
                };
                self.state.execute(Command::ReverseLine {
                    line_id,
                    swap_left_right: *swap_left_right,
                })
            }
            Op::Move {
                line,
                offset,
                segment,
            } => {
                let Some(line_id) = refs.line(*line, targets) else {
                    return refs.skip(&label);
                };
                self.state.execute(if *segment {
                    Command::MoveSegment {
                        line_id,
                        offset: *offset,
                    }
                } else {
                    Command::MoveLine {
                        line_id,
                        offset: *offset,
                    }
                })
            }
            Op::MoveStation { station, offset } => {
                let Some(station_ref) = refs.station(*station, targets) else {
                    return refs.skip(&label);
                };
                self.state.execute(Command::MoveStation {
                    station_ref,
                    offset: *offset,
                })
            }
            Op::StationComment { station, comment } => {
                let Some(station_ref) = refs.station(*station, targets) else {
                    return refs.skip(&label);
                };
                self.state.execute(Command::SetStationComment {
                    station_ref,
                    comment: comment.clone(),
                })
            }
            Op::Merge { a, b } => {
                let (Some(station_a), Some(station_b)) =
                    (refs.station(*a, targets), refs.station(*b, targets))
                else {
                    return refs.skip(&label);
                };
                self.state.execute(Command::MergeStations {
                    station_a,
                    station_b,
                })
            }
            Op::Detach { a, b } => {
                let (Some(station_a), Some(station_b)) =
                    (refs.station(*a, targets), refs.station(*b, targets))
                else {
                    return refs.skip(&label);
                };
                self.state.execute(Command::UnmergeStationPair {
                    station_a,
                    station_b,
                })
            }
            Op::CreateClosure { a, b, strategy } => {
                let (Some(station_a), Some(station_b)) =
                    (refs.station(*a, targets), refs.station(*b, targets))
                else {
                    return refs.skip(&label);
                };
                self.state.execute(Command::CreateLoopClosure {
                    station_a,
                    station_b,
                    closure_strategy: *strategy,
                })
            }
            Op::RemoveClosure { closure } => {
                let Some(closure_id) = refs.closure(*closure, targets) else {
                    return refs.skip(&label);
                };
                self.state
                    .execute(Command::RemoveLoopClosure { closure_id })
            }
            Op::SetZeroDatum {
                station,
                coordinates,
                accuracy_mm,
                entrance,
            } => {
                let Some(station_ref) = refs.station(*station, targets) else {
                    return refs.skip(&label);
                };
                self.state.execute(Command::SetZeroDatum {
                    station_ref,
                    coordinates: *coordinates,
                    accuracy_m: accuracy_mm.map(|mm| f64::from(mm) / 1000.0),
                    entrance: *entrance,
                })
            }
            Op::RemoveZeroDatum { station } => {
                let Some(station_ref) = refs.station(*station, targets) else {
                    return refs.skip(&label);
                };
                self.state.execute(Command::RemoveZeroDatum { station_ref })
            }
            Op::SplitShotAtDistance { shot, distance } => {
                let Some((line_id, shot_id)) = refs.shot(shot, targets) else {
                    return refs.skip(&label);
                };
                self.state
                    .execute_split_shot_at_distance(line_id, shot_id, *distance)
            }
            Op::DeleteShotAndSplit { shot, names } => {
                let Some((line_id, shot_id)) = refs.shot(shot, targets) else {
                    return refs.skip(&label);
                };
                self.state.execute_delete_shot_and_split(
                    line_id,
                    shot_id,
                    format!("split-{names}a"),
                    format!("split-{names}b"),
                )
            }
            Op::SplitLine {
                shot,
                along,
                distance,
                side,
                names,
            } => {
                let Some((line_id, shot_id)) = refs.shot(shot, targets) else {
                    return refs.skip(&label);
                };
                let at = if *along {
                    crate::models::SplitAt::Shot {
                        shot_id,
                        distance: *distance,
                    }
                } else {
                    let position = targets
                        .shots_of(line_id)
                        .and_then(|shots| shots.iter().position(|&s| s == shot_id))
                        .unwrap_or(0);
                    crate::models::SplitAt::Station {
                        station_index: position as u32 + 1,
                    }
                };
                let pick = |n: u8| match n {
                    0 => crate::models::SplitSide::A,
                    1 => crate::models::SplitSide::B,
                    _ => crate::models::SplitSide::Both,
                };
                let options = crate::models::SplitLineOptions {
                    name_a: format!("split-{names}a"),
                    name_b: format!("split-{names}b"),
                    color_a: "#111111".to_string(),
                    color_b: "#222222".to_string(),
                    comment_side: pick(side % 3),
                    datum_side: pick(side % 2),
                };
                let result = self.state.execute_split_line(line_id, at, options);
                if result.success {
                    // A function of the state, so a replay sets it the same way.
                    self.last_split_join = self.state.get_project_state().and_then(|p| {
                        let first = p.line(line_id)?;
                        Some(StationT {
                            station: first.stations().last()?,
                            line: line_id,
                        })
                    });
                }
                result
            }
            Op::CombineLines {
                station,
                candidate,
                keep_b,
                swap_lr,
                pick,
            } => {
                let Some(station_ref) = refs.station(*station, targets) else {
                    return refs.skip(&label);
                };
                let candidates = self
                    .state
                    .combine_candidates(station_ref)
                    .unwrap_or_default();
                if candidates.is_empty() {
                    // What the context menu does: nothing to offer here.
                    return Some(Drawn::new(
                        label,
                        CommandResult::failure("no line to combine at this station".to_string()),
                    ));
                }
                let chosen = &candidates[usize::from(*candidate) % candidates.len()];
                let (kept, other) = if *keep_b {
                    (chosen.line_b, chosen.line_a)
                } else {
                    (chosen.line_a, chosen.line_b)
                };
                let pick = match pick % 3 {
                    0 => crate::models::CombinePick::Kept,
                    1 => crate::models::CombinePick::Other,
                    _ => crate::models::CombinePick::Both,
                };
                let options = crate::models::CombineLinesOptions {
                    name: "combined".to_string(),
                    color: "#333333".to_string(),
                    swap_lr: *swap_lr,
                    comment_pick: pick,
                    name_pick: pick,
                    join: None,
                };
                self.state.execute_combine_lines(kept, other, options)
            }
            Op::DeleteLastShotAndLine { shot } => {
                let Some((line_id, shot_id)) = refs.shot(shot, targets) else {
                    return refs.skip(&label);
                };
                self.state
                    .execute_delete_last_shot_and_line(line_id, shot_id)
            }
            Op::DeleteShotRange {
                line,
                from,
                to,
                split,
                names,
            } => {
                let Some(line_id) = refs.line(*line, targets) else {
                    return refs.skip(&label);
                };
                let (Some((_, lo)), Some((_, hi))) =
                    (refs.shot(from, targets), refs.shot(to, targets))
                else {
                    return refs.skip(&label);
                };
                if *split {
                    self.state.execute_delete_shot_range_split(
                        line_id,
                        lo,
                        hi,
                        format!("range-{names}a"),
                        format!("range-{names}b"),
                    )
                } else {
                    self.state.execute_delete_shot_range_shift(line_id, lo, hi)
                }
            }
            Op::Batch { commands, poisoned } => {
                // A sub-command whose target is gone is dropped rather than
                // failing the batch.
                let mut built = Vec::new();
                for command in commands {
                    if let Some(command) = batch_command(command, refs, targets) {
                        built.push(command);
                    }
                }
                if built.is_empty() {
                    return refs.skip(&label);
                }
                if *poisoned {
                    built.push(Command::MoveLine {
                        line_id: 999_999,
                        offset: Coordinates::new(1.0, 1.0, 1.0),
                    });
                }
                self.state.execute(Command::Batch {
                    commands: built,
                    label: None,
                })
            }
            Op::Extended(ext) => {
                return fuzz_extended_tests::apply_extended(self, ext, refs, targets);
            }
        };
        Some(Drawn::new(label, result))
    }
}

/// One batch sub-command, resolved; `None` when its target is gone.
fn batch_command(command: &BatchOp, refs: &Refs, targets: &Targets) -> Option<Command> {
    Some(match command {
        BatchOp::UpdateShotLength { shot, length } => {
            let (line_id, shot_id) = refs.shot(shot, targets)?;
            Command::UpdateShot {
                line_id,
                shot_id,
                updates: ShotUpdates {
                    length: Some(*length),
                    ..ShotUpdates::default()
                },
            }
        }
        BatchOp::MoveLine { line, offset } => Command::MoveLine {
            line_id: refs.line(*line, targets)?,
            offset: *offset,
        },
        BatchOp::MoveSegment { line, offset } => Command::MoveSegment {
            line_id: refs.line(*line, targets)?,
            offset: *offset,
        },
        BatchOp::MoveStation { station, offset } => Command::MoveStation {
            station_ref: refs.station(*station, targets)?,
            offset: *offset,
        },
        BatchOp::ReverseLine {
            line,
            swap_left_right,
        } => Command::ReverseLine {
            line_id: refs.line(*line, targets)?,
            swap_left_right: *swap_left_right,
        },
        BatchOp::RenameLine { line, name } => Command::UpdateLineMetadata {
            line_id: refs.line(*line, targets)?,
            updates: LineUpdates {
                name: Some(Some(format!("batch-{name}"))),
                ..LineUpdates::default()
            },
        },
        BatchOp::RenameStation { station, name } => Command::SetStationName {
            station_ref: refs.station(*station, targets)?,
            name: format!("b{name}"),
        },
        BatchOp::Disallowed { shot } => {
            let (line_id, shot_id) = refs.shot(shot, targets)?;
            Command::DeleteShot { line_id, shot_id }
        }
    })
}

// ===========================================================================
// The plan, the knobs and the config space
// ===========================================================================

/// What a run is allowed to do. Serializable because a recorded [`Trace`]
/// carries it: the plan decides the starting project and the prologues the
/// trace does not contain.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct Plan {
    pub(super) iterations: usize,
    /// Cross the save/close/reopen boundary part way through.
    pub(super) persistence: bool,
    /// Draw multi-step `undo_to` / `redo_to` jumps as well as single steps.
    pub(super) jumps: bool,
    /// A genuine loop closure in the starting project.
    pub(super) closures: bool,
    /// Build session ids out of positional order and put the id-addressed
    /// entries on top before crossing - see `expose_stale_ids`.
    pub(super) divergence: bool,
    /// Build the two closure shapes (list order, the orphan) - see
    /// `expose_closure_shapes`. Implies `closures`.
    pub(super) closure_shapes: bool,
    /// Replay `(seed, moves)` as a corpus PROLOGUE first.
    pub(super) prologue: Option<(u32, usize)>,
    /// Crossings per persistence run.
    pub(super) crossings: usize,
    /// Run past the history limit, so the FIFO window slides.
    pub(super) horizon: bool,
    /// Draw this run's `undo_history_limit`.
    #[serde(default)]
    pub(super) config_fuzz: bool,
    /// Horizon, closures, closure shapes, jumps and several crossings at
    /// once; implies `config_fuzz`.
    #[serde(default)]
    pub(super) combined: bool,
    /// Start from a migrated legacy project.
    #[serde(default)]
    pub(super) legacy_origin: bool,
    #[serde(default)]
    pub(super) knobs: Knobs,
}

impl Plan {
    /// The plain plan: in memory, nothing constructed.
    pub(super) const fn plain(iterations: usize) -> Self {
        Self {
            iterations,
            persistence: false,
            jumps: false,
            closures: false,
            divergence: false,
            closure_shapes: false,
            prologue: None,
            crossings: 1,
            horizon: false,
            config_fuzz: false,
            combined: false,
            legacy_origin: false,
            knobs: Knobs::OFF,
        }
    }

    pub(super) const fn persisted(iterations: usize, crossings: usize) -> Self {
        let mut plan = Self::plain(iterations);
        plan.persistence = true;
        plan.crossings = crossings;
        plan
    }

    /// The plan the survey's knobs describe.
    pub(super) fn from_env(iterations: usize, persistence: bool) -> Self {
        let combined = env_flag("COMBINED");
        Self {
            iterations,
            persistence,
            jumps: env_flag("JUMPS") || combined,
            closures: env_flag("CLOSURES") || combined,
            divergence: env_flag("DIVERGENCE"),
            closure_shapes: env_flag("CLOSURE_SHAPES") || combined,
            prologue: None,
            crossings: env_number("CROSSINGS", if combined { COMBINED_CROSSINGS } else { 1 }),
            horizon: env_flag("HORIZON") || combined,
            config_fuzz: env_flag("CONFIG"),
            combined,
            legacy_origin: env_flag("LEGACY_ORIGIN"),
            knobs: Knobs::from_env(),
        }
    }
}

/// The COVERAGE stream's seed offset: `seed ^ COVERAGE_STREAM`.
const COVERAGE_STREAM: u32 = 0x5EED_C0DE;

/// The coverage knobs. All off by default; with all of them off nothing
/// draws from the coverage stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct Knobs {
    #[serde(default)]
    pub(super) format_corrupt: bool,
    #[serde(default)]
    pub(super) trim_crossings: bool,
    #[serde(default)]
    pub(super) saves: bool,
    #[serde(default)]
    pub(super) extended: bool,
    #[serde(default)]
    pub(super) large: bool,
}

impl Knobs {
    pub(super) const OFF: Self = Self {
        format_corrupt: false,
        trim_crossings: false,
        saves: false,
        extended: false,
        large: false,
    };

    pub(super) fn from_env() -> Self {
        Self {
            format_corrupt: env_flag("FORMAT_CORRUPT"),
            trim_crossings: env_flag("TRIM_CROSSINGS"),
            saves: env_flag("SAVES"),
            extended: env_flag("EXTENDED"),
            large: env_flag("LARGE"),
        }
    }

    pub(super) const fn any(self) -> bool {
        self.format_corrupt || self.trim_crossings || self.saves || self.extended || self.large
    }

    pub(super) fn describe(self) -> String {
        [
            (self.format_corrupt, "format-corrupt"),
            (self.trim_crossings, "trim-crossings"),
            (self.saves, "saves"),
            (self.extended, "extended"),
            (self.large, "large"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join("+")
    }
}

/// The CONFIG stream's seed offset: additive, so the primary stream is the
/// same whether or not the config is drawn.
const CONFIG_STREAM: u32 = 0x00C0_F167;

/// The undo-relevant config one run drew. `undo_persist_limit` is not drawn:
/// writing it into the config would persist it through `note_recent_project`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DrawnConfig {
    pub(super) history_limit: usize,
}

impl DrawnConfig {
    fn describe(self) -> String {
        format!(", undo_history_limit={}", self.history_limit)
    }
}

/// The persisted window's default (`config::defaults`).
const UNDO_PERSIST_LIMIT: usize = 20;
/// Headroom below the in-memory limit in the ordinary modes.
const MAX_MODEL_DEPTH: usize = 40;
const DEFAULT_HISTORY_LIMIT: usize = 100;
const JUMP_PERCENT: u32 = 40;

/// 1 (every push trims), a handful, the persisted window, the ordinary
/// depth cap, the app's default, and one the run cannot reach (the control).
fn config_history_ladder(iterations: usize) -> [usize; 10] {
    [
        1,
        2,
        3,
        5,
        8,
        13,
        UNDO_PERSIST_LIMIT,
        MAX_MODEL_DEPTH,
        DEFAULT_HISTORY_LIMIT,
        iterations.saturating_add(1),
    ]
}

fn draw_config(seed: u32, plan: Plan) -> Option<DrawnConfig> {
    if !plan.config_fuzz && !plan.combined {
        return None;
    }
    let mut rng = Rng::new(seed ^ CONFIG_STREAM);
    let ladder = config_history_ladder(plan.iterations);
    Some(DrawnConfig {
        history_limit: ladder[rng.below(ladder.len())],
    })
}

/// Everything a run does BEFORE its own move stream: the starting project,
/// the corpus prologue and the exposure prologues. A replay re-runs all of it.
fn start_run(seed: u32, plan: Plan) -> Fuzzer {
    let mut fuzzer = Fuzzer::new_with(
        plan.prologue
            .map_or(seed, |(prologue_seed, _)| prologue_seed),
        plan.closures || plan.closure_shapes,
        draw_config(seed, plan),
        plan.knobs,
        plan.legacy_origin,
    );
    fuzzer.seed = seed;
    if let Some((_, moves)) = plan.prologue {
        for _ in 0..moves {
            if fuzzer.stopped() {
                break;
            }
            fuzzer.step_mutate();
        }
        fuzzer.rng = Rng::new(seed);
    }
    if plan.divergence {
        fuzz_extended_tests::expose_stale_ids(&mut fuzzer);
        fuzzer.cross_boundary();
    }
    if plan.closure_shapes {
        fuzz_extended_tests::expose_closure_shapes(&mut fuzzer);
        fuzzer.orphan_pending = plan.persistence;
    }
    fuzzer
}

/// The closing guarantee, and the one crossing a persistence run owes.
fn finish_run(mut fuzzer: Fuzzer, plan: Plan) -> Outcome {
    if fuzzer.outcome.app_panic {
        fuzzer.fail(
            "the app panicked inside a move",
            "  see the last move in the log below for the panic's own message",
        );
    }
    fuzzer.outcome.signature = topology_signature(&fuzzer.state);
    if plan.persistence && fuzzer.outcome.reloads == 0 {
        fuzzer.cross_boundary();
    }
    fuzzer.finish();
    if fuzzer.outcome.app_panic {
        fuzzer.fail(
            "the app panicked inside a move",
            "  see the last move in the log below for the panic's own message",
        );
    }
    fuzzer.outcome
}

/// Which iterations of the main loop cross the boundary.
fn boundary_iterations(plan: Plan) -> Vec<usize> {
    let crossings = plan.crossings.max(1);
    if plan.persistence && !plan.divergence {
        (0..crossings)
            .map(|index| plan.iterations / 3 + index * (plan.iterations * 2 / 3) / crossings)
            .collect()
    } else {
        Vec::new()
    }
}

pub(super) fn run_fuzz(seed: u32, plan: Plan) -> Outcome {
    let mut fuzzer = start_run(seed, plan);
    if recorder_armed() {
        recorder_begin(fuzzer.inventory());
    }
    let boundaries = boundary_iterations(plan);
    let depth_cap = if plan.horizon {
        fuzzer.history_limit * 2
    } else {
        MAX_MODEL_DEPTH
    };
    for iteration in 0..plan.iterations {
        if fuzzer.stopped() {
            break;
        }
        if boundaries.contains(&iteration) {
            let mark = fuzzer.recorder_mark(MoveKind::Cross);
            fuzzer.cross_boundary();
            fuzzer.recorder_seal(mark);
        }
        if fuzzer.knobs.saves && fuzzer.aux.chance(SAVE_PERCENT) {
            let mark = fuzzer.recorder_mark(MoveKind::Save);
            fuzzer.save_in_place();
            fuzzer.recorder_seal(mark);
            continue;
        }
        // 60 / 25 / 15 mutate / undo / redo; a horizon run CLIMBS (85 / 8 /
        // 7) until its window slides (the floor leaves 0), or it never
        // reaches the limit - and draws jumps from then on.
        let roll = fuzzer.rng.below(100);
        let sliding = fuzzer.floor > 0;
        let climbing = plan.horizon && !sliding && fuzzer.cursor - fuzzer.floor < depth_cap;
        let mut back = if climbing {
            (85..93).contains(&roll)
        } else {
            (60..85).contains(&roll)
        };
        let mut forward = roll >= if climbing { 93 } else { 85 };
        if forward && fuzzer.cursor >= fuzzer.reach_top() {
            forward = false;
            back = fuzzer.cursor > fuzzer.reach_floor();
        }
        if back && fuzzer.cursor == fuzzer.reach_floor() {
            back = false;
        }
        if !back && !forward {
            if fuzzer.cursor - fuzzer.floor >= depth_cap {
                if !fuzzer.step_recorded_history(true, 1) {
                    break;
                }
                continue;
            }
            fuzzer.step_mutate();
            continue;
        }
        let reach = if back {
            fuzzer.cursor - fuzzer.reach_floor()
        } else {
            fuzzer.reach_top() - fuzzer.cursor
        };
        let steps = if plan.jumps && !climbing && reach > 1 && fuzzer.rng.chance(JUMP_PERCENT) {
            1 + fuzzer.rng.below(reach)
        } else {
            1
        };
        if !fuzzer.step_recorded_history(back, steps) {
            break;
        }
    }
    recorder_end();
    finish_run(fuzzer, plan)
}

/// Replay a recorded trace, or any subsequence of one, through the SAME
/// engine. A move whose target no longer exists is skipped.
pub(super) fn run_trace(trace: &Trace, moves: &[TraceMove]) -> (Outcome, Vec<String>) {
    let mut fuzzer = start_run(trace.seed, trace.plan);
    let mut refs = match Refs::replay(&trace.prelude, &fuzzer.inventory()) {
        Ok(refs) => refs.checked(true),
        Err(message) => panic!("cannot replay this trace: {message}"),
    };
    for mv in moves {
        if fuzzer.stopped() {
            break;
        }
        fuzzer.rng.restore(mv.rng);
        if let Some(aux) = mv.aux {
            fuzzer.aux.restore(aux);
        }
        let before = fuzzer.inventory();
        let alive = match &mv.kind {
            MoveKind::Mutate(op) => {
                let targets = read_targets(&fuzzer.state);
                fuzzer.step_apply(op, &mut refs, &targets) != StepOutcome::Stopped
            }
            MoveKind::Undo(steps) => fuzzer.step_history(true, *steps),
            MoveKind::Redo(steps) => fuzzer.step_history(false, *steps),
            MoveKind::Cross => {
                fuzzer.cross_boundary();
                true
            }
            MoveKind::Save => {
                fuzzer.save_in_place();
                true
            }
        };
        refs.learn(&mv.created, &fuzzer.inventory().created_since(&before));
        if let Some(fault) = trace.fault {
            let lines = fuzzer.live().lines().len();
            if lines >= fault.at_lines {
                fuzzer.fail(
                    &format!("injected fault: the project holds {lines} lines"),
                    "  (the shrinker's own gate)",
                );
            }
        }
        if !alive {
            break;
        }
    }
    let skipped = refs.skipped().to_vec();
    (finish_run(fuzzer, trace.plan), skipped)
}

// ===========================================================================
// Failure signatures and summaries
// ===========================================================================

/// Every run of digits as a single `#`.
fn collapse_digits(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_digits = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            if !in_digits {
                out.push('#');
                in_digits = true;
            }
        } else {
            out.push(ch);
            in_digits = false;
        }
    }
    out
}

/// The discriminator a payload carries below its headline: an app panic's
/// own message up to its first `:` (the failing operation, not the values),
/// so two different panics behind the one headline are two buckets.
fn discriminator(message: &str) -> Option<String> {
    let rest = message.split_once("^ the app panicked: ")?.1;
    let line = rest.lines().next().unwrap_or(rest);
    let token = line.split(':').next().unwrap_or(line).trim();
    (!token.is_empty()).then(|| format!("panic={}", collapse_digits(token)))
}

/// Bucket a failure by its SHAPE: the headline and the first differing
/// member, digits collapsed, plus an app panic's discriminator.
pub(super) fn failure_signature(message: &str) -> String {
    let headline = collapse_digits(message.lines().next().unwrap_or("<empty>"));
    let first_path = message
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("at "))
        .map(|line| collapse_digits(line.split(" (line").next().unwrap_or(line)));
    let mut signature = headline;
    if let Some(token) = discriminator(message) {
        signature.push_str(" | ");
        signature.push_str(&token);
    }
    if let Some(path) = first_path {
        signature.push_str(" | ");
        signature.push_str(&path);
    }
    signature
}

pub(super) fn summarize(label: &str, seed: u32, outcome: &Outcome) -> String {
    format!(
        "{label} seed={seed}: {} mutations, {} rejected, {} no-ops, {} skipped, {} undos, \
         {} redos, {} jumps, {} saves, {} reloads, depth<={}, compared={}, refusals={} \
         (tampered {}), trimmed={}, oracle-us=(state {} derived {} entries {}), peak={:?}{}{}{}",
        outcome.mutations,
        outcome.rejected,
        outcome.noops,
        outcome.skipped,
        outcome.undos,
        outcome.redos,
        outcome.jumps,
        outcome.saves,
        outcome.reloads,
        outcome.max_depth,
        outcome.compared,
        outcome.refusals,
        outcome.tampered,
        outcome.trimmed,
        outcome.oracle_us.0,
        outcome.oracle_us.1,
        outcome.oracle_us.2,
        outcome.peak,
        outcome
            .config
            .map(DrawnConfig::describe)
            .unwrap_or_default(),
        if outcome.app_panic { " APP-PANIC" } else { "" },
        describe_coverage(outcome),
    )
}

fn describe_coverage(outcome: &Outcome) -> String {
    if !outcome.knobs.any() {
        return String::new();
    }
    let c = outcome.coverage;
    format!(
        "; knobs={} corrupted={} dropped={} kept-corrupted={} refused-corrupted={} \
         trimmed-crossings={} window-cut={} saves-in-place={} double-crossings={} extended={}",
        outcome.knobs.describe(),
        c.corrupted,
        c.dropped,
        c.kept_corrupted,
        c.refused_corrupted,
        c.trimmed_crossings,
        c.window_cut,
        c.saves_in_place,
        c.double_crossings,
        c.extended,
    )
}

/// The per-family table over a set of outcomes, sorted by family.
pub(super) fn coverage_table<'a>(outcomes: impl IntoIterator<Item = &'a Outcome>) -> String {
    let mut total: BTreeMap<&'static str, KindCount> = BTreeMap::new();
    for outcome in outcomes {
        for (kind, count) in &outcome.kinds {
            let entry = total.entry(kind).or_default();
            entry.applied += count.applied;
            entry.pushed += count.pushed;
            entry.rejected += count.rejected;
            entry.noops += count.noops;
            entry.skipped += count.skipped;
        }
    }
    let mut out = format!(
        "  {:<28} {:>7} {:>7} {:>8} {:>6} {:>7}\n",
        "family", "applied", "pushed", "rejected", "no-op", "skipped"
    );
    for (kind, count) in &total {
        writeln!(
            out,
            "  {kind:<28} {:>7} {:>7} {:>8} {:>6} {:>7}",
            count.applied, count.pushed, count.rejected, count.noops, count.skipped
        )
        .expect("a String takes any write");
    }
    out
}

// ===========================================================================
// The env knobs and the seed-parallel runner
// ===========================================================================

pub(super) fn env_flag(name: &str) -> bool {
    std::env::var(format!("STALAGMIGHT_RUST_FUZZ_{name}")).is_ok()
}

pub(super) fn env_number<T: std::str::FromStr>(name: &str, fallback: T) -> T {
    std::env::var(format!("STALAGMIGHT_RUST_FUZZ_{name}"))
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

/// The sweeps' seeds: `SEED_START..SEED_START + SEEDS`.
fn env_seeds(default_count: u32) -> Vec<u32> {
    let start: u32 = env_number("SEED_START", 1);
    let count: u32 = env_number("SEEDS", default_count);
    (start..start.saturating_add(count)).collect()
}

/// `VERBOSE`, latched once per process (a run cannot change mode half way,
/// and the sweeps' workers never race the environment lock).
fn fuzz_verbose() -> bool {
    static VERBOSE: OnceLock<bool> = OnceLock::new();
    *VERBOSE.get_or_init(|| env_flag("VERBOSE"))
}

fn fuzz_trace() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| env_flag("TRACE"))
}

pub(super) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_else(|| "<non-string panic>".to_string())
}

const THREAD_HEADROOM: usize = 1;
const WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Sweep workers: `THREADS`, else the machine's parallelism less one; one
/// under `TRACE` (the trace is a stream).
pub(super) fn fuzz_threads(jobs: usize) -> usize {
    let requested = std::env::var("STALAGMIGHT_RUST_FUZZ_THREADS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    let threads = match requested {
        Some(count) => count,
        None if fuzz_trace() => 1,
        None => std::thread::available_parallelism()
            .map_or(1, |cores| cores.get().saturating_sub(THREAD_HEADROOM)),
    };
    threads.clamp(1, jobs.max(1))
}

/// Run `job` per entry across `threads` workers and return the results IN
/// INPUT ORDER: a run is a pure function of its inputs, so the merged output
/// does not depend on the scheduler. A panic is caught per job.
pub(super) fn run_seed_jobs<J: Sync, R: Send>(
    jobs: &[J],
    threads: usize,
    job: impl Fn(&J) -> R + Sync,
) -> Vec<Result<R, String>> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Result<R, String>)>> =
        Mutex::new(Vec::with_capacity(jobs.len()));
    std::thread::scope(|scope| {
        for worker in 0..threads {
            std::thread::Builder::new()
                .name(format!("fuzz-worker-{worker}"))
                .stack_size(WORKER_STACK_BYTES)
                .spawn_scoped(scope, || loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = jobs.get(index) else {
                        break;
                    };
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(item)))
                            .map_err(|payload| panic_message(&*payload));
                    results
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push((index, result));
                })
                .expect("failed to spawn a fuzz worker thread");
        }
    });
    let mut collected = results.into_inner().unwrap_or_else(PoisonError::into_inner);
    collected.sort_by_key(|(index, _)| *index);
    collected.into_iter().map(|(_, result)| result).collect()
}

// ===========================================================================
// The CI gates
// ===========================================================================

/// Seeds every gate runs. No seed is excluded: nothing is known to fail.
pub(super) const GATE_SEEDS: &[u32] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
/// Moves per gate seed.
pub(super) const GATE_ITERATIONS: usize = 60;

fn gate(label: &str, seeds: &[u32], plan: Plan) -> Vec<Outcome> {
    let mut outcomes = Vec::new();
    for &seed in seeds {
        let outcome = run_fuzz(seed, plan);
        println!("{}", summarize(label, seed, &outcome));
        assert!(
            outcome.mutations > 10,
            "seed {seed} barely mutated ({} mutations)",
            outcome.mutations
        );
        assert!(
            outcome.undos > 3 && outcome.redos > 3,
            "seed {seed} barely walked the history (undos={} redos={})",
            outcome.undos,
            outcome.redos
        );
        assert!(
            outcome.refusals >= 2,
            "seed {seed} probed no refusal ({})",
            outcome.refusals
        );
        outcomes.push(outcome);
    }
    outcomes
}

/// THE GATE, in memory: a regression guard on every `cargo test`.
#[test]
fn undo_redo_fuzz_gate() {
    let _config = isolated_config();
    let outcomes = gate("in-memory", GATE_SEEDS, Plan::plain(GATE_ITERATIONS));
    assert!(
        outcomes.iter().any(|o| o.tampered > 0),
        "no gate seed refused a tampered step - the refusal oracle went unexercised"
    );
}

/// The persistence half: every seed crosses save/close/reopen and walks the
/// RESTORED stacks (persisted entries, minted ids).
#[test]
fn undo_redo_fuzz_gate_persistence() {
    let _config = isolated_config();
    let outcomes = gate(
        "persistence",
        GATE_SEEDS,
        Plan::persisted(GATE_ITERATIONS, 1),
    );
    let reloads: usize = outcomes.iter().map(|o| o.reloads).sum();
    assert!(
        reloads >= GATE_SEEDS.len(),
        "the persistence gate crossed only {reloads} time(s)"
    );
}

/// Several crossings per run: stacks a previous crossing restored, crossed
/// again.
#[test]
fn undo_redo_fuzz_gate_multi_crossing() {
    let _config = isolated_config();
    let outcomes = gate(
        "multi-crossing",
        GATE_SEEDS,
        Plan::persisted(GATE_ITERATIONS, 3),
    );
    let reloads: usize = outcomes.iter().map(|o| o.reloads).sum();
    assert!(
        reloads > GATE_SEEDS.len(),
        "the multi-crossing gate crossed {reloads} time(s) over {} seed(s) - nothing stacked",
        GATE_SEEDS.len()
    );
}

/// Starting from a MIGRATED legacy project (`legacy_origin`): a seeded 0.6.0
/// survey written by the frozen 0.6.0 save, opened through the migration,
/// fuzzed in memory and across crossings.
#[test]
fn undo_redo_fuzz_gate_legacy_origin() {
    let _config = isolated_config();
    for persistence in [false, true] {
        let mut plan = Plan::persisted(GATE_ITERATIONS, 2);
        plan.persistence = persistence;
        plan.legacy_origin = true;
        let outcomes = gate(
            if persistence {
                "legacy-persist"
            } else {
                "legacy-memory"
            },
            &GATE_SEEDS[..6],
            plan,
        );
        if persistence {
            assert!(outcomes.iter().all(|o| o.reloads > 0));
        }
    }
}

const COMBINED_CROSSINGS: usize = 3;
const COMBINED_ITERATIONS: usize = 300;

const fn combined_plan(iterations: usize) -> Plan {
    let mut plan = Plan::persisted(iterations, COMBINED_CROSSINGS);
    plan.jumps = true;
    plan.closures = true;
    plan.closure_shapes = true;
    plan.horizon = true;
    plan.config_fuzz = true;
    plan.combined = true;
    // A drawn limit above the persisted window would refuse every crossing.
    plan.knobs.trim_crossings = true;
    plan
}

/// The combined regime's seeds, chosen for what they drive: a small drawn
/// limit (the window slides from the first moves), jumps and crossings.
const COMBINED_SEEDS: &[u32] = &[2, 7];

/// THE COMBINED GATE: horizon, closures, closure shapes, jumps, three
/// crossings and a drawn `undo_history_limit` in one run, with the structural
/// properties (it trimmed, it crossed more than once, it jumped) asserted so
/// a regime that stopped combining cannot pass by finding nothing.
#[test]
fn undo_redo_fuzz_gate_combined() {
    let _config = isolated_config();
    let (mut reloads, mut trimmed, mut jumps) = (0, 0, 0);
    for &seed in COMBINED_SEEDS {
        let outcome = run_fuzz(seed, combined_plan(COMBINED_ITERATIONS));
        println!("{}", summarize("combined", seed, &outcome));
        reloads += outcome.reloads;
        trimmed += outcome.trimmed;
        jumps += outcome.jumps;
        assert!(
            outcome.mutations > 50,
            "seed {seed} barely mutated ({} mutations)",
            outcome.mutations
        );
    }
    assert!(
        trimmed > 0,
        "no combined seed reached its drawn history limit"
    );
    assert!(
        reloads > COMBINED_SEEDS.len(),
        "the combined gate crossed {reloads} time(s) - nothing stacked"
    );
    assert!(jumps > 0, "the combined gate drew no multi-step jump");
}

/// The coverage per command family over the gate's default seeds, in memory
/// and through the crossings: every family is drawn, and every family that
/// can record an entry recorded one.
#[test]
fn every_command_family_is_covered_over_the_gate_seeds() {
    let _config = isolated_config();
    let mut outcomes = Vec::new();
    for &seed in GATE_SEEDS {
        outcomes.push(run_fuzz(seed, Plan::plain(GATE_ITERATIONS)));
        outcomes.push(run_fuzz(seed, Plan::persisted(GATE_ITERATIONS, 1)));
    }
    let table = coverage_table(&outcomes);
    println!("coverage per command family over the gate seeds (in memory + persistence):\n{table}");
    let mut pushed: BTreeMap<&str, usize> = BTreeMap::new();
    for outcome in &outcomes {
        for (kind, count) in &outcome.kinds {
            *pushed.entry(kind).or_default() += count.pushed;
        }
    }
    let missing: Vec<&&str> = ALL_FAMILIES
        .iter()
        .filter(|kind| pushed.get(**kind).copied().unwrap_or(0) == 0)
        .collect();
    assert!(
        missing.is_empty(),
        "these families never recorded an entry over the gate seeds: {missing:?}\n{table}"
    );
}

/// Every family the draw table holds (the coverage gate's checklist).
pub(super) const ALL_FAMILIES: &[&str] = &[
    "create_line",
    "create_shot",
    "update_shot",
    "set_shot_kind",
    "update_line",
    "delete_shot",
    "delete_line",
    "reverse_line",
    "move_line",
    "move_segment",
    "move_station",
    "station_comment",
    "merge_stations",
    "detach_pair",
    "create_closure",
    "remove_closure",
    "set_datum",
    "remove_datum",
    "split_shot_at_distance",
    "delete_shot_and_split",
    "split_line",
    "combine_lines",
    "delete_last_shot_and_line",
    "delete_range_split",
    "delete_range_shift",
    "batch",
    "edit_shot_credit",
    "unmerge_station",
    "move_comment",
    "project_settings",
    "surface_elevation",
    "untrack_import",
    "import_lines",
    "add_project_file",
    "rename_station",
    "cave_section",
    "create_and_merge",
];

/// Determinism is the property every other guarantee rests on: a seed
/// replays bit for bit, in every gated regime.
#[test]
fn the_fuzzer_is_deterministic() {
    let _config = isolated_config();
    let mut legacy = Plan::persisted(GATE_ITERATIONS, 1);
    legacy.legacy_origin = true;
    let runs = GATE_SEEDS[..4]
        .iter()
        .map(|&seed| (seed, Plan::plain(GATE_ITERATIONS)))
        .chain(
            COMBINED_SEEDS
                .iter()
                .map(|&seed| (seed, combined_plan(COMBINED_ITERATIONS))),
        )
        .chain([(3, legacy)]);
    for (seed, plan) in runs {
        let first = run_fuzz(seed, plan);
        let second = run_fuzz(seed, plan);
        assert_eq!(
            first.shape(),
            second.shape(),
            "seed {seed} did not replay identically (combined={}, legacy={})",
            plan.combined,
            plan.legacy_origin
        );
        assert_eq!(
            first.config, second.config,
            "seed {seed} drew a different config on replay"
        );
    }
}

/// The v2 line diff REPORTS when it hid something: "exactly the cap" and
/// "more than the cap" are different statements.
#[test]
fn the_line_diff_says_when_it_hid_something() {
    let _config = isolated_config();
    let text = |count: usize, from: usize| -> Vec<u8> {
        let mut out = String::from("{\"format\":\"stalx\",\n\"stations\":[\n");
        for i in 0..count {
            writeln!(out, "[{i},\"S{}\"],", i + from).expect("a String takes any write");
        }
        out.push_str("],\n\"end\":true}\n");
        out.into_bytes()
    };
    let (few, hidden) = line_diff(&text(3, 0), &text(3, 100), 6);
    assert_eq!(few.len(), 3);
    assert!(!hidden);
    assert!(few[0].starts_with("at stations (line 2)"), "{}", few[0]);
    let (exact, hidden) = line_diff(&text(6, 0), &text(6, 100), 6);
    assert_eq!(exact.len(), 6);
    assert!(!hidden, "exactly the cap must not read as truncated");
    let (over, hidden) = line_diff(&text(7, 0), &text(7, 100), 6);
    assert_eq!(over.len(), 6, "the reported list stays capped");
    assert!(hidden, "one past the cap must report as truncated");
    // A line one side lacks is a difference too.
    let (shorter, _) = line_diff(&text(3, 0), &text(2, 0), 6);
    assert!(shorter
        .iter()
        .any(|d| d.contains("<absent>") || d.contains("S2")));
}

/// A move that does NOT RETURN is the move the failure names, and an app
/// panic's own message reaches the signature as its discriminator.
#[test]
fn a_move_that_panics_is_the_move_the_failure_tail_names() {
    let _config = isolated_config();
    let mut fuzzer = Fuzzer::new(1, false);
    fuzzer.note("createShot(s=8,append) -> ok (undo 0->1, redo 0->0)".to_string());
    fuzzer.starting_move("deleteShot(8:800002)".to_string());
    let mid_move = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fuzzer.fail("something failed mid-move", "  detail")
    }))
    .expect_err("`fail` returned");
    let mid_move = panic_message(&*mid_move);
    assert!(
        mid_move
            .lines()
            .last()
            .is_some_and(|line| line.contains("deleteShot(8:800002) -> IN FLIGHT")),
        "the in-flight move is not the LAST line of the tail:\n{mid_move}"
    );
    let absorbed = fuzzer
        .catching(|_| -> () { panic!("delete_shot_range: index 7 out of range for 5 shots") });
    assert!(absorbed.is_none());
    assert!(fuzzer.outcome.app_panic);
    let flushed = fuzzer
        .log
        .iter()
        .position(|line| line.contains("deleteShot(8:800002)"))
        .expect("the in-flight move was never written to the log");
    assert!(fuzzer.log[flushed].contains("PANICKED"));
    assert!(fuzzer.log[flushed + 1].contains("^ the app panicked"));
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fuzzer.fail(
            "the app panicked inside a move",
            "  see the last move in the log below for the panic's own message",
        )
    }))
    .expect_err("`fail` returned");
    let message = panic_message(&*payload);
    assert_eq!(
        failure_signature(&message),
        "the app panicked inside a move | panic=delete_shot_range"
    );
    // The same panic at other values is one bucket.
    let other = message.replace("index 7 out of range for 5", "index 12 out of range for 9");
    assert_eq!(failure_signature(&message), failure_signature(&other));
}

/// BUG 7's regression guard: `undo_to(N)` is N x `undo()` (every jump of a
/// jumps run is re-walked in single steps by `probe_jump_equivalence`).
#[test]
fn bug_7_a_jump_is_n_single_steps() {
    let _config = isolated_config();
    let mut plan = Plan::persisted(150, 1);
    plan.jumps = true;
    plan.closures = true;
    plan.closure_shapes = true;
    let outcome = run_fuzz(6, plan);
    println!("{}", summarize("bug-7", 6, &outcome));
    assert!(outcome.jumps > 0, "seed 6 drew no jump");
}

/// Which seeds are clean, and what the rest fail on. Prints a per-seed line
/// for both modes, the clean list, the failure buckets and the coverage per
/// family. The triage tool for `GATE_SEEDS`.
#[test]
#[ignore = "survey: prints per-seed status, used to choose GATE_SEEDS"]
fn fuzz_seed_survey() {
    let _config = isolated_config();
    let iterations: usize = env_number("ITERATIONS", GATE_ITERATIONS);
    let seeds = env_seeds(40);
    let verbose = fuzz_verbose();
    let jobs: Vec<(u32, bool)> = seeds
        .iter()
        .flat_map(|&seed| [(seed, false), (seed, true)])
        .collect();
    let results = run_seed_jobs(&jobs, fuzz_threads(jobs.len()), |&(seed, persistence)| {
        run_fuzz(seed, Plan::from_env(iterations, persistence))
    });
    let mut clean: Vec<u32> = seeds;
    let mut failures: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut outcomes = Vec::new();
    for (&(seed, persistence), outcome) in jobs.iter().zip(results) {
        match outcome {
            Ok(result) => {
                println!(
                    "{}",
                    summarize(
                        if persistence { "persist" } else { "memory " },
                        seed,
                        &result
                    )
                );
                outcomes.push(result);
            }
            Err(message) => {
                if verbose {
                    println!(
                        "--- seed {seed} ({}) panicked ---\n{message}",
                        if persistence { "persist" } else { "memory" }
                    );
                }
                failures
                    .entry(failure_signature(&message))
                    .or_default()
                    .push(seed);
                clean.retain(|&s| s != seed);
            }
        }
    }
    println!("clean seeds ({}): {clean:?}", clean.len());
    for (headline, seeds) in &failures {
        println!("FAILING {} seed(s) on: {headline}", seeds.len());
        println!("  seeds: {seeds:?}");
    }
    println!(
        "coverage per command family:\n{}",
        coverage_table(&outcomes)
    );
}

/// CORPUS-GUIDED sweeping, and the measurement that says whether it is
/// worth anything: the same budget with and without guidance, distinct
/// topology signatures reached by each. On the old model the answer was NO
/// (78 against 73 distinct signatures at 80 runs x 60 moves: novelty admits
/// nearly everything, so guidance only spends half the budget replaying a
/// prefix); `fuzz_rarity_tests.rs` measured the rare-feature alternative and
/// it was NO again. The mechanism is kept because it costs two integers;
/// it is wired into no gate.
#[test]
#[ignore = "measurement: compares signature coverage with corpus guidance on and off"]
fn fuzz_corpus_sweep() {
    let _config = isolated_config();
    let budget: usize = env_number("CORPUS_BUDGET", 80);
    let iterations: usize = env_number("ITERATIONS", GATE_ITERATIONS);
    let base = |prologue: Option<(u32, usize)>| Plan {
        prologue,
        ..Plan::plain(iterations)
    };
    let mut unguided: BTreeSet<String> = BTreeSet::new();
    for seed in 1..=budget as u32 {
        if let Ok(outcome) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_fuzz(seed, base(None))))
        {
            unguided.insert(outcome.signature);
        }
    }
    let mut guided: BTreeSet<String> = BTreeSet::new();
    let mut corpus: Vec<(u32, usize)> = Vec::new();
    let mut picker = Rng::new(0x00C0_FFEE);
    for step in 0..budget {
        let seed = step as u32 + 1;
        let prologue = if corpus.is_empty() || step % 2 == 0 {
            None
        } else {
            Some(corpus[picker.below(corpus.len())])
        };
        let Ok(outcome) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_fuzz(seed, base(prologue))
        })) else {
            continue;
        };
        if guided.insert(outcome.signature.clone()) {
            corpus.push((
                prologue.map_or(seed, |(prologue_seed, _)| prologue_seed),
                iterations / 2,
            ));
        }
    }
    println!("corpus sweep: budget {budget} run(s) x {iterations} moves, each way");
    println!("  UNGUIDED: {} distinct signature(s)", unguided.len());
    println!(
        "  GUIDED:   {} distinct signature(s), corpus grew to {}",
        guided.len(),
        corpus.len()
    );
    println!(
        "  reached ONLY with guidance: {}",
        guided.difference(&unguided).count()
    );
    println!(
        "  reached ONLY without:       {}",
        unguided.difference(&guided).count()
    );
}

/// THE HORIZON RUN: thousands of moves past the history limit - the FIFO
/// trim and its sliding window, growth, and the counters over a long life.
#[test]
#[ignore = "horizon: thousands of moves per seed - run deliberately with STALAGMIGHT_RUST_FUZZ_SEEDS/ITERATIONS"]
fn undo_redo_fuzz_horizon() {
    let _config = isolated_config();
    let seeds = env_seeds(2);
    let iterations: usize = env_number("ITERATIONS", 10_000);
    let persistence = env_flag("PERSISTENCE");
    let mut plan = Plan::from_env(iterations, persistence);
    plan.horizon = true;
    plan.jumps = true;
    // A horizon history is far deeper than the persisted window: model the
    // window, or no crossing is ever taken.
    plan.knobs.trim_crossings |= persistence;
    let results = run_seed_jobs(&seeds, fuzz_threads(seeds.len()), |&seed| {
        run_fuzz(seed, plan)
    });
    let mut trimmed = 0;
    let mut failures = Vec::new();
    for (&seed, result) in seeds.iter().zip(results) {
        match result {
            Ok(outcome) => {
                println!("{}", summarize("horizon", seed, &outcome));
                trimmed += outcome.trimmed;
            }
            Err(message) => {
                println!("horizon seed={seed} FAILED:\n{message}");
                failures.push(seed);
            }
        }
    }
    assert!(trimmed > 0, "no horizon run reached the history limit");
    assert!(
        failures.is_empty(),
        "horizon seed(s) {failures:?} failed - see the payloads above"
    );
}

/// The SOAK: the same engine at a scale no gate can afford. Both modes per
/// seed, crossings, jumps, closures and the constructed shapes.
#[test]
#[ignore = "soak: minutes, not seconds - run deliberately with STALAGMIGHT_RUST_FUZZ_SEEDS/ITERATIONS"]
fn undo_redo_fuzz_soak() {
    let _config = isolated_config();
    let seeds = env_seeds(50);
    let iterations: usize = env_number("ITERATIONS", 1000);
    let jobs: Vec<(u32, bool)> = seeds
        .iter()
        .flat_map(|&seed| [(seed, false), (seed, true)])
        .collect();
    let results = run_seed_jobs(&jobs, fuzz_threads(jobs.len()), |&(seed, persistence)| {
        let mut plan = Plan::from_env(iterations, persistence);
        plan.crossings = plan.crossings.max(3);
        plan.jumps = true;
        plan.closures = true;
        plan.closure_shapes = true;
        plan.divergence = plan.divergence || persistence;
        run_fuzz(seed, plan)
    });
    let mut failures = Vec::new();
    let mut outcomes = Vec::new();
    for (&(seed, persistence), result) in jobs.iter().zip(results) {
        match result {
            Ok(outcome) => outcomes.push(outcome),
            Err(message) => {
                println!(
                    "soak seed={seed} ({}) FAILED:\n{message}",
                    if persistence { "persist" } else { "memory" }
                );
                failures.push(seed);
            }
        }
    }
    println!(
        "soak: {} seed(s) x {iterations} iterations x 2 modes",
        seeds.len()
    );
    println!(
        "coverage per command family:\n{}",
        coverage_table(&outcomes)
    );
    assert!(
        failures.is_empty(),
        "soak seed(s) {failures:?} failed - see the payloads above"
    );
}

/// Is the Computed View a FUNCTION of the survey? Re-solve one state many
/// times, through the memo (invalidated) and fresh, and report the spread -
/// the measurement the bit-exact derived oracle rests on.
#[test]
#[ignore = "measurement: prints the solve-to-solve spread the derived oracle relies on"]
fn measure_adjusted_solve_determinism() {
    let _config = isolated_config();
    // The loop the CLOSURES project starts with, plus soft closures across
    // it: the least-squares solve itself runs, not only the raw walk.
    let fuzzer = Fuzzer::new(1, true);
    let lines: Vec<LineId> = fuzzer.live().lines().iter().map(|l| l.id).collect();
    for (a, b, strategy) in [
        ((lines[0], 1), (lines[4], 1), ClosureStrategy::Batch),
        (
            (lines[3], 1),
            (lines[0], 2),
            ClosureStrategy::LeastSquaresWeighted,
        ),
    ] {
        let r = fuzzer.state.execute(Command::CreateLoopClosure {
            station_a: StationRef::new(a.0, a.1),
            station_b: StationRef::new(b.0, b.1),
            closure_strategy: strategy,
        });
        println!("closure {a:?} - {b:?} ({strategy:?}): {:?}", r.error);
    }
    let project = fuzzer.live();
    let first = derived_text(&project, &fuzzer.adjust);
    let mut differing = 0;
    for _ in 0..64 {
        fuzzer.state.bump_revision();
        let memo = memo_derived_text(&fuzzer.state).unwrap_or_default();
        let fresh = derived_text(&project, &fuzzer.adjust);
        if memo != first || fresh != first {
            differing += 1;
        }
    }
    println!(
        "{differing} of 64 re-solves differ from the first ({} closures, {} lines)",
        project.closures.len(),
        project.lines().len()
    );
    assert!(
        project.closures.len() >= 2,
        "the soft closures were refused"
    );
    assert_eq!(differing, 0, "the solve is not a function of the survey");
}
