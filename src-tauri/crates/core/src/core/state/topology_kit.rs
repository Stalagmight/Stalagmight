//! Shared test kit of the station-graph command suites: the topology suites
//! (`topology_commands_tests`, `multi_datum_tests`, `datum_entrance_tests`,
//! `datum_ui_tests`, `station_identity_tests`), the Split Line Here and
//! Combine Lines suites (`split_line_tests`, `combine_lines_tests`,
//! `issue_738_tests`, `issue_756_tests`) and the undo suites that compare
//! snapshots (`undo_pipeline_tests`, `open_undo_bugs_tests`,
//! `undo_size_gate_tests`, `misc_state_tests`). It gives surveys built on the
//! station-graph model with its own operations (so every anchor rule holds),
//! written as a `.stalx` v2 file and opened through `AppState` the way the
//! app opens one, the commands run through `execute`, and the canonical
//! snapshot an undo or a redo must give back exactly.
//!
//! All data is synthetic; every file goes to a temporary directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::survey::{
    ClosureId, ClosureStrategy, EditContext, LineMeta, Project, StartAt, ZeroDatum,
};
use crate::models::{local_to_gps, GpsCoordinates, LineId, ShotData, StationId, StationRef};
use crate::persistence::stalx_v2::{self, Extras, Mode};

/// The first entrance's GPS in every datum suite.
pub(super) const ORIGIN: (f64, f64) = (20.5, -87.25);

pub(super) fn origin() -> GpsCoordinates {
    GpsCoordinates::new(ORIGIN.0, ORIGIN.1, 0.0).unwrap()
}

/// The GPS position `east`/`north` metres from [`origin`].
pub(super) fn gps_at(east: f64, north: f64) -> GpsCoordinates {
    let (lat, lon) = local_to_gps(east, north, &origin()).unwrap();
    GpsCoordinates::new(lat, lon, 0.0).unwrap()
}

/// A GPS position at sea level.
pub(super) fn gps(latitude: f64, longitude: f64) -> GpsCoordinates {
    GpsCoordinates::new(latitude, longitude, 0.0).unwrap()
}

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

/// `count` flat shots due north of `length` metres.
pub(super) fn north(count: usize, length: f64) -> Vec<ShotData> {
    (0..count).map(|_| shot(0.0, length)).collect()
}

pub(super) fn meta(name: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        color: "#123456".to_string(),
        ..LineMeta::default()
    }
}

/// A new project with nothing in it.
pub(super) fn empty(name: &str) -> Project {
    Project::new(name, None)
}

/// A line on a new piece of its own, its start placed at `at` (Create
/// Line's own operation: the origin for 0.6.0's Create Line).
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

pub(super) fn slot(line: LineId, index: u32) -> StationRef {
    StationRef::new(line, index)
}

/// The station at `index` along `line`.
pub(super) fn st(p: &Project, line: LineId, index: u32) -> StationId {
    p.topology().station_at(slot(line, index)).unwrap()
}

/// A loop closure between two slots through Create Loop Closure's own
/// operation (its refusals included).
pub(super) fn close(
    p: &mut Project,
    a: StationRef,
    b: StationRef,
    strategy: ClosureStrategy,
) -> ClosureId {
    p.create_closure_at(a, b, strategy, None, &[])
        .unwrap()
        .value
        .closure
        .id
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
    let [x, y, _] = p.raw_positions().slot(p, at).unwrap();
    [x, y]
}

/// The raw (measured) position of every slot.
pub(super) fn raw_xyz(p: &Project) -> BTreeMap<StationRef, [f64; 3]> {
    p.raw_positions()
        .slots(p)
        .into_iter()
        .map(|s| (s.slot, [s.x, s.y, s.z]))
        .collect()
}

/// [`raw_xyz`] without depth.
pub(super) fn raw_xy(p: &Project) -> BTreeMap<StationRef, [f64; 2]> {
    raw_xyz(p)
        .into_iter()
        .map(|(slot, [x, y, _])| (slot, [x, y]))
        .collect()
}

