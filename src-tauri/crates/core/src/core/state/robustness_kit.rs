//! Shared kit of the robustness suites (`adversarial_tests`,
//! `concurrency_tests`, `crash_property_tests`, `format_property_tests`): a
//! seeded generator, a random survey built through the command path, the live
//! targets a move is aimed at, and the oracles every suite compares a state on
//! (the canonical v2 text, the integrity verdict, the Closures Computed View,
//! the id allocators and both history stacks).
//!
//! The generator (`Rng`, `random_shot`, `random_offset`) is the undo fuzzer's
//! (`fuzz_undo_tests`), re-exported. The starting survey and the targets stay
//! this kit's own: the robustness suites start without the undo fuzzer's
//! standing junction (line 6 on line 3) and, with no loop, without station
//! comments, and they aim at slots and at shots by sorted session id, so
//! their seeds keep the trajectories they were swept on. All data is
//! synthetic; nothing reads a fixture.

use std::collections::BTreeMap;
use std::path::Path;

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::survey::Project;
use crate::models::{
    ClosureStrategy, GpsCoordinates, LineId, ShotData, ShotId, StationRef, SurveyDirection,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};

// ===========================================================================
// The generator
// ===========================================================================

// The undo fuzzer's generator, shared: mulberry32 (integer-only, so a seed
// replays bit for bit), its shot, its move offset and its input bounds. The
// robustness seeds follow the undo fuzzer's stream, so a change to these
// moves both suites' seeds.
pub(super) use super::fuzz_undo_tests::{
    random_offset, random_shot, Rng, COLORS, GEOMETRY_EXTREME,
};

/// Shot counts of the starting lines: an empty line and single-shot lines on
/// purpose (both have carried findings).
const SHAPE: &[usize] = &[3, 0, 1, 4, 2, 1, 3];

fn must(result: &CommandResult, what: &str) {
    assert!(result.success, "setup {what} failed: {:?}", result.error);
}

/// A new project built through the command path: lines of varied length
/// (empty and single-shot ones included, some with no metadata), a zero
/// datum on the first line, and with `closures` two junctions chaining lines
/// 0, 3 and 4 plus a loop closure across that chain and two station comments.
pub(super) fn build_project(state: &AppState, rng: &mut Rng, closures: bool) -> Vec<LineId> {
    must(
        &state.execute(Command::CreateProject {
            name: "RustFuzz".to_string(),
            location: None,
        }),
        "CreateProject",
    );
    let mut ids = Vec::new();
    for (index, count) in SHAPE.iter().enumerate() {
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
        must(&result, "CreateLine");
        ids.push(result.line_id.unwrap_or_default());
    }
    must(
        &state.execute(Command::SetZeroDatum {
            station_ref: StationRef::new(ids[0], 0),
            coordinates: GpsCoordinates::new(20.4, -87.3, 0.0).expect("a valid GPS"),
            accuracy_m: None,
            entrance: None,
        }),
        "SetZeroDatum",
    );
    if !closures {
        return ids;
    }
    for (left, right) in [(0_usize, 3_usize), (3, 4)] {
        must(
            &state.execute(Command::MergeStations {
                station_a: StationRef::new(ids[left], index_of(SHAPE[left])),
                station_b: StationRef::new(ids[right], 0),
            }),
            "MergeStations",
        );
    }
    must(
        &state.execute(Command::CreateLoopClosure {
            station_a: StationRef::new(ids[0], 0),
            station_b: StationRef::new(ids[4], index_of(SHAPE[4])),
            closure_strategy: ClosureStrategy::Move,
        }),
        "CreateLoopClosure",
    );
    for index in [0_usize, 5] {
        must(
            &state.execute(Command::SetStationComment {
                station_ref: StationRef::new(ids[index], 0),
                comment: Some(format!("station-note-{index}")),
            }),
            "SetStationComment",
        );
    }
    ids
}

