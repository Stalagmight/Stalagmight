//! Shared test kit of the Split Line Here and Combine Lines suites
//! (`split_line_tests`, `combine_lines_tests`, `issue_738_tests`,
//! `issue_756_tests`): surveys built on the station-graph model with its own
//! operations (so every anchor rule holds), written as a `.stalx` v2 file and
//! opened through `AppState` the way the app opens one, and the canonical
//! snapshot an undo or a redo must give back exactly.
//!
//! All data is synthetic; every file goes to a temporary directory.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::survey::{
    ClosureId, ClosureStrategy, EditContext, LineMeta, Project, StartAt, ZeroDatum,
};
use crate::models::{GpsCoordinates, LineId, ShotData, StationId, StationRef};
use crate::persistence::stalx_v2::{self, Extras, Mode};

/// A flat shot (no depth change).
pub(super) fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..ShotData::default()
    }
}

/// A shot with its depth readings.
pub(super) fn deep(azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    ShotData {
        depth_in,
        depth_out,
        ..shot(azimuth, length)
    }
}

pub(super) fn meta(name: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        color: "#ff0000".to_string(),
        ..LineMeta::default()
    }
}

/// A new project with nothing in it.
pub(super) fn empty(name: &str) -> Project {
    Project::new(name, None)
}

/// A line on a new piece of its own, its start placed at `at`.
pub(super) fn line_at(p: &mut Project, name: &str, at: [f64; 2], shots: Vec<ShotData>) -> LineId {
    p.create_line(meta(name), StartAt::New, shots, Some(at))
        .unwrap()
        .value
}

/// A line starting on the existing station `start` (a junction there).
pub(super) fn line_from(
    p: &mut Project,
    name: &str,
    start: StationId,
    shots: Vec<ShotData>,
) -> LineId {
    p.create_line(meta(name), StartAt::Station(start), shots, None)
        .unwrap()
        .value
}

/// The station at `index` along `line`.
pub(super) fn st(p: &Project, line: LineId, index: u32) -> StationId {
    p.topology()
        .station_at(StationRef::new(line, index))
        .unwrap()
}

pub(super) fn slot(line: LineId, index: u32) -> StationRef {
    StationRef::new(line, index)
}

/// A loop closure between two slots through Create Loop Closure's own
/// operation (its refusals included).
pub(super) fn close(
    p: &mut Project,
    a: StationRef,
    b: StationRef,
    strategy: ClosureStrategy,
) -> ClosureId {
    p.create_closure_at(a, b, strategy)
        .unwrap()
        .value
        .closure
        .id
}

pub(super) fn gps(latitude: f64, longitude: f64) -> GpsCoordinates {
    GpsCoordinates::new(latitude, longitude, 0.0).unwrap()
}

/// Put a zero datum on `station` as Set Zero Datum would (the anchor
/// algorithm places its piece on it, or keeps it a GPS fix).
pub(super) fn add_datum(p: &mut Project, station: StationId, at: GpsCoordinates) {
    let before = p.anchor_snapshot();
    p.push_datum(ZeroDatum::new(station, at));
    p.settle_anchors(&before, &EditContext::none());
}

/// Put a zero datum on `station` that holds its piece where it is (a
/// declared correction, as Add .stalx leaves one): two pieces each placed by
/// their own datum, side by side.
pub(super) fn hold(p: &mut Project, station: StationId, at: GpsCoordinates) {
    let here = p.raw_positions().xy(station).unwrap();
    add_datum(p, station, at);
    let i = p.datums.len() - 1;
    p.datums[i].correction = p.correction_to_hold(i, here);
    assert!(
        apart(p.raw_positions().xy(station).unwrap(), here) < 1e-9,
        "setup: held"
    );
}

/// The raw (measured) position of a slot.
pub(super) fn xy(p: &Project, at: StationRef) -> [f64; 2] {
    let raw = p.raw_positions();
    let [x, y, _] = raw.slot(p, at).unwrap();
    [x, y]
}