/// Every slot's raw position without depth, lines in list order (unlike
/// [`raw_xy`], a reordered line list compares different).
pub(super) fn positions(p: &Project) -> Vec<(StationRef, [f64; 2])> {
    p.raw_positions()
        .slots(p)
        .into_iter()
        .map(|s| (s.slot, [s.x, s.y]))
        .collect()
}

pub(super) fn apart(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

pub(super) fn assert_near(actual: [f64; 2], expected: [f64; 2], tol: f64, what: &str) {
    assert!(
        (actual[0] - expected[0]).abs() < tol && (actual[1] - expected[1]).abs() < tol,
        "{what}: {actual:?}, expected {expected:?}"
    );
}

/// Every slot of `a` is within 1e-6 m of the same slot of `b`.
pub(super) fn assert_same_xy(
    a: &BTreeMap<StationRef, [f64; 2]>,
    b: &BTreeMap<StationRef, [f64; 2]>,
) {
    assert_eq!(
        a.keys().collect::<Vec<_>>(),
        b.keys().collect::<Vec<_>>(),
        "the same slots"
    );
    for (slot, at) in a {
        assert_near(*at, b[slot], 1e-6, &format!("{slot:?}"));
    }
}

/// A survey written as a `.stalx` v2 file and opened through `AppState`.
pub(super) struct Opened {
    pub(super) state: AppState,
    pub(super) path: PathBuf,
    pub(super) dir: TempDir,
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
    Opened { state, path, dir }
}

pub(super) fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

/// Run `cmd`, which must succeed.
pub(super) fn run(state: &AppState, cmd: Command) -> CommandResult {
    let what = cmd.description();
    let r = state.execute(cmd);
    assert!(r.success, "{what}: {:?}", r.error);
    r
}

/// Run `cmd`, which must be refused and change nothing (no entry either);
/// returns the refusal.
pub(super) fn refused(state: &AppState, cmd: Command) -> String {
    let before = snapshot(state);
    let entries = state.get_undo_stack().len();
    let what = cmd.description();
    let r = state.execute(cmd);
    assert!(!r.success, "{what} must be refused");
    assert_eq!(snapshot(state), before, "{what}: a refusal changes nothing");
    assert_eq!(state.get_undo_stack().len(), entries, "{what}: no entry");
    r.error.unwrap_or_default()
}

pub(super) fn merge(state: &AppState, a: StationRef, b: StationRef) -> CommandResult {
    run(
        state,
        Command::MergeStations {
            station_a: a,
            station_b: b,
        },
    )
}

pub(super) fn set_datum(
    state: &AppState,
    at: StationRef,
    gps: GpsCoordinates,
    accuracy_m: Option<f64>,
) -> CommandResult {
    run(
        state,
        Command::SetZeroDatum {
            station_ref: at,
            coordinates: gps,
            accuracy_m,
            entrance: None,
        },
    )
}

/// The whole survey as the file writes it, less what a step or a reopen may
/// change by design (`modified_at`, the path), plus every shot's session id
/// (an in-session undo gives them back exactly).
pub(super) type Snapshot = (String, Vec<Vec<u32>>);

pub(super) fn snapshot(state: &AppState) -> Snapshot {
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

/// The warning codes and messages of a result.
pub(super) fn warnings(r: &CommandResult) -> Vec<(String, String)> {
    r.warnings
        .iter()
        .map(|w| (w.code.clone(), w.message.clone()))
        .collect()
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
    before: &Snapshot,
    after: &Snapshot,
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

/// [`assert_undo_redo_exact`] from the state now (the edit just made) back
/// to `before`.
pub(super) fn assert_round_trip(state: &AppState, before: &Snapshot, what: &str) {
    let after = snapshot(state);
    assert_undo_redo_exact(state, before, &after, what);
}

/// The id allocators of a snapshot's `counters` line, by name.
fn counters(line: &str) -> BTreeMap<String, u64> {
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
pub(super) fn assert_same(got: &Snapshot, want: &Snapshot, what: &str) {
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