/// A count as a station index.
pub(super) fn index_of(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

// ===========================================================================
// Live targets
// ===========================================================================

/// What the generators aim at, read out of the live project and sorted (so a
/// seed's trajectory depends on the survey, not on any container order).
#[derive(Default)]
pub(super) struct Targets {
    /// `(line, [session shot id, ...])`, ascending.
    pub(super) lines: Vec<(LineId, Vec<ShotId>)>,
    /// Every slot `(line, index)`, ascending.
    pub(super) stations: Vec<(LineId, u32)>,
    /// Loop closure ids, ascending.
    pub(super) closures: Vec<u64>,
}

impl Targets {
    pub(super) fn of(project: &Project) -> Self {
        let mut lines: Vec<(LineId, Vec<ShotId>)> = project
            .lines()
            .iter()
            .map(|line| {
                let mut shots: Vec<ShotId> = line.shots.iter().map(|s| s.id).collect();
                shots.sort_unstable();
                (line.id, shots)
            })
            .collect();
        lines.sort_by_key(|entry| entry.0);
        let mut stations = Vec::new();
        for (id, shots) in &lines {
            for index in 0..=shots.len() {
                stations.push((*id, index_of(index)));
            }
        }
        let mut closures: Vec<u64> = project.closures.iter().map(|c| c.id).collect();
        closures.sort_unstable();
        Self {
            lines,
            stations,
            closures,
        }
    }

    /// A random `(line, shot)` of a line that has shots.
    pub(super) fn pick_shot(&self, rng: &mut Rng) -> Option<(LineId, ShotId)> {
        let populated: Vec<&(LineId, Vec<ShotId>)> = self
            .lines
            .iter()
            .filter(|(_, shots)| !shots.is_empty())
            .collect();
        if populated.is_empty() {
            return None;
        }
        let (line, shots) = populated[rng.below(populated.len())];
        Some((*line, shots[rng.below(shots.len())]))
    }
}

/// The targets of the open project, or `None` with no project open.
pub(super) fn read_targets(state: &AppState) -> Option<Targets> {
    state.get_project_state().map(|p| Targets::of(&p))
}

// ===========================================================================
// The oracles
// ===========================================================================

/// The survey as the file writes it, less what a step or a reopen changes by
/// design (`modified_at`, the path): the encoder's canonical text with empty
/// extras.
pub(super) fn canonical(project: &Project) -> String {
    let mut p = project.clone();
    p.modified_at = p.created_at;
    p.file_path = None;
    match stalx_v2::encode(&p, &Extras::default(), Mode::Normal) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => format!("<the encoder refused the survey: {e}>"),
    }
}

/// [`canonical`] of the open project (empty with none open).
pub(super) fn canonical_snapshot(state: &AppState) -> String {
    state
        .get_project_state()
        .map_or_else(String::new, |p| canonical(&p))
}

/// The model's integrity verdict (`integrity_violation_messages`), messages
/// sorted, in the format the retired `check_project_integrity` answer gave.
pub(super) fn integrity_verdict(state: &AppState) -> String {
    let mut messages = state.integrity_violation_messages();
    messages.sort();
    format!(
        "save_blocked={} messages={messages:?}",
        !messages.is_empty()
    )
}

/// The Closures Computed View per slot, bit for bit, sorted by slot (the solve
/// is deterministic, so no tolerance applies).
pub(super) fn adjusted_geometry_of(state: &AppState) -> String {
    let Some(stations) = state.get_adjusted_stations() else {
        return String::new();
    };
    let mut rows: Vec<String> = stations
        .iter()
        .map(|s| {
            format!(
                "{}:{} {:?} {:?} {:?}",
                s.station_ref.line_id,
                s.station_ref.station_index,
                s.coordinates.x,
                s.coordinates.y,
                s.coordinates.z
            )
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

/// Every id allocator of `project`: the stored counters, the derived next line
/// id, and each line's session shot allocator.
pub(super) fn allocators(project: &Project) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    out.insert(
        "next_station_id".to_string(),
        u64::from(project.counters.next_station_id),
    );
    out.insert(
        "next_closure_id".to_string(),
        project.counters.next_closure_id,
    );
    out.insert(
        "next_section_id".to_string(),
        u64::from(project.counters.next_section_id),
    );
    out.insert(
        "next_line_id".to_string(),
        u64::from(project.next_line_id()),
    );
    for line in project.lines() {
        out.insert(
            format!("line {}.next_shot_id", line.id),
            u64::from(line.next_shot_id),
        );
    }
    out
}

/// Everything a state is compared on.
#[derive(Clone)]
pub(super) struct Observed {
    pub(super) stored: String,
    pub(super) verdict: String,
    pub(super) adjusted: String,
    pub(super) allocators: BTreeMap<String, u64>,
    pub(super) undo_len: usize,
    pub(super) redo_len: usize,
}

pub(super) fn observe(state: &AppState) -> Observed {
    Observed {
        stored: canonical_snapshot(state),
        verdict: integrity_verdict(state),
        adjusted: adjusted_geometry_of(state),
        allocators: state
            .get_project_state()
            .map(|p| allocators(&p))
            .unwrap_or_default(),
        undo_len: state.get_undo_stack().len(),
        redo_len: state.get_redo_stack().len(),
    }
}

/// The first line where two texts differ, with a few lines of each side.
pub(super) fn first_difference(left: &str, right: &str) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (left.lines().collect(), right.lines().collect());
    let at = a
        .iter()
        .zip(&b)
        .position(|(x, y)| x != y)
        .unwrap_or_else(|| a.len().min(b.len()));
    let show = |lines: &[&str]| {
        lines
            .iter()
            .skip(at)
            .take(3)
            .map(|l| l.chars().take(240).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n        ")
    };
    format!(
        "line {} of {} / {}:\n      before: {}\n      after:  {}",
        at + 1,
        a.len(),
        b.len(),
        show(&a),
        show(&b)
    )
}

/// Every way `after` differs from `before`, named per oracle (empty: the same
/// state as far as anything can see).
pub(super) fn differences(before: &Observed, after: &Observed) -> Vec<String> {
    let mut out = Vec::new();
    if before.stored != after.stored {
        out.push(format!(
            "the survey differs at {}",
            first_difference(&before.stored, &after.stored)
        ));
    }
    if before.verdict != after.verdict {
        out.push(format!(
            "the integrity verdict differs:\n      before: {}\n      after:  {}",
            before.verdict, after.verdict
        ));
    }
    if before.adjusted != after.adjusted {
        out.push(format!(
            "the Closures Computed View differs at {}",
            first_difference(&before.adjusted, &after.adjusted)
        ));
    }
    if before.allocators != after.allocators {
        out.push(format!(
            "an id allocator differs:\n      before: {:?}\n      after:  {:?}",
            before.allocators, after.allocators
        ));
    }
    if (before.undo_len, before.redo_len) != (after.undo_len, after.redo_len) {
        out.push(format!(
            "the history stacks differ: undo {} -> {}, redo {} -> {}",
            before.undo_len, after.undo_len, before.redo_len, after.redo_len
        ));
    }
    out
}

/// The text of a panic payload, first line only.
pub(super) fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    super::fuzz_undo_tests::panic_message(payload)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Open `path` in a fresh `AppState` through the executor's open; `None` when
/// the open is refused.
pub(super) fn open_fresh(path: &Path) -> Option<AppState> {
    let state = AppState::new().ok()?;
    state
        .execute(Command::OpenProject {
            path: path.to_path_buf(),
        })
        .success
        .then_some(state)
}

/// A legacy (SQLite) project at `path`, written by the 0.6.0 save path: three
/// lines of random shots, the second line's start merged onto the first
/// line's end, and a datum at the first line's start.
pub(super) fn write_legacy(path: &Path, rng: &mut Rng, name: &str) {
    use super::legacy_fixture_kit::{frozen, old, slot, write_legacy_file, Legacy};
    let mut p = Legacy::new(name.to_string(), None);
    let mut ids = Vec::new();
    for i in 0..3 {
        let shots: Vec<ShotData> = (0..2 + rng.below(3)).map(|_| random_shot(rng)).collect();
        ids.push(
            p.add_line(old::LineData {
                name: Some(format!("L{i}")),
                color: (*rng.pick(COLORS)).to_string(),
                shots: shots.iter().map(frozen).collect(),
                ..Default::default()
            })
            .id,
        );
    }
    let end = index_of(p.get_line(ids[0]).map_or(0, |l| l.shots.len()));
    p.merge_stations(slot(ids[0], end), slot(ids[1], 0))
        .expect("a legacy merge");
    p.set_zero_datum(
        slot(ids[0], 0),
        old::GpsCoordinates::new(20.4, -87.3, 0.0).expect("a valid GPS"),
    )
    .expect("a legacy datum");
    write_legacy_file(path, &p);
}

/// The full load's progress events go nowhere.
pub(super) struct NullSink;

impl crate::core::events::EventSink for NullSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}

    fn with_app_state(&self, _f: Box<dyn FnOnce(&AppState) + Send>) -> bool {
        false
    }
}