pub(super) fn apart(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Every slot's raw position, lines in list order.
pub(super) fn positions(p: &Project) -> Vec<(StationRef, [f64; 2])> {
    p.raw_positions()
        .slots(p)
        .into_iter()
        .map(|s| (s.slot, [s.x, s.y]))
        .collect()
}

/// A survey written as a `.stalx` v2 file and opened through `AppState`.
pub(super) struct Opened {
    pub(super) state: AppState,
    pub(super) path: PathBuf,
    _dir: TempDir,
}

/// Write `p` as a v2 file and open it (a valid model only: the encoder
/// refuses through `validate`).
pub(super) fn open(p: &Project) -> Opened {
    assert!(p.validate().is_empty(), "setup: {:?}", p.validate());
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("survey.stalx");
    std::fs::write(
        &path,
        stalx_v2::encode(p, &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject { path: path.clone() });
    assert!(r.success, "open: {:?}", r.error);
    Opened {
        state,
        path,
        _dir: dir,
    }
}

pub(super) fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

/// The whole survey as the file writes it, less what a step or a reopen may
/// change by design (`modified_at`, the path), plus every shot's session id
/// (an in-session undo gives them back exactly).
pub(super) fn snapshot(state: &AppState) -> (String, Vec<Vec<u32>>) {
    let mut p = project(state);
    let ids = p
        .lines()
        .iter()
        .map(|l| l.shots.iter().map(|s| s.id).collect())
        .collect();
    p.modified_at = p.created_at;
    p.file_path = None;
    let text =
        String::from_utf8(stalx_v2::encode(&p, &Extras::default(), Mode::Normal).unwrap()).unwrap();
    (text, ids)
}

/// [`snapshot`] without the session ids, for comparisons across a reopen.
pub(super) fn canonical(state: &AppState) -> String {
    snapshot(state).0
}

/// The undo history, newest first: `(command type, description)`.
pub(super) fn history(state: &AppState) -> Vec<(String, String)> {
    state
        .get_undo_stack()
        .into_iter()
        .map(|item| (item.command_type, item.description))
        .collect()
}

/// The newest entry's preview rows: `(field, old, new)`.
pub(super) fn preview_rows(state: &AppState) -> Vec<(String, String, String)> {
    state
        .get_undo_stack()
        .into_iter()
        .next()
        .and_then(|item| item.preview_data)
        .map(|preview| {
            preview
                .changes
                .into_iter()
                .map(|c| (c.field, c.old_value, c.new_value))
                .collect()
        })
        .unwrap_or_default()
}

/// Save the open project to its file, close it, and open the file again in
/// the same `AppState`.
pub(super) fn save_close_reopen(state: &AppState, path: &Path) {
    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "save: {:?}", saved.error);
    assert!(state.execute(Command::CloseProject).success);
    let r = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(r.success, "reopen: {:?}", r.error);
}

/// One undo and one redo land exactly on `before` and `after`.
pub(super) fn assert_undo_redo_exact(
    state: &AppState,
    before: &(String, Vec<Vec<u32>>),
    after: &(String, Vec<Vec<u32>>),
    what: &str,
) {
    let r = state.undo();
    assert!(r.success, "{what}: undo {:?}", r.error);
    assert_same(
        &snapshot(state),
        before,
        &format!("{what}: one undo restores it all"),
    );
    let r = state.redo();
    assert!(r.success, "{what}: redo {:?}", r.error);
    assert_same(
        &snapshot(state),
        after,
        &format!("{what}: one redo re-applies it"),
    );
}

/// The id allocators of a snapshot's `counters` line, by name.
fn counters(line: &str) -> std::collections::BTreeMap<String, u64> {
    let object = line
        .trim_start_matches("\"counters\":")
        .trim_end_matches(',');
    serde_json::from_str(object).unwrap()
}

/// Two snapshots are equal, to the bit, except that the id allocators may
/// be higher than wanted (plan 8.1 and 11.4: a step applies the recorded
/// counters as `max(live, recorded)`, so an undo never hands an id out
/// twice); on a difference, the first differing line of the file text (or
/// the session ids) says where.
pub(super) fn assert_same(
    got: &(String, Vec<Vec<u32>>),
    want: &(String, Vec<Vec<u32>>),
    what: &str,
) {
    let (g, w): (Vec<&str>, Vec<&str>) = (got.0.lines().collect(), want.0.lines().collect());
    assert_eq!(g.len(), w.len(), "{what}: the survey differs in length");
    for (g, w) in g.iter().zip(&w) {
        if g == w {
            continue;
        }
        if g.starts_with("\"counters\":") && w.starts_with("\"counters\":") {
            let (g, w) = (counters(g), counters(w));
            assert_eq!(
                g.keys().collect::<Vec<_>>(),
                w.keys().collect::<Vec<_>>(),
                "{what}"
            );
            assert!(
                g.iter().all(|(k, v)| *v >= w[k]),
                "{what}: an allocator went back: {g:?} {w:?}"
            );
            continue;
        }
        assert_eq!(g, w, "{what}: the survey differs");
    }
    assert_eq!(got.1, want.1, "{what}: session shot ids");
}