/// Whether the derived views the app memoised for the open project are the
/// ones a fresh computation gives (the raw layout per slot, bit for bit, and
/// the Computed View); `Err` says what differs. `Ok(false)` with no project.
pub(super) fn derived_cache_is_coherent(state: &AppState) -> Result<bool, String> {
    let config = state.get_config().closure_adjustment;
    let before = state.current_revision();
    let cached_adjusted = adjusted_geometry_of(state);
    let guard = state
        .project
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(project) = guard.as_ref() else {
        return Ok(false);
    };
    // The revision only grows: equal means no edit landed since `before`, so
    // the memoised Computed View read above describes this project.
    let unchanged = state.current_revision() == before;
    let cached = state.raw_positions_of(project);
    let fresh = project.raw_positions();
    let rows = |raw: &crate::models::survey::RawPositions| -> Vec<String> {
        raw.slots(project)
            .iter()
            .map(|s| format!("{:?} {:?} {:?} {:?}", s.slot, s.x, s.y, s.z))
            .collect()
    };
    let (cached_rows, fresh_rows) = (rows(&cached), rows(&fresh));
    if cached_rows != fresh_rows {
        return Err(format!(
            "the memoised raw layout is not the survey's (revision {}): {}",
            state.current_revision(),
            first_difference(&fresh_rows.join("\n"), &cached_rows.join("\n"))
        ));
    }
    let adjusted = project.adjusted_positions_from(&fresh, &config);
    let mut fresh_adjusted: Vec<String> = project
        .adjusted_stations_wire(&adjusted)
        .iter()
        .map(|s| {
            format!(
                "{}:{} {:?} {:?} {:?}",
                s.station_ref.line_id,
                s.station_ref.station_index,
                s.coordinates.x,
                s.coordinates.y,
                s.coordinates.z
            )
        })
        .collect();
    fresh_adjusted.sort();
    drop(guard);
    let fresh_adjusted = fresh_adjusted.join("\n");
    if unchanged && cached_adjusted != fresh_adjusted {
        return Err(format!(
            "the memoised Computed View is not the survey's (revision {before}): {}",
            first_difference(&fresh_adjusted, &cached_adjusted)
        ));
    }
    Ok(true)
}

/// Every file in `dir` (not descending), sorted.
pub(super) fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}
