//! The shot commands and the delete recipes at the app-state level, on the
//! station-graph model: create, update, delete and set the kind of a shot;
//! Insert Station (split shot at distance); delete a shot and split the line;
//! the range deletes (split, gap-close, prefix, suffix); delete a line's last
//! shot with the line.
//!
//! Ported from the 0.6.0 suites in `state/tests.rs` ("Shot Operations",
//! "split_shot_at_distance", "#41 synthetic repro", "Delete Shot and Split
//! Line", "Bulk shot range deletion (#95)", "delete_last_shot_and_line",
//! "Coverage gap: delete_shot with shift-blocked", "Group 2: Shot
//! insertion/deletion near merge points"). What 0.6.0 pinned on positional
//! cross-references (merge keys, line offsets, replacement merges) is pinned
//! here on what replaced it: a reference is a station id, so it follows its
//! station; a junction stays for its other lines; positions come from the
//! solver, so "the line offset compensates" becomes "the station did not
//! move". Every recipe is one undo entry, and undo and redo land on the
//! recorded state exactly (the canonical v2 bytes, session shot ids too).
//!
//! All data is synthetic, built in memory.

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::survey::{ClosureStrategy, LineMeta, Project, StartAt, ZeroDatum};
use crate::models::{
    GpsCoordinates, LineId, RangeDeleteKind, ShotData, ShotId, ShotKind, ShotUpdates, StationId,
    StationRef, SurveyCredit, SurveyDirection, MIN_SPLIT_CHILD_LENGTH_M,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};

// ----- builders ------------------------------------------------------------

/// 0.6.0's `test_shot_data`: azimuth 45 in / 225 out, depths 10 -> 11.
fn shot(length: f64) -> ShotData {
    ShotData {
        azimuth_in: 45.0,
        azimuth_out: 225.0,
        length,
        depth_in: 10.0,
        depth_out: 11.0,
        ..ShotData::default()
    }
}

/// A shot along `azimuth` (in and out), depths 10 -> 11.
fn along(length: f64, azimuth: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: 10.0,
        depth_out: 11.0,
        ..ShotData::default()
    }
}

/// A level shot along `azimuth` at depth 0.
fn level(length: f64, azimuth: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..ShotData::default()
    }
}

fn shots(lengths: &[f64]) -> Vec<ShotData> {
    lengths.iter().map(|&l| shot(l)).collect()
}

fn meta(name: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        color: "#ff0000".to_string(),
        ..LineMeta::default()
    }
}

fn gps(latitude: f64, longitude: f64) -> GpsCoordinates {
    GpsCoordinates::new(latitude, longitude, 100.0).unwrap()
}

/// A survey under construction (the model's construction API, then
/// `validate`): every free piece is placed at `at`, a datum piece is held by
/// its datum.
struct Build(Project);

impl Build {
    fn new() -> Self {
        Self(Project::new("Shots", None))
    }

    /// A new free line starting at a new station placed at `at`.
    fn free_at(&mut self, name: &str, shots: Vec<ShotData>, at: [f64; 2]) -> LineId {
        let line = self.0.add_line(meta(name), StartAt::New, shots).unwrap();
        let start = self.0.line(line).unwrap().start;
        self.0.set_placement(start, at[0], at[1]).unwrap();
        line
    }

    /// [`Self::free_at`] at the origin.
    fn free(&mut self, name: &str, shots: Vec<ShotData>) -> LineId {
        self.free_at(name, shots, [0.0, 0.0])
    }

    /// A new line held by a datum on its start station (no placement).
    fn datum_line(&mut self, name: &str, shots: Vec<ShotData>, at: GpsCoordinates) -> LineId {
        let line = self.0.add_line(meta(name), StartAt::New, shots).unwrap();
        let start = self.0.line(line).unwrap().start;
        self.0.push_datum(ZeroDatum::new(start, at));
        line
    }

    /// A new line held by nothing yet (the caller adds a datum or a
    /// placement).
    fn bare(&mut self, name: &str, shots: Vec<ShotData>) -> LineId {
        self.0.add_line(meta(name), StartAt::New, shots).unwrap()
    }

    /// A line starting at station `from` of another line (a junction).
    fn from(&mut self, name: &str, from: StationId, shots: Vec<ShotData>) -> LineId {
        self.0
            .add_line(meta(name), StartAt::Station(from), shots)
            .unwrap()
    }

    fn st(&self, line: LineId, index: usize) -> StationId {
        st(&self.0, line, index)
    }

    fn closure(&mut self, a: StationId, b: StationId, strategy: ClosureStrategy) -> u64 {
        self.0.push_closure(a, None, b, None, strategy)
    }

    fn datum(&mut self, station: StationId, at: GpsCoordinates) {
        self.0.push_datum(ZeroDatum::new(station, at));
    }

    fn comment(&mut self, station: StationId, text: &str) {
        self.0.stations.get_mut(&station).unwrap().comment = Some(text.to_string());
    }

    /// The finished survey installed as the open project of a new state.
    fn state(self) -> AppState {
        let violations = self.0.validate();
        assert!(violations.is_empty(), "invalid test survey: {violations:?}");
        let state = AppState::new().unwrap();
        state.set_project_for_tests(self.0);
        state
    }
}

// ----- readers -------------------------------------------------------------

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

/// The station at chain index `index` of `line`.
fn st(p: &Project, line: LineId, index: usize) -> StationId {
    p.line(line)
        .and_then(|l| l.station(index))
        .unwrap_or_else(|| panic!("line {line} has no station {index}"))
}

fn shot_ids(p: &Project, line: LineId) -> Vec<ShotId> {
    p.line(line).unwrap().shots.iter().map(|s| s.id).collect()
}

fn lengths(p: &Project, line: LineId) -> Vec<f64> {
    p.line(line)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.length)
        .collect()
}

fn xy(p: &Project, station: StationId) -> [f64; 2] {
    p.raw_positions()
        .xy(station)
        .unwrap_or_else(|| panic!("station {station} has no position"))
}

fn z(p: &Project, station: StationId) -> f64 {
    p.raw_positions().z(station).unwrap()
}

fn assert_near(a: [f64; 2], b: [f64; 2], what: &str) {
    assert!(
        (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6,
        "{what}: {a:?} vs {b:?}"
    );
}

/// Every station of `stations` sits in `after` where it sat in `before`.
fn assert_still(before: &Project, after: &Project, stations: &[StationId]) {
    for &s in stations {
        assert_near(xy(after, s), xy(before, s), &format!("station {s} moved"));
    }
}

/// The survey as the file would hold it, without what a step or the command
/// layer changes by design: `modified_at` (stamped per edit), the file path,
/// and, with `counters`, the allocators (an undo leaves them standing).
fn canonical_with(p: &Project, counters: Option<crate::models::survey::Counters>) -> Vec<u8> {
    let mut p = p.clone();
    p.modified_at = p.created_at;
    p.file_path = None;
    if let Some(counters) = counters {
        p.counters = counters;
    }
    stalx_v2::encode(&p, &Extras::default(), Mode::Normal).unwrap()
}

fn canonical(p: &Project) -> Vec<u8> {
    canonical_with(p, None)
}

fn ok(result: CommandResult) -> CommandResult {
    assert!(result.success, "command refused: {:?}", result.error);
    result
}

fn refused(result: &CommandResult) -> String {
    assert!(!result.success, "command was not refused");
    result.error.clone().unwrap_or_default()
}

/// The newest history entry's label, kind and preview rows.
fn top_entry(state: &AppState) -> (String, String, Vec<(String, String)>) {
    let items = state.get_undo_stack();
    let top = items.first().expect("an undo entry");
    let rows = top
        .preview_data
        .as_ref()
        .map(|p| {
            p.changes
                .iter()
                .map(|c| (c.field.clone(), c.new_value.clone()))
                .collect()
        })
        .unwrap_or_default();
    (top.description.clone(), top.command_type.clone(), rows)
}

/// Run `edit` and check it records exactly one entry, and that undo and redo
/// land on the states before and after it exactly (session shot ids too).
/// Returns the edit's result and the state after it.
fn one_exact_entry(
    state: &AppState,
    edit: impl FnOnce() -> CommandResult,
) -> (CommandResult, Project) {
    let before = project(state);
    let depth = state.get_undo_stack().len();
    let result = ok(edit());
    let after = project(state);
    assert_eq!(
        state.get_undo_stack().len(),
        depth + 1,
        "one undo entry per action"
    );
    assert!(!state.can_redo(), "a new edit clears the redo branch");

    ok(state.undo());
    let undone = project(state);
    assert_eq!(
        canonical_with(&undone, Some(after.counters)),
        canonical_with(&before, Some(after.counters)),
        "undo lands on the state before the edit"
    );
    for line in before.lines() {
        assert_eq!(
            shot_ids(&undone, line.id),
            shot_ids(&before, line.id),
            "undo gives line {} its session shot ids back",
            line.id
        );
    }
    ok(state.redo());
    let redone = project(state);
    assert_eq!(
        canonical(&redone),
        canonical(&after),
        "redo lands on the edit"
    );
    for line in after.lines() {
        assert_eq!(shot_ids(&redone, line.id), shot_ids(&after, line.id));
    }
    (result, after)
}

/// The project, the history depth and the redo flag stay as they were across
/// a refused `edit`.
fn assert_refused_changes_nothing(
    state: &AppState,
    edit: impl FnOnce() -> CommandResult,
) -> String {
    let before = canonical(&project(state));
    let depth = state.get_undo_stack().len();
    let error = refused(&edit());
    assert_eq!(
        canonical(&project(state)),
        before,
        "a refusal changes nothing"
    );
    assert_eq!(
        state.get_undo_stack().len(),
        depth,
        "a refusal records nothing"
    );
    error
}

fn create_shot(
    state: &AppState,
    line_id: LineId,
    shot: ShotData,
    after: Option<ShotId>,
    before: Option<ShotId>,
) -> CommandResult {
    state.execute(Command::CreateShot {
        line_id,
        shot,
        after_shot_id: after,
        before_shot_id: before,
    })
}

fn delete_shot(state: &AppState, line_id: LineId, shot_id: ShotId) -> CommandResult {
    state.execute(Command::DeleteShot { line_id, shot_id })
}

/// A datum-held loop whose every shot is frozen: line A from the datum
/// station, line B from the same station, and a `Tie` between their far ends
/// (a demoted 0.6.0 merge, aligned in the solve) closing the loop.
fn frozen_loop(lengths_a: &[f64]) -> (Build, LineId, LineId) {
    let mut b = Build::new();
    let a = b.datum_line("A", shots(lengths_a), gps(45.0, -73.0));
    let origin = b.st(a, 0);
    let line_b = b.from("B", origin, shots(lengths_a));
    let (end_a, end_b) = (b.st(a, lengths_a.len()), b.st(line_b, lengths_a.len()));
    b.closure(end_a, end_b, ClosureStrategy::Tie);
    (b, a, line_b)
}

// ===== Shot Operations ======================================================

#[test]
fn a_created_shot_goes_at_the_end_with_a_new_station_and_undoes_exactly() {
    let mut b = Build::new();
    let line = b.free("Line", Vec::new());
    let state = b.state();

    let (result, after) =
        one_exact_entry(&state, || create_shot(&state, line, shot(10.0), None, None));
    let l = after.line(line).unwrap();
    assert_eq!(lengths(&after, line), vec![10.0]);
    assert_eq!(
        result.shot_id,
        Some(l.shots[0].id),
        "the result names the new shot"
    );
    assert_eq!(
        after.station(l.shots[0].to).unwrap().name,
        "1",
        "End rule after 0"
    );
    let (label, kind, _) = top_entry(&state);
    assert_eq!(kind, "create_shot");
    assert_eq!(label, "Create shot in line \"Line\"");
}

#[test]
fn an_updated_shot_takes_its_new_length_and_comment() {
    let mut b = Build::new();
    let line = b.free("Line", shots(&[5.0]));
    let state = b.state();
    let shot_id = shot_ids(&project(&state), line)[0];

    let (_, after) = one_exact_entry(&state, || {
        state.execute(Command::UpdateShot {
            line_id: line,
            shot_id,
            updates: ShotUpdates {
                length: Some(15.0),
                comment: Some(Some("Updated shot".to_string())),
                ..ShotUpdates::default()
            },
        })
    });
    let s = &after.line(line).unwrap().shots[0];
    assert_eq!(s.length, 15.0);
    assert_eq!(s.comment.as_deref(), Some("Updated shot"));
}

#[test]
fn a_deleted_shot_leaves_the_line_with_one_shot_less() {
    let mut b = Build::new();
    let line = b.free("Line", shots(&[5.0, 7.0]));
    let state = b.state();
    let first = shot_ids(&project(&state), line)[0];

    let (_, after) = one_exact_entry(&state, || delete_shot(&state, line, first));
    assert_eq!(lengths(&after, line), vec![7.0]);
    let (label, kind, _) = top_entry(&state);
    assert_eq!(
        (label.as_str(), kind.as_str()),
        ("Delete shot Line: 0 \u{2192} 1", "delete_shot")
    );
}

/// 0.6.0's `test_delete_first_shot_preserves_station_positions`: the stations
/// that stay keep their map position, and their depth is the mean of the
/// readings still at them.
#[test]
fn deleting_the_first_shot_keeps_the_other_stations_where_they_were() {
    let mut b = Build::new();
    let line = b.free("Line", shots(&[5.0, 7.0]));
    let state = b.state();
    let before = project(&state);
    let (s1, s2) = (st(&before, line, 1), st(&before, line, 2));
    let first = shot_ids(&before, line)[0];

    ok(delete_shot(&state, line, first));
    let after = project(&state);
    assert_eq!(
        st(&after, line, 0),
        s1,
        "the old station 1 is the new start"
    );
    assert_still(&before, &after, &[s1, s2]);
    // Station 1 had 11 m in and 10 m out; now only the remaining shot's
    // 10 m reading is there.
    assert!((z(&after, s1) - 10.0).abs() < 1e-9);
    assert!((z(&after, s2) - 11.0).abs() < 1e-9);
    assert!(
        after.station(st(&before, line, 0)).is_none(),
        "station 0 left the survey"
    );
}

/// 0.6.0's `test_delete_shot_rewires_merges`: deleting a hub line's first
/// shot used to remove its member of the merge group and stitch the other
/// two members back together. On the station graph the hub is one station:
/// the line leaves it, the others keep it.
#[test]
fn a_line_leaving_a_hub_leaves_the_hub_to_its_other_lines() {
    let mut b = Build::new();
    let a = b.free("A", shots(&[5.0, 7.0]));
    let hub = b.st(a, 0);
    let line_b = b.from("B", hub, shots(&[6.0]));
    let c = b.from("C", hub, shots(&[8.0]));
    let state = b.state();
    let first = shot_ids(&project(&state), a)[0];

    let (_, after) = one_exact_entry(&state, || delete_shot(&state, a, first));
    assert_ne!(after.line(a).unwrap().start, hub, "A left the hub");
    assert_eq!(after.line(line_b).unwrap().start, hub);
    assert_eq!(
        after.line(c).unwrap().start,
        hub,
        "B and C still share the hub"
    );
    assert!(after.station(hub).is_some());
}

#[test]
fn a_frozen_shot_cannot_be_deleted() {
    let (b, a, _) = frozen_loop(&[5.0, 7.0]);
    let state = b.state();
    let p = project(&state);
    assert!(
        super::command_shot::any_shot_frozen(&p, a, [0]),
        "precondition: the loop between the datum and the tie freezes A's shots"
    );
    let first = shot_ids(&p, a)[0];
    let error = assert_refused_changes_nothing(&state, || delete_shot(&state, a, first));
    assert_eq!(
        error,
        "Cannot delete a frozen shot. This shot is part of an adjusted survey network."
    );
}

#[test]
fn a_frozen_shot_cannot_be_deleted_by_a_split() {
    let (b, a, _) = frozen_loop(&[5.0, 7.0, 4.0]);
    let state = b.state();
    let p = project(&state);
    assert!(super::command_shot::any_shot_frozen(&p, a, [1]));
    let middle = shot_ids(&p, a)[1];
    let error = assert_refused_changes_nothing(&state, || {
        state.execute_delete_shot_and_split(a, middle, "A-1".to_string(), "A-2".to_string())
    });
    assert!(error.contains("frozen"), "{error}");
}

#[test]
fn a_frozen_shot_cannot_change_geometry_but_can_change_its_comment() {
    let (b, a, _) = frozen_loop(&[5.0, 7.0]);
    let state = b.state();
    let first = shot_ids(&project(&state), a)[0];
    let error = assert_refused_changes_nothing(&state, || {
        state.execute(Command::UpdateShot {
            line_id: a,
            shot_id: first,
            updates: ShotUpdates {
                length: Some(6.0),
                ..ShotUpdates::default()
            },
        })
    });
    assert!(error.contains("frozen"), "{error}");
    ok(state.execute(Command::UpdateShot {
        line_id: a,
        shot_id: first,
        updates: ShotUpdates {
            comment: Some(Some("note".to_string())),
            ..ShotUpdates::default()
        },
    }));
}

// ===== Insert Station (split shot at distance, #59) =========================

fn split_target() -> ShotData {
    ShotData {
        uuid: None,
        azimuth_in: 30.0,
        azimuth_out: 32.0,
        length: 10.0,
        depth_in: 10.0,
        depth_out: 20.0,
        pitch_in: Some(10.0),
        pitch_out: Some(-5.0),
        left: Some(1.0),
        right: Some(2.0),
        up: Some(0.5),
        down: Some(0.25),
        comment: Some("mid comment".to_string()),
        kind: ShotKind::Normal,
        marker_index: Some(3),
        temperature: Some(15.0),
        credit: None,
    }
}

#[test]
fn insert_station_splits_the_shot_into_a_new_head_and_the_old_shot_as_tail() {
    let mut b = Build::new();
    let line = b.free("Main", vec![shot(5.0), split_target(), shot(5.0)]);
    let state = b.state();
    let before = project(&state);
    let target = before.line(line).unwrap().shots[1].clone();

    let (result, after) = one_exact_entry(&state, || {
        state.execute_split_shot_at_distance(line, target.id, 4.0)
    });
    let l = after.line(line).unwrap();
    assert_eq!(l.shots.len(), 4);
    let (head, tail) = (&l.shots[1], &l.shots[2]);
    assert_eq!(result.shot_id, Some(head.id), "the result names the head");
    assert_ne!(head.uuid, target.uuid, "the head gets a fresh uuid");
    assert_eq!(head.length, 4.0);
    assert_eq!(head.depth_in, 10.0);
    assert!(
        (head.depth_out - 14.0).abs() < 1e-9,
        "mid depth interpolated"
    );
    assert_eq!((head.azimuth_in, head.azimuth_out), (30.0, 32.0));
    assert_eq!((head.pitch_in, head.pitch_out), (Some(10.0), Some(-5.0)));
    assert_eq!(head.comment, None);
    assert_eq!(head.marker_index, None);
    assert_eq!(head.temperature, Some(15.0));
    assert!(!head.kind.is_virtual());
    assert_eq!(tail.id, target.id, "the tail keeps the shot's id");
    assert_eq!(tail.uuid, target.uuid, "and its uuid");
    assert_eq!(tail.length, 6.0);
    assert!((tail.depth_in - 14.0).abs() < 1e-9);
    assert_eq!(tail.depth_out, 20.0);
    assert_eq!(tail.comment.as_deref(), Some("mid comment"));
    assert_eq!(tail.marker_index, Some(3));
    // Both children keep the parent's LRUD (a deliberate mid-station estimate).
    for child in [head, tail] {
        assert_eq!(
            (child.left, child.right, child.up, child.down),
            (Some(1.0), Some(2.0), Some(0.5), Some(0.25))
        );
    }

    let made = st(&after, line, 2);
    let name = after.station(made).unwrap().name.clone();
    let (label, kind, rows) = top_entry(&state);
    assert_eq!(kind, "batch", "recorded under 0.6.0's kind");
    assert_eq!(label, format!("Insert station Main / {name}"));
    assert_eq!(
        rows,
        vec![
            (
                "Create shot in line \"Main\"".to_string(),
                "Length: 4.00, Azimuth: 30.0".to_string()
            ),
            (
                "Update shot in line \"Main\"".to_string(),
                "Length: 6.00, Depth: 14.00".to_string()
            ),
        ]
    );
}

/// 0.6.0 pinned that a reference on the shot's end station (a merge, a datum,
/// a comment) resolved to the shifted end, not the new station. On the
/// station graph a reference IS the station: the junction, the datum and the
/// comment stay on it, now one index further along the line.
#[test]
fn insert_station_leaves_what_the_end_station_carries_on_it() {
    let mut b = Build::new();
    let a = b.bare("A", vec![level(3.0, 0.0), level(3.0, 180.0), shot(5.0)]);
    let end = b.st(a, 2);
    let line_b = b.from("B", end, shots(&[5.0]));
    b.datum(end, gps(45.0, -73.0));
    b.comment(end, "endpoint note");
    let state = b.state();
    let target = shot_ids(&project(&state), a)[1];

    ok(state.execute_split_shot_at_distance(a, target, 1.0));
    let after = project(&state);
    assert_eq!(
        st(&after, a, 3),
        end,
        "the end station is one index further on"
    );
    assert_ne!(st(&after, a, 2), end, "the new station is not it");
    assert_eq!(after.line(line_b).unwrap().start, end);
    assert_eq!(after.datums.len(), 1);
    assert_eq!(after.datums[0].station, end);
    assert_eq!(
        after.station(end).unwrap().comment.as_deref(),
        Some("endpoint note")
    );
    assert_eq!(after.station(st(&after, a, 2)).unwrap().comment, None);
}

#[test]
fn insert_station_keeps_a_closure_on_the_end_station() {
    let mut b = Build::new();
    let a = b.free("A", vec![level(3.0, 0.0), level(3.0, 180.0), shot(5.0)]);
    let (start, end) = (b.st(a, 0), b.st(a, 2));
    b.closure(end, start, ClosureStrategy::Batch);
    let state = b.state();
    let target = shot_ids(&project(&state), a)[1];

    ok(state.execute_split_shot_at_distance(a, target, 1.0));
    let after = project(&state);
    assert_eq!(after.closures.len(), 1);
    let c = &after.closures[0];
    assert_eq!((c.a, c.b), (end, start));
    assert_eq!(
        after.topology().slot_on(end, a),
        Some(StationRef::new(a, 3)),
        "the closure's end is the old end station, now at index 3"
    );
    assert_eq!(
        after.topology().slot_on(start, a),
        Some(StationRef::new(a, 0))
    );
}

#[test]
fn insert_station_in_the_first_shot_keeps_the_line_start() {
    let mut b = Build::new();
    let a = b.bare("A", shots(&[10.0, 5.0]));
    let start = b.st(a, 0);
    let line_b = b.from("B", start, shots(&[5.0]));
    b.datum(start, gps(45.0, -73.0));
    let state = b.state();
    let first = shot_ids(&project(&state), a)[0];

    ok(state.execute_split_shot_at_distance(a, first, 4.0));
    let after = project(&state);
    assert_eq!(st(&after, a, 0), start);
    assert_eq!(after.datums[0].station, start);
    assert_eq!(after.line(line_b).unwrap().start, start);
}

#[test]
fn insert_station_refuses_distances_outside_the_shot() {
    let mut b = Build::new();
    let line = b.free("Line", shots(&[10.0]));
    let state = b.state();
    let shot_id = shot_ids(&project(&state), line)[0];
    // 0.005 and 9.995 fall under the MIN_SPLIT_CHILD_LENGTH_M (0.01 m) floor.
    for distance in [0.0, 10.0, -1.0, f64::NAN, f64::INFINITY, 11.0, 0.005, 9.995] {
        assert_refused_changes_nothing(&state, || {
            state.execute_split_shot_at_distance(line, shot_id, distance)
        });
    }
    let error = refused(&state.execute_split_shot_at_distance(line, shot_id, f64::NAN));
    assert_eq!(error, "Distance must be finite");
}

#[test]
fn insert_station_accepts_the_exact_minimum_child_length() {
    let mut b = Build::new();
    let line = b.free("Line", shots(&[1.0]));
    let state = b.state();
    let shot_id = shot_ids(&project(&state), line)[0];

    ok(state.execute_split_shot_at_distance(line, shot_id, MIN_SPLIT_CHILD_LENGTH_M));
    let l = lengths(&project(&state), line);
    assert_eq!(l.len(), 2);
    assert!((l[0] - MIN_SPLIT_CHILD_LENGTH_M).abs() < 1e-12);
    assert!((l[1] - (1.0 - MIN_SPLIT_CHILD_LENGTH_M)).abs() < 1e-12);
}

#[test]
fn insert_station_refuses_a_frozen_shot() {
    let (b, a, _) = frozen_loop(&[5.0, 7.0, 4.0]);
    let state = b.state();
    let middle = shot_ids(&project(&state), a)[1];
    let error = assert_refused_changes_nothing(&state, || {
        state.execute_split_shot_at_distance(a, middle, 3.0)
    });
    assert_eq!(error, super::commands::FROZEN_GEOMETRY);
}

#[test]
fn insert_station_moves_no_station_of_the_line() {
    let mut b = Build::new();
    let line = b.free(
        "Main",
        vec![
            ShotData {
                depth_in: 0.0,
                depth_out: 1.0,
                ..level(5.0, 10.0)
            },
            ShotData {
                depth_in: 1.0,
                depth_out: 4.0,
                ..level(8.0, 50.0)
            },
            ShotData {
                depth_in: 4.0,
                depth_out: 2.0,
                ..level(6.0, 120.0)
            },
        ],
    );
    let state = b.state();
    let before = project(&state);
    let stations: Vec<StationId> = before.line(line).unwrap().stations().collect();
    let target = shot_ids(&before, line)[1];

    ok(state.execute_split_shot_at_distance(line, target, 3.0));
    let after = project(&state);
    assert_still(&before, &after, &stations);
    assert_eq!(
        st(&after, line, 4),
        stations[3],
        "the last station is one further on"
    );
}

#[test]
fn insert_station_in_a_jump_shot_makes_two_jump_shots() {
    let mut b = Build::new();
    let line = b.free(
        "Line",
        vec![ShotData {
            kind: ShotKind::Jump,
            ..shot(10.0)
        }],
    );
    let state = b.state();
    let shot_id = shot_ids(&project(&state), line)[0];

    ok(state.execute_split_shot_at_distance(line, shot_id, 4.0));
    let after = project(&state);
    let l = after.line(line).unwrap();
    assert!(l.shots[0].kind.is_virtual(), "the head inherits the kind");
    assert!(l.shots[1].kind.is_virtual(), "the tail keeps its own");
}

#[test]
fn insert_station_refuses_a_line_at_the_shot_cap() {
    let max = crate::models::FEATURE_ID_STRIDE as usize - 1;
    let mut b = Build::new();
    let line = b.free("Cap", vec![level(1.0, 0.0); max]);
    let state = b.state();
    let shot_id = shot_ids(&project(&state), line)[0];
    let error = refused(&state.execute_split_shot_at_distance(line, shot_id, 0.5));
    assert!(error.contains("maximum"), "{error}");
    assert_eq!(state.get_undo_stack().len(), 0);
}

// ===== #41: a delete moves nothing it does not have to ======================

/// Deleting the frame datum's station re-frames the survey on the other
/// datum: that station becomes the local origin and everything translates by
/// the same vector, so nothing moves relative to anything else.
#[test]
fn deleting_the_frame_datum_station_reframes_without_moving_anything_relative() {
    let mut b = Build::new();
    let a = b.datum_line("A", shots(&[5.0, 7.0]), gps(45.0, -73.0));
    let line_b = b.datum_line("B", shots(&[4.0, 6.0]), gps(46.0, -74.0));
    let state = b.state();
    let before = project(&state);
    let b0 = st(&before, line_b, 0);
    let [dx, dy] = xy(&before, b0);
    assert!(dx.abs() > 1000.0, "setup: the datums are kilometres apart");
    let first = shot_ids(&before, a)[0];

    ok(delete_shot(&state, a, first));
    let after = project(&state);
    assert_near(
        xy(&after, b0),
        [0.0, 0.0],
        "the promoted datum is the origin",
    );
    let moved: Vec<StationId> = before
        .line(line_b)
        .unwrap()
        .stations()
        .chain(after.line(a).unwrap().stations())
        .collect();
    for s in moved {
        let [x, y] = xy(&before, s);
        assert_near(xy(&after, s), [x - dx, y - dy], &format!("station {s}"));
    }
}

#[test]
fn deleting_the_last_shot_moves_neither_the_line_head_nor_a_line_on_it() {
    let mut b = Build::new();
    let a = b.datum_line("A", shots(&[5.0, 7.0]), gps(45.0, -73.0));
    let line_b = b.from("B", b.st(a, 0), shots(&[4.0, 6.0]));
    let state = b.state();
    let before = project(&state);
    let mut kept: Vec<StationId> = before.line(line_b).unwrap().stations().collect();
    kept.push(st(&before, a, 1));
    let last = *shot_ids(&before, a).last().unwrap();

    ok(delete_shot(&state, a, last));
    assert_still(&before, &project(&state), &kept);
}

#[test]
fn deleting_the_first_shot_moves_no_line_downstream() {
    let mut b = Build::new();
    let a = b.free("A", shots(&[5.0, 7.0]));
    let c = b.from("C", b.st(a, 2), shots(&[8.0]));
    let state = b.state();
    let before = project(&state);
    let kept: Vec<StationId> = before.line(c).unwrap().stations().collect();
    let first = shot_ids(&before, a)[0];

    ok(delete_shot(&state, a, first));
    assert_still(&before, &project(&state), &kept);
}

#[test]
fn unrecognised_shot_kinds_are_scoped_to_their_load() {
    use crate::models::UnrecognisedShotKind;

    let state = AppState::new().unwrap();
    let entry = |line_id: u32| UnrecognisedShotKind {
        line_id,
        shot_id: line_id * crate::models::FEATURE_ID_STRIDE,
        shot_number: 1,
        line_name: Some(format!("s{line_id}")),
        raw_value: "7".to_string(),
    };
    state.set_unrecognised_shot_kinds(1, vec![entry(11)]);
    state.set_unrecognised_shot_kinds(2, vec![entry(22)]);
    state.set_unrecognised_shot_kinds(1, vec![entry(11)]);
    assert!(
        state.take_unrecognised_shot_kinds(1).is_empty(),
        "a stale load must not receive the current load's diagnostics"
    );
    let for_b = state.take_unrecognised_shot_kinds(2);
    assert_eq!(for_b.len(), 1);
    assert_eq!(for_b[0].line_id, 22);
    assert!(state.take_unrecognised_shot_kinds(2).is_empty());
}

/// A legacy file whose shot holds a kind 0.6.0 cannot read: the load hands
/// it to the frontend under that load's id, named by the migrated shot id,
/// and the shot opens as `Reference` (the fail-closed substitution).
#[test]
fn a_legacy_shot_with_an_unreadable_kind_is_reported_to_its_load() {
    use super::legacy_fixture_kit::{frozen, old, write_legacy_file, Legacy};
    use std::sync::Arc;

    crate::app_dirs::init_for_test();
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("kinds.stalx");
    let mut legacy = Legacy::new("Kinds".to_string(), None);
    legacy.add_line(old::LineData {
        name: Some("Main".to_string()),
        shots: shots(&[5.0, 7.0]).iter().map(frozen).collect(),
        ..Default::default()
    });
    write_legacy_file(&path, &legacy);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE shots SET kind = 7 WHERE sequence_order = 1", [])
        .unwrap();

    let state = AppState::new().unwrap();
    state.set_event_sink(Arc::new(super::robustness_kit::NullSink));
    let loaded = state.do_load_project_full(path).unwrap();
    let found = state.take_unrecognised_shot_kinds(loaded.loading_id);
    assert_eq!(found.len(), 1, "{found:?}");
    let p = project(&state);
    let line = &p.lines()[0];
    let index = found[0].shot_number as usize - 1;
    assert_eq!(found[0].line_id, line.id);
    assert_eq!(found[0].shot_id, line.shots[index].id);
    assert_eq!(found[0].line_name.as_deref(), Some("Main"));
    assert_eq!(found[0].raw_value, "7");
    assert_eq!(line.shots[index].kind, ShotKind::Reference);
    assert!(state
        .take_unrecognised_shot_kinds(loaded.loading_id)
        .is_empty());
}

#[test]
fn a_shot_kind_change_is_one_exact_entry() {
    let mut b = Build::new();
    let line = b.free("Line", shots(&[5.0]));
    let state = b.state();
    let shot_id = shot_ids(&project(&state), line)[0];

    let (_, after) = one_exact_entry(&state, || {
        state.execute(Command::SetShotKind {
            line_id: line,
            shot_id,
            kind: ShotKind::Jump,
        })
    });
    assert!(after.line(line).unwrap().shots[0].kind.is_virtual());
    assert_eq!(top_entry(&state).1, "set_shot_kind");
}

// ===== Delete shot and split line ===========================================

#[test]
fn delete_and_split_makes_two_lines_on_the_deleted_shots_two_stations() {
    let mut b = Build::new();
    let line = b.free("Original", shots(&[5.0, 7.0, 3.0, 4.0, 6.0]));
    let state = b.state();
    let before = project(&state);
    let (from, to) = (st(&before, line, 2), st(&before, line, 3));
    let middle = shot_ids(&before, line)[2];

    let (result, after) = one_exact_entry(&state, || {
        state.execute_delete_shot_and_split(
            line,
            middle,
            "Line A".to_string(),
            "Line B".to_string(),
        )
    });
    let new_line = result.line_id.unwrap();
    assert_eq!(after.line(line).unwrap().name.as_deref(), Some("Line A"));
    assert_eq!(
        after.line(new_line).unwrap().name.as_deref(),
        Some("Line B")
    );
    assert_eq!(lengths(&after, line), vec![5.0, 7.0]);
    assert_eq!(lengths(&after, new_line), vec![4.0, 6.0]);
    assert_eq!(*after.line_stations(line).unwrap().last().unwrap(), from);
    assert_eq!(after.line(new_line).unwrap().start, to);
    assert_eq!(
        after.stations.len(),
        before.stations.len(),
        "no station leaves"
    );

    let (label, kind, rows) = top_entry(&state);
    assert_eq!(kind, "batch");
    assert_eq!(
        label,
        "Delete shot 2 \u{2192} 3 and split line \"Original\""
    );
    assert_eq!(
        rows,
        vec![
            ("Delete shot in line \"Line A\"".to_string(), String::new()),
            (
                "Create line \"Line B\"".to_string(),
                "Name: Line B".to_string()
            ),
        ]
    );
}

#[test]
fn both_halves_keep_the_line_metadata_and_the_shots_their_credit() {
    let date = chrono::NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
    let credit = SurveyCredit::from_legacy(Some("Jane"), Some(date))
        .stored()
        .map(std::sync::Arc::new);
    let mut b = Build::new();
    let line = b.free(
        "Original",
        (0..3)
            .map(|i| ShotData {
                credit: credit.clone(),
                ..shot(5.0 + f64::from(i))
            })
            .collect(),
    );
    {
        let l = b.0.line_mut(line).unwrap();
        l.color = "#aabbcc".to_string();
        l.direction = SurveyDirection::Out;
        l.quality = 0.5;
    }
    let state = b.state();
    let middle = shot_ids(&project(&state), line)[1];

    let new_line =
        ok(state.execute_delete_shot_and_split(line, middle, "A".to_string(), "B".to_string()))
            .line_id
            .unwrap();
    let after = project(&state);
    for id in [line, new_line] {
        let l = after.line(id).unwrap();
        assert_eq!(l.color, "#aabbcc");
        assert_eq!(l.direction, SurveyDirection::Out);
        assert_eq!(l.quality, 0.5);
        assert!(
            l.shots.iter().all(|s| s.credit == credit),
            "credit rides on the shots"
        );
    }
}

/// 0.6.0 computed the second half's line offset from the shots before it;
/// pinned here as what it was for: the second half stays where it was.
#[test]
fn the_second_half_stays_where_it_was() {
    let mut b = Build::new();
    let line = b.free(
        "Original",
        vec![along(10.0, 90.0), along(5.0, 90.0), along(8.0, 90.0)],
    );
    let state = b.state();
    let before = project(&state);
    let stations: Vec<StationId> = before.line(line).unwrap().stations().collect();
    let middle = shot_ids(&before, line)[1];

    ok(state.execute_delete_shot_and_split(line, middle, "A".to_string(), "B".to_string()));
    assert_still(&before, &project(&state), &stations);
}

#[test]
fn a_junction_on_the_tail_follows_its_station_onto_the_second_half() {
    let mut b = Build::new();
    let a = b.free("ToSplit", shots(&[5.0, 7.0, 3.0]));
    let junction = b.st(a, 3);
    let other = b.from("Other", junction, shots(&[4.0]));
    let state = b.state();
    let second = shot_ids(&project(&state), a)[1];

    let new_line =
        ok(state.execute_delete_shot_and_split(a, second, "A".to_string(), "B".to_string()))
            .line_id
            .unwrap();
    let after = project(&state);
    assert_eq!(after.line(other).unwrap().start, junction);
    assert_eq!(
        after.topology().slot_on(junction, new_line),
        Some(StationRef::new(new_line, 1))
    );
}

/// A closure that the split leaves as the only connection of the second half
/// is a merge, not a loop (#8): the sweep promotes it.
#[test]
fn a_closure_left_as_the_second_halfs_only_connection_becomes_a_junction() {
    let mut b = Build::new();
    let a = b.free("ToSplit", shots(&[5.0, 7.0, 3.0, 4.0]));
    let line_b = b.from("Other", b.st(a, 0), shots(&[4.0]));
    let (a4, b1) = (b.st(a, 4), b.st(line_b, 1));
    b.closure(a4, b1, ClosureStrategy::Batch);
    let state = b.state();
    let third = shot_ids(&project(&state), a)[2];

    let (result, after) = one_exact_entry(&state, || {
        state.execute_delete_shot_and_split(a, third, "A".to_string(), "B-split".to_string())
    });
    let new_line = result.line_id.unwrap();
    assert!(
        after.closures.is_empty(),
        "the sole-bridge closure was promoted"
    );
    let joined = st(&after, new_line, 1);
    assert_eq!(
        after.topology().lines_of(joined),
        {
            let mut lines = vec![line_b, new_line];
            lines.sort_unstable();
            lines
        },
        "the second half's end and Other's end are one station now"
    );
}

#[test]
fn delete_and_split_refuses_an_edge_shot() {
    let mut b = Build::new();
    let line = b.free("Line", shots(&[5.0, 7.0, 3.0]));
    let state = b.state();
    let ids = shot_ids(&project(&state), line);
    for edge in [ids[0], ids[2]] {
        let error = assert_refused_changes_nothing(&state, || {
            state.execute_delete_shot_and_split(line, edge, "A".to_string(), "B".to_string())
        });
        assert_eq!(
            error,
            "Cannot split at edge shot (first or last). Use simple delete instead."
        );
    }
}

#[test]
fn delete_and_split_refuses_names_a_rename_would_refuse() {
    let mut b = Build::new();
    let line = b.free("Line", shots(&[5.0, 7.0, 3.0]));
    b.free_at("Taken", shots(&[2.0]), [50.0, 0.0]);
    let state = b.state();
    let middle = shot_ids(&project(&state), line)[1];
    for (a, name_b, expected) in [
        ("", "B", "A line name cannot be empty"),
        ("A", "Taken", "A line named \"Taken\" already exists"),
        ("Same", " Same ", "A line named \"Same\" already exists"),
    ] {
        let error = assert_refused_changes_nothing(&state, || {
            state.execute_delete_shot_and_split(line, middle, a.to_string(), name_b.to_string())
        });
        assert_eq!(error, expected);
    }
    // The line's own name may be reused by a half (it is being replaced).
    let done = ok(state.execute_delete_shot_and_split(
        line,
        middle,
        " Line ".to_string(),
        "Line 2".to_string(),
    ));
    let after = project(&state);
    assert_eq!(after.line(line).unwrap().name.as_deref(), Some("Line"));
    assert_eq!(
        after.line(done.line_id.unwrap()).unwrap().name.as_deref(),
        Some("Line 2")
    );
}

#[test]
fn a_legacy_hand_move_on_the_tail_moves_to_the_second_half() {
    let mut b = Build::new();
    let line = b.free("Line", shots(&[5.0, 7.0, 3.0, 4.0]));
    let s3 = b.st(line, 3);
    b.0.line_mut(line)
        .unwrap()
        .hand_moves
        .insert(s3, [1.0, 2.0, 3.0]);
    let state = b.state();
    let second = shot_ids(&project(&state), line)[1];

    let new_line =
        ok(state.execute_delete_shot_and_split(line, second, "A".to_string(), "B".to_string()))
            .line_id
            .unwrap();
    let after = project(&state);
    assert_eq!(
        after.line(new_line).unwrap().hand_moves.get(&s3),
        Some(&[1.0, 2.0, 3.0])
    );
    assert!(after.line(line).unwrap().hand_moves.is_empty());
}

#[test]
fn delete_and_split_undoes_to_the_one_line_and_redoes_the_split() {
    let mut b = Build::new();
    let line = b.free("Original", shots(&[5.0, 7.0, 3.0]));
    let state = b.state();
    let middle = shot_ids(&project(&state), line)[1];

    let (result, _) = one_exact_entry(&state, || {
        state.execute_delete_shot_and_split(line, middle, "A".to_string(), "B".to_string())
    });
    ok(state.undo());
    let undone = project(&state);
    assert_eq!(undone.lines().len(), 1);
    assert_eq!(undone.line(line).unwrap().name.as_deref(), Some("Original"));
    assert_eq!(lengths(&undone, line), vec![5.0, 7.0, 3.0]);
    ok(state.redo());
    let redone = project(&state);
    assert_eq!(redone.lines().len(), 2);
    assert_eq!(lengths(&redone, line), vec![5.0]);
    assert_eq!(lengths(&redone, result.line_id.unwrap()), vec![3.0]);
}

// ===== Range deletes (#95) ==================================================

fn six_shots() -> Vec<ShotData> {
    shots(&[5.0, 7.0, 3.0, 4.0, 6.0, 8.0])
}

#[test]
fn a_range_split_keeps_the_head_and_makes_a_line_of_the_tail() {
    let mut b = Build::new();
    let line = b.free("Original", six_shots());
    b.0.line_mut(line).unwrap().direction = SurveyDirection::Out;
    let state = b.state();
    let ids = shot_ids(&project(&state), line);

    let (result, after) = one_exact_entry(&state, || {
        state.execute_delete_shot_range_split(
            line,
            ids[2],
            ids[3],
            "A".to_string(),
            "B".to_string(),
        )
    });
    let new_line = result.line_id.unwrap();
    assert_eq!(after.line(line).unwrap().name.as_deref(), Some("A"));
    assert_eq!(lengths(&after, line), vec![5.0, 7.0]);
    assert_eq!(after.line(new_line).unwrap().name.as_deref(), Some("B"));
    assert_eq!(lengths(&after, new_line), vec![6.0, 8.0]);
    assert_eq!(
        after.line(new_line).unwrap().direction,
        SurveyDirection::Out
    );
    let (label, kind, rows) = top_entry(&state);
    assert_eq!(kind, "batch");
    assert_eq!(
        label,
        "Delete 2 shots (2 \u{2192} 4) and split line \"Original\""
    );
    assert_eq!(rows[0].0, "Delete shot in line \"A\"");
    assert_eq!(
        rows[1],
        ("Create line \"B\"".to_string(), "Name: B".to_string())
    );
}

#[test]
fn a_range_split_leaves_the_tail_where_it_was() {
    let mut b = Build::new();
    let line = b.free(
        "Original",
        vec![
            along(10.0, 90.0),
            along(5.0, 90.0),
            along(7.0, 90.0),
            along(8.0, 90.0),
        ],
    );
    let state = b.state();
    let before = project(&state);
    let tail = [st(&before, line, 3), st(&before, line, 4)];
    let ids = shot_ids(&before, line);

    let new_line = ok(state.execute_delete_shot_range_split(
        line,
        ids[1],
        ids[2],
        "A".to_string(),
        "B".to_string(),
    ))
    .line_id
    .unwrap();
    let after = project(&state);
    assert_eq!(after.line(new_line).unwrap().start, tail[0]);
    assert_still(&before, &after, &tail);
    assert!(
        after.station(st(&before, line, 2)).is_none(),
        "the interior station left"
    );
}

#[test]
fn a_range_gap_close_rejoins_the_survivors_into_one_line() {
    let mut b = Build::new();
    let line = b.free("Original", six_shots());
    let state = b.state();
    let ids = shot_ids(&project(&state), line);

    let (result, after) = one_exact_entry(&state, || {
        state.execute_delete_shot_range_shift(line, ids[2], ids[3])
    });
    assert_eq!(result.line_id, Some(line));
    assert_eq!(after.lines().len(), 1);
    assert_eq!(lengths(&after, line), vec![5.0, 7.0, 6.0, 8.0]);
    let (label, kind, rows) = top_entry(&state);
    assert_eq!(kind, "batch");
    assert_eq!(
        label,
        "Delete 2 shots (2 \u{2192} 4) from line \"Original\""
    );
    assert_eq!(
        rows,
        vec![
            (
                "Delete shot in line \"Original\"".to_string(),
                String::new()
            ),
            (
                "Delete shot in line \"Original\"".to_string(),
                String::new()
            ),
        ]
    );
}

#[test]
fn a_one_shot_gap_close_is_labelled_as_one_shot() {
    let mut b = Build::new();
    let line = b.free("Original", six_shots());
    let state = b.state();
    let ids = shot_ids(&project(&state), line);
    ok(state.execute_delete_shot_range_shift(line, ids[2], ids[2]));
    assert_eq!(
        top_entry(&state).0,
        "Delete shot 2 \u{2192} 3 from line \"Original\""
    );
}

#[test]
fn a_prefix_delete_keeps_the_new_start_where_it_was() {
    let mut b = Build::new();
    let line = b.free(
        "Original",
        (0..5).map(|i| along(10.0 - f64::from(i), 90.0)).collect(),
    );
    let state = b.state();
    let before = project(&state);
    let kept: Vec<StationId> = before.line(line).unwrap().stations().skip(2).collect();
    let ids = shot_ids(&before, line);

    ok(state.execute_delete_shot_range_shift(line, ids[0], ids[1]));
    let after = project(&state);
    assert_eq!(lengths(&after, line), vec![8.0, 7.0, 6.0]);
    assert_eq!(st(&after, line, 0), kept[0]);
    assert_still(&before, &after, &kept);
}

#[test]
fn a_suffix_delete_keeps_the_head_where_it_was() {
    let mut b = Build::new();
    let line = b.free(
        "Original",
        (0..5).map(|i| along(10.0 - f64::from(i), 90.0)).collect(),
    );
    let state = b.state();
    let before = project(&state);
    let head: Vec<StationId> = before.line(line).unwrap().stations().take(4).collect();
    let ids = shot_ids(&before, line);

    ok(state.execute_delete_shot_range_shift(line, ids[3], ids[4]));
    let after = project(&state);
    assert_eq!(lengths(&after, line), vec![10.0, 9.0, 8.0]);
    assert_still(&before, &after, &head);
}

#[test]
fn a_range_split_moves_a_junction_on_the_tail_onto_the_new_line() {
    let mut b = Build::new();
    let a = b.free("ToSplit", six_shots());
    let junction = b.st(a, 5);
    let other = b.from("Other", junction, shots(&[4.0]));
    let state = b.state();
    let ids = shot_ids(&project(&state), a);

    let new_line = ok(state.execute_delete_shot_range_split(
        a,
        ids[2],
        ids[3],
        "A".to_string(),
        "B".to_string(),
    ))
    .line_id
    .unwrap();
    let after = project(&state);
    assert_eq!(after.line(other).unwrap().start, junction);
    assert_eq!(
        after.topology().slot_on(junction, new_line),
        Some(StationRef::new(new_line, 1))
    );
}

/// 0.6.0 had to add a replacement merge between two lines bridged only by
/// an interior station the range removed. The station is now a junction of
/// those lines: the range takes it off its own line and it stays for them.
#[test]
fn an_interior_junction_the_range_removes_stays_for_its_other_lines() {
    for split in [true, false] {
        let mut b = Build::new();
        let a = b.free("Main", six_shots());
        let bridge = b.st(a, 3);
        let line_p = b.from("P", bridge, shots(&[4.0]));
        let line_q = b.from("Q", bridge, shots(&[4.0]));
        let state = b.state();
        let ids = shot_ids(&project(&state), a);

        let (_, after) = one_exact_entry(&state, || {
            if split {
                state.execute_delete_shot_range_split(
                    a,
                    ids[2],
                    ids[3],
                    "A".to_string(),
                    "B".to_string(),
                )
            } else {
                state.execute_delete_shot_range_shift(a, ids[2], ids[3])
            }
        });
        assert_eq!(after.line(line_p).unwrap().start, bridge);
        assert_eq!(
            after.line(line_q).unwrap().start,
            bridge,
            "P and Q stay joined"
        );
        assert!(
            after.topology().slot_on(bridge, a).is_none(),
            "Main left the junction"
        );
    }
}

/// 0.6.0's `test_delete_range_shift_blocked_aborts` blocked the gap-close of
/// a range inside a MERGE loop. On the station graph shots form a forest and
/// a loop is always a closure, which never blocks (0.6.0's later rule), so
/// the block that remains is the datum one: both sides held by a datum.
#[test]
fn a_gap_close_between_two_datum_held_sides_is_refused_and_changes_nothing() {
    let mut b = Build::new();
    let a = b.datum_line("A", shots(&[5.0, 7.0, 3.0, 4.0]), gps(45.0, -73.0));
    let e = b.datum_line("E", shots(&[4.0]), gps(45.0001, -73.0001));
    let (a4, e1) = (b.st(a, 4), b.st(e, 1));
    b.closure(a4, e1, ClosureStrategy::Batch);
    let state = b.state();
    let ids = shot_ids(&project(&state), a);

    let error = assert_refused_changes_nothing(&state, || {
        state.execute_delete_shot_range_shift(a, ids[1], ids[2])
    });
    assert_eq!(
        error,
        "Both sides are anchored to a zero datum. Use split instead."
    );
}

#[test]
fn a_range_split_undoes_to_every_station_where_it_was() {
    let mut b = Build::new();
    let line = b.free(
        "Original",
        vec![
            along(10.0, 90.0),
            along(5.0, 45.0),
            along(7.0, 90.0),
            along(8.0, 135.0),
            along(6.0, 90.0),
        ],
    );
    let state = b.state();
    let before = project(&state);
    let ids = shot_ids(&before, line);

    let (_, _) = one_exact_entry(&state, || {
        state.execute_delete_shot_range_split(
            line,
            ids[1],
            ids[2],
            "A".to_string(),
            "B".to_string(),
        )
    });
    ok(state.undo());
    let undone = project(&state);
    let stations: Vec<StationId> = before.line(line).unwrap().stations().collect();
    assert_still(&before, &undone, &stations);
    ok(state.redo());
    assert_eq!(project(&state).lines().len(), 2);
}

#[test]
fn the_range_preview_classifies_the_range_and_normalises_its_ends() {
    let mut b = Build::new();
    let line = b.free("S", shots(&[5.0, 7.0, 3.0, 4.0, 6.0]));
    let state = b.state();
    let p = project(&state);
    let ids = shot_ids(&p, line);
    let kind = |a: usize, b: usize| {
        p.preview_delete_shot_range(line, ids[a], ids[b])
            .unwrap()
            .kind
    };
    assert_eq!(kind(0, 4), RangeDeleteKind::WholeLine);
    assert_eq!(kind(0, 2), RangeDeleteKind::Prefix);
    assert_eq!(kind(2, 4), RangeDeleteKind::Suffix);
    assert_eq!(kind(1, 3), RangeDeleteKind::Interior);
    let reversed = p.preview_delete_shot_range(line, ids[3], ids[1]).unwrap();
    assert_eq!((reversed.start, reversed.end), (1, 3));
}

#[test]
fn a_range_split_needs_shots_on_both_sides_and_a_gap_close_a_survivor() {
    let mut b = Build::new();
    let line = b.free("S", shots(&[5.0, 7.0, 3.0]));
    let state = b.state();
    let ids = shot_ids(&project(&state), line);
    let error = assert_refused_changes_nothing(&state, || {
        state.execute_delete_shot_range_split(
            line,
            ids[0],
            ids[1],
            "A".to_string(),
            "B".to_string(),
        )
    });
    assert_eq!(
        error,
        "Range is not interior to the line (split needs shots before and after). Use shift or \
         delete the line instead."
    );
    let error = assert_refused_changes_nothing(&state, || {
        state.execute_delete_shot_range_shift(line, ids[0], ids[2])
    });
    assert_eq!(
        error,
        "Cannot shift-delete an entire line; delete the line instead."
    );
}

/// #95: an interior gap-close removes station `end+1` too (the split keeps it),
/// so a datum there blocks the gap-close; the split's preview does not list
/// it among the datums it removes.
#[test]
fn a_datum_on_the_gap_closes_tail_boundary_blocks_the_gap_close() {
    let mut b = Build::new();
    let line = b.bare("S", shots(&[5.0, 7.0, 3.0, 4.0, 6.0]));
    let tail = b.st(line, 3);
    b.datum(tail, gps(10.0, 20.0));
    let state = b.state();
    let p = project(&state);
    let ids = shot_ids(&p, line);
    let preview = p.preview_delete_shot_range(line, ids[1], ids[2]).unwrap();
    assert!(preview.shift_blocked);
    assert!(
        preview.removed_datums.is_empty(),
        "the split keeps station end+1"
    );

    let error = assert_refused_changes_nothing(&state, || {
        state.execute_delete_shot_range_shift(line, ids[1], ids[2])
    });
    assert_eq!(
        error,
        "Deleting this range would remove a zero datum. Use split instead."
    );
}

#[test]
fn a_datum_a_prefix_delete_would_remove_blocks_it_and_is_listed() {
    let mut b = Build::new();
    let line = b.bare("S", shots(&[5.0, 7.0, 3.0, 4.0]));
    let s1 = b.st(line, 1);
    b.datum(s1, gps(10.0, 20.0));
    let state = b.state();
    let p = project(&state);
    let ids = shot_ids(&p, line);
    let preview = p.preview_delete_shot_range(line, ids[0], ids[1]).unwrap();
    assert!(preview.shift_blocked);
    assert_eq!(preview.removed_datums.len(), 1);
    assert_eq!(preview.removed_datums[0].station, s1);
    let error = refused(&state.execute_delete_shot_range_shift(line, ids[0], ids[1]));
    assert!(error.contains("zero datum"), "{error}");
    assert_eq!(project(&state).datums.len(), 1);
}

#[test]
fn the_range_preview_lists_a_datum_the_split_removes_from_an_interior_station() {
    let mut b = Build::new();
    let line = b.bare("S", shots(&[5.0, 7.0, 3.0, 4.0, 6.0]));
    let interior = b.st(line, 2);
    b.datum(interior, gps(10.0, 20.0));
    let state = b.state();
    let p = project(&state);
    let ids = shot_ids(&p, line);
    let preview = p.preview_delete_shot_range(line, ids[1], ids[2]).unwrap();
    assert_eq!(preview.removed_datums.len(), 1);
    assert_eq!(preview.removed_datums[0].station, interior);
}

#[test]
fn the_range_preview_keeps_a_junction_the_range_takes_off_its_line() {
    let mut b = Build::new();
    let a = b.free("Main", six_shots());
    let bridge = b.st(a, 3);
    b.from("P", bridge, shots(&[4.0]));
    b.from("Q", bridge, shots(&[4.0]));
    let state = b.state();
    let p = project(&state);
    let ids = shot_ids(&p, a);
    let preview = p.preview_delete_shot_range(a, ids[2], ids[3]).unwrap();
    assert!(preview.junctions_kept.contains(&bridge), "{preview:?}");
    assert!(preview.promoted_closures.is_empty());
}

/// A same-line merge A:0 - A:2 of a 0.6.0 file is a `Tie` on the station
/// graph; a prefix delete of the shots it spans is not blocked by it.
#[test]
fn a_prefix_delete_is_not_blocked_by_a_tie_inside_it() {
    let mut b = Build::new();
    let line = b.free("S", shots(&[5.0, 7.0, 3.0, 4.0]));
    let (s0, s2) = (b.st(line, 0), b.st(line, 2));
    b.closure(s0, s2, ClosureStrategy::Tie);
    let state = b.state();
    let p = project(&state);
    let ids = shot_ids(&p, line);
    assert!(
        !p.preview_delete_shot_range(line, ids[0], ids[1])
            .unwrap()
            .shift_blocked
    );
    ok(state.execute_delete_shot_range_shift(line, ids[0], ids[1]));
    assert_eq!(lengths(&project(&state), line), vec![3.0, 4.0]);
}

/// #95 Codex review 4: a suffix delete keeps the head in place even when the
/// tail station it takes off the line holds the datum of another line.
#[test]
fn a_suffix_delete_keeps_the_head_when_the_tail_holds_another_lines_datum() {
    let mut b = Build::new();
    let a = b.bare("A", six_shots());
    let end = b.st(a, 6);
    let line_b = b.from("B", end, shots(&[4.0]));
    b.datum(end, gps(45.0, -73.0));
    let state = b.state();
    let before = project(&state);
    let mut kept: Vec<StationId> = before.line(a).unwrap().stations().take(4).collect();
    kept.extend(before.line(line_b).unwrap().stations());
    let ids = shot_ids(&before, a);

    ok(state.execute_delete_shot_range_shift(a, ids[3], ids[5]));
    let after = project(&state);
    assert_eq!(lengths(&after, a), vec![5.0, 7.0, 3.0]);
    assert_still(&before, &after, &kept);
    assert_eq!(after.datums[0].station, end, "the datum stays with B");
}

#[test]
fn the_range_preview_lists_the_comments_the_gap_close_removes_in_station_order() {
    let mut b = Build::new();
    let line = b.free("S", shots(&[1.0; 9]));
    for i in 2..=7 {
        let s = b.st(line, i);
        b.comment(s, &format!("note {i}"));
    }
    let state = b.state();
    let p = project(&state);
    let ids = shot_ids(&p, line);
    let preview = p.preview_delete_shot_range(line, ids[1], ids[6]).unwrap();
    let stations: Vec<StationId> = preview.removed_comments.iter().map(|(s, _)| *s).collect();
    let expected: Vec<StationId> = (2..=7).map(|i| st(&p, line, i)).collect();
    assert_eq!(
        stations, expected,
        "every removed comment, in station order"
    );
    assert_eq!(preview.removed_comments[0].1, "note 2");
}

#[test]
fn the_range_preview_lists_the_tail_boundary_the_gap_close_removes() {
    let mut b = Build::new();
    let a = b.free("Main", six_shots());
    let boundary = b.st(a, 4);
    b.from("M", boundary, shots(&[4.0]));
    b.comment(boundary, "boundary note");
    let state = b.state();
    let p = project(&state);
    let ids = shot_ids(&p, a);
    let preview = p.preview_delete_shot_range(a, ids[2], ids[3]).unwrap();
    assert!(!preview.shift_blocked);
    assert!(preview
        .removed_slots
        .contains(&(StationRef::new(a, 4), boundary)));
    assert!(preview.junctions_kept.contains(&boundary));
}

#[test]
fn a_closure_loop_does_not_block_a_gap_close() {
    let mut b = Build::new();
    let a = b.free("A", six_shots());
    let line_b = b.from("B", b.st(a, 2), shots(&[4.0]));
    let (a5, b1) = (b.st(a, 5), b.st(line_b, 1));
    b.closure(a5, b1, ClosureStrategy::Batch);
    let state = b.state();
    let p = project(&state);
    let ids = shot_ids(&p, a);
    assert!(
        !p.preview_delete_shot_range(a, ids[2], ids[3])
            .unwrap()
            .shift_blocked
    );
    one_exact_entry(&state, || {
        state.execute_delete_shot_range_shift(a, ids[2], ids[3])
    });
}

/// When the gap-close is blocked only the split is offered, and the split
/// keeps the tail boundary: the preview then neither lists it nor its
/// junction as removed.
#[test]
fn a_blocked_gap_close_previews_the_split() {
    let mut b = Build::new();
    let a = b.bare("A", six_shots());
    let interior = b.st(a, 3);
    let boundary = b.st(a, 4);
    b.datum(interior, gps(45.0, -73.0));
    b.from("M", boundary, shots(&[4.0]));
    let state = b.state();
    let p = project(&state);
    let ids = shot_ids(&p, a);
    let preview = p.preview_delete_shot_range(a, ids[2], ids[3]).unwrap();
    assert!(preview.shift_blocked);
    assert!(
        preview.removed_slots.iter().all(|(_, s)| *s != boundary),
        "{preview:?}"
    );
    assert!(!preview.junctions_kept.contains(&boundary));
}

/// #95 Codex re-review P1: a suffix gap-close leaves a free line hung on the
/// doomed tail where the surveyor put it.
#[test]
fn a_suffix_delete_leaves_a_free_line_on_the_tail_where_it_was() {
    let mut b = Build::new();
    let a = b.free("A", six_shots());
    let line_b = b.from("B", b.st(a, 6), shots(&[4.0, 5.0]));
    let state = b.state();
    let before = project(&state);
    let kept: Vec<StationId> = before.line(line_b).unwrap().stations().collect();
    let ids = shot_ids(&before, a);

    ok(state.execute_delete_shot_range_shift(a, ids[3], ids[5]));
    assert_still(&before, &project(&state), &kept);
}

/// #464 defect B: Y - X - Z with a closure Y1 - Z1 closing the loop; the range
/// takes both junction stations off X, so Y and Z are left joined by the
/// closure alone, and the sweep promotes it to a junction. The station graph
/// has no merge to leave out of alignment: the model stays valid.
#[test]
fn a_gap_close_that_strands_a_closure_promotes_it() {
    let mut b = Build::new();
    let x = b.datum_line("X", shots(&[5.0, 6.0, 7.0, 8.0, 9.0]), gps(45.0, -73.0));
    let y = b.from("Y", b.st(x, 2), vec![along(10.0, 0.0)]);
    let zl = b.from("Z", b.st(x, 3), vec![along(10.0, 90.0)]);
    let (y1, z1) = (b.st(y, 1), b.st(zl, 1));
    b.closure(y1, z1, ClosureStrategy::Batch);
    let state = b.state();
    let ids = shot_ids(&project(&state), x);

    // The gap-close of shots 1..=2 takes stations 2 and 3 off X.
    let (result, after) = one_exact_entry(&state, || {
        state.execute_delete_shot_range_shift(x, ids[1], ids[2])
    });
    assert!(
        after.closures.is_empty(),
        "the stranded closure was promoted"
    );
    assert!(after.validate().is_empty());
    assert!(
        result.warnings.is_empty(),
        "a promotion is logged, not reported"
    );
}

#[test]
fn a_loop_through_the_removed_tail_boundary_does_not_block_a_gap_close() {
    let mut b = Build::new();
    let a = b.free("A", six_shots());
    let e = b.from("E", b.st(a, 1), shots(&[4.0]));
    let (a4, e1) = (b.st(a, 4), b.st(e, 1));
    b.closure(a4, e1, ClosureStrategy::Tie);
    let state = b.state();
    let p = project(&state);
    let ids = shot_ids(&p, a);
    assert!(
        !p.preview_delete_shot_range(a, ids[2], ids[3])
            .unwrap()
            .shift_blocked
    );
    ok(state.execute_delete_shot_range_shift(a, ids[2], ids[3]));
}

#[test]
fn ties_onto_a_removed_interior_station_do_not_block_a_gap_close() {
    let mut b = Build::new();
    let a = b.free("A", six_shots());
    let (a2, a3, a5) = (b.st(a, 2), b.st(a, 3), b.st(a, 5));
    b.from("P", a2, shots(&[4.0]));
    b.from("Q", a5, shots(&[4.0]));
    b.closure(a2, a3, ClosureStrategy::Tie);
    b.closure(a5, a3, ClosureStrategy::Tie);
    let state = b.state();
    let p = project(&state);
    let ids = shot_ids(&p, a);
    assert!(
        !p.preview_delete_shot_range(a, ids[2], ids[3])
            .unwrap()
            .shift_blocked
    );
    let (_, after) = one_exact_entry(&state, || {
        state.execute_delete_shot_range_shift(a, ids[2], ids[3])
    });
    assert!(
        after.closures.is_empty(),
        "the ties left with their station"
    );
}

#[test]
fn the_range_preview_lists_a_comment_a_prefix_delete_removes() {
    let mut b = Build::new();
    let line = b.free("S", shots(&[5.0, 7.0, 3.0, 4.0]));
    let s1 = b.st(line, 1);
    b.comment(s1, "removed note");
    let state = b.state();
    let p = project(&state);
    let ids = shot_ids(&p, line);
    let preview = p.preview_delete_shot_range(line, ids[0], ids[1]).unwrap();
    assert_eq!(
        preview.removed_comments,
        vec![(s1, "removed note".to_string())]
    );
}

#[test]
fn the_range_preview_lists_a_comment_on_the_gap_closes_tail_boundary() {
    let mut b = Build::new();
    let line = b.free("S", six_shots());
    let boundary = b.st(line, 4);
    b.comment(boundary, "boundary note");
    let state = b.state();
    let p = project(&state);
    let ids = shot_ids(&p, line);
    let preview = p.preview_delete_shot_range(line, ids[2], ids[3]).unwrap();
    assert!(!preview.shift_blocked);
    assert!(preview
        .removed_comments
        .contains(&(boundary, "boundary note".to_string())));
}

#[test]
fn a_range_with_a_frozen_shot_cannot_be_deleted_either_way() {
    let (b, a, _) = frozen_loop(&[5.0, 7.0, 4.0, 3.0]);
    let state = b.state();
    let ids = shot_ids(&project(&state), a);
    let expected = "Cannot delete a frozen shot. This range includes shots that are part of an \
                    adjusted survey network.";
    let error = assert_refused_changes_nothing(&state, || {
        state.execute_delete_shot_range_split(a, ids[1], ids[2], "A1".to_string(), "A2".to_string())
    });
    assert_eq!(error, expected);
    let error = assert_refused_changes_nothing(&state, || {
        state.execute_delete_shot_range_shift(a, ids[1], ids[2])
    });
    assert_eq!(error, expected);
}

// ===== Delete last shot and line ============================================

#[test]
fn deleting_the_last_shot_removes_the_line_in_one_exact_entry() {
    let mut b = Build::new();
    let line = b.free("WillDelete", shots(&[5.0]));
    b.free_at("Other", shots(&[3.0]), [20.0, 0.0]);
    let state = b.state();
    let only = shot_ids(&project(&state), line)[0];

    let (result, after) = one_exact_entry(&state, || {
        state.execute_delete_last_shot_and_line(line, only)
    });
    assert!(result.warnings.is_empty());
    assert!(after.line(line).is_none());
    let (label, kind, rows) = top_entry(&state);
    assert_eq!(kind, "batch");
    assert_eq!(label, "Delete line \"WillDelete\" with its last shot");
    assert_eq!(
        rows,
        vec![
            ("Delete shot".to_string(), String::new()),
            ("Delete line \"WillDelete\"".to_string(), String::new()),
        ]
    );
}

#[test]
fn deleting_the_last_shot_refuses_a_line_with_more_shots() {
    let mut b = Build::new();
    let line = b.free("Two", shots(&[5.0, 6.0]));
    let state = b.state();
    let first = shot_ids(&project(&state), line)[0];
    let error = assert_refused_changes_nothing(&state, || {
        state.execute_delete_last_shot_and_line(line, first)
    });
    assert_eq!(error, "That line has 2 shots, not exactly 1.");
}

#[test]
fn undoing_the_last_shot_delete_restores_the_line_its_shot_and_its_credit() {
    let credit = SurveyCredit::from_legacy(Some("UndoTester"), None)
        .stored()
        .map(std::sync::Arc::new);
    let mut b = Build::new();
    let line = b.free(
        "WillDelete",
        vec![ShotData {
            credit: credit.clone(),
            ..shot(5.0)
        }],
    );
    let state = b.state();
    let only = shot_ids(&project(&state), line)[0];

    ok(state.execute_delete_last_shot_and_line(line, only));
    ok(state.undo());
    let after = project(&state);
    let l = after.line(line).unwrap();
    assert_eq!(l.name.as_deref(), Some("WillDelete"));
    assert_eq!(l.shots.len(), 1);
    assert_eq!(l.shots[0].id, only);
    assert_eq!(l.shots[0].length, 5.0);
    assert_eq!(l.shots[0].credit, credit);
}

/// 0.6.0's cleanup tests (line offsets, merges, station offsets, closures
/// referencing the deleted line): on the station graph the line's own
/// stations leave with their placement, comments, closures and hand moves; a
/// junction stays for its other lines, untouched; one undo restores all.
#[test]
fn deleting_the_last_shot_takes_the_lines_own_stations_and_keeps_a_junction() {
    let mut b = Build::new();
    let a = b.free("A", shots(&[5.0]));
    let (a0, a1) = (b.st(a, 0), b.st(a, 1));
    let line_b = b.from("B", a1, shots(&[4.0, 3.0]));
    let b2 = b.st(line_b, 2);
    b.comment(a0, "own note");
    b.comment(a1, "junction note");
    b.closure(a0, b2, ClosureStrategy::Batch);
    b.0.line_mut(a)
        .unwrap()
        .hand_moves
        .insert(a0, [0.5, 0.5, 0.0]);
    let state = b.state();
    let before = project(&state);
    let kept: Vec<StationId> = before.line(line_b).unwrap().stations().collect();
    let only = shot_ids(&before, a)[0];

    let (_, after) = one_exact_entry(&state, || state.execute_delete_last_shot_and_line(a, only));
    assert!(after.line(a).is_none());
    assert!(after.station(a0).is_none(), "A's own station left");
    assert!(after.closures.is_empty(), "its closure left with it");
    assert_eq!(
        after.line(line_b).unwrap().start,
        a1,
        "the junction stays for B"
    );
    assert_eq!(
        after.station(a1).unwrap().comment.as_deref(),
        Some("junction note")
    );
    assert!(after
        .placements
        .iter()
        .all(|p| after.station(p.station).is_some()));
    assert_still(&before, &after, &kept);
}

// ===== Coverage gaps: refusals on the values a shot edit sets ===============

#[test]
fn a_new_line_with_a_negative_length_shot_is_refused() {
    let state = Build::new().state();
    let error = refused(&state.execute(Command::CreateLine {
        name: Some("Bad".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(-1.0)],
        cave_section_id: None,
    }));
    assert!(error.contains("negative length"), "{error}");
}

#[test]
fn a_zero_length_new_shot_is_refused() {
    let mut b = Build::new();
    let line = b.free("Line", shots(&[5.0]));
    let state = b.state();
    let error =
        assert_refused_changes_nothing(&state, || create_shot(&state, line, shot(0.0), None, None));
    assert_eq!(error, "The new shot in line \"Line\" has zero length");
    let error = refused(&create_shot(&state, line, shot(-2.0), None, None));
    assert_eq!(
        error,
        "The new shot in line \"Line\" has negative length: -2.0m"
    );
}

#[test]
fn an_update_that_sets_a_zero_or_negative_length_is_refused() {
    let mut b = Build::new();
    let line = b.free("Line", shots(&[5.0]));
    let state = b.state();
    let shot_id = shot_ids(&project(&state), line)[0];
    let set = |length: f64| {
        state.execute(Command::UpdateShot {
            line_id: line,
            shot_id,
            updates: ShotUpdates {
                length: Some(length),
                ..ShotUpdates::default()
            },
        })
    };
    let error = assert_refused_changes_nothing(&state, || set(0.0));
    assert_eq!(error, "Shot Line: 0 \u{2192} 1 has zero length");
    let error = assert_refused_changes_nothing(&state, || set(-3.0));
    assert_eq!(error, "Shot Line: 0 \u{2192} 1 has negative length: -3.0m");
}

/// A zero-length shot loads (a validation finding, not a refusal); an edit
/// that does not set the length must not be judged by it (#413), and the
/// undo of a kind change on it must not wedge the history.
#[test]
fn a_zero_length_shot_takes_kind_edits_and_their_undo_drains_the_history() {
    let mut b = Build::new();
    let line = b.free("Line", shots(&[5.0, 6.0]));
    b.0.line_mut(line).unwrap().shots[1].length = 0.0;
    let state = b.state();
    let ids = shot_ids(&project(&state), line);
    let kind_of = |i: usize| project(&state).line(line).unwrap().shots[i].kind;

    ok(state.execute(Command::UpdateShot {
        line_id: line,
        shot_id: ids[1],
        updates: ShotUpdates {
            kind: Some(ShotKind::Jump),
            ..ShotUpdates::default()
        },
    }));
    ok(state.execute(Command::SetShotKind {
        line_id: line,
        shot_id: ids[0],
        kind: ShotKind::Jump,
    }));
    ok(state.execute(Command::SetShotKind {
        line_id: line,
        shot_id: ids[1],
        kind: ShotKind::Normal,
    }));
    ok(state.undo());
    assert_eq!(kind_of(1), ShotKind::Jump);
    ok(state.undo());
    assert_eq!(kind_of(0), ShotKind::Normal);
    ok(state.undo());
    assert_eq!(kind_of(1), ShotKind::Normal);
    assert!(!state.can_undo(), "the whole history drained");
}

#[test]
fn a_new_shot_refuses_both_anchors_and_takes_no_client_identity() {
    let mut b = Build::new();
    let line = b.free("Line", shots(&[5.0, 6.0]));
    let state = b.state();
    let p = project(&state);
    let ids = shot_ids(&p, line);
    let error = assert_refused_changes_nothing(&state, || {
        create_shot(&state, line, shot(2.0), Some(ids[0]), Some(ids[1]))
    });
    assert_eq!(
        error,
        "Cannot specify both after_shot_id and before_shot_id"
    );

    let taken = p.line(line).unwrap().shots[0].uuid.clone();
    for uuid in [taken.clone(), "client-chosen".to_string()] {
        let created = ok(create_shot(
            &state,
            line,
            ShotData {
                uuid: Some(uuid.clone()),
                ..shot(2.0)
            },
            None,
            None,
        ));
        let after = project(&state);
        let made = after
            .line(line)
            .unwrap()
            .shots
            .iter()
            .find(|s| Some(s.id) == created.shot_id)
            .unwrap();
        assert_ne!(made.uuid, uuid, "the project mints the identity");
        assert_eq!(
            after.line(line).unwrap().shots[0].uuid,
            taken,
            "the original keeps its own"
        );
    }
}

#[test]
fn a_new_shot_takes_its_neighbours_credit_and_reports_its_validation_findings() {
    let credit = SurveyCredit::from_legacy(Some("Neighbour"), None)
        .stored()
        .map(std::sync::Arc::new);
    let mut b = Build::new();
    let line = b.free(
        "Line",
        vec![ShotData {
            credit: credit.clone(),
            ..shot(5.0)
        }],
    );
    let state = b.state();
    let created = ok(create_shot(&state, line, shot(500.0), None, None));
    let after = project(&state);
    let made = &after.line(line).unwrap().shots[1];
    assert_eq!(
        made.credit, credit,
        "a new shot continues its neighbour's survey"
    );
    assert!(
        created
            .warnings
            .iter()
            .any(|w| w.message.contains("Line: 1 \u{2192} 2")),
        "the finding names the shot as the user reads it: {:?}",
        created.warnings
    );
}

// ===== Shots inserted and deleted near junctions ============================

#[test]
fn a_shot_inserted_before_a_junction_moves_the_junction_one_index_along() {
    let mut b = Build::new();
    let a = b.free("A", shots(&[5.0, 7.0, 3.0]));
    let junction = b.st(a, 2);
    let line_b = b.from("B", junction, shots(&[4.0]));
    let state = b.state();
    let second = shot_ids(&project(&state), a)[1];

    let (_, after) = one_exact_entry(&state, || {
        create_shot(&state, a, shot(2.0), None, Some(second))
    });
    assert_eq!(after.line(a).unwrap().shots.len(), 4);
    assert_eq!(
        after.topology().slot_on(junction, a),
        Some(StationRef::new(a, 3))
    );
    assert_eq!(
        after.line(line_b).unwrap().start,
        junction,
        "B is still on it"
    );
}

#[test]
fn a_shot_inserted_after_a_junction_leaves_it_in_place() {
    let mut b = Build::new();
    let a = b.free("A", shots(&[5.0, 7.0, 3.0]));
    let junction = b.st(a, 1);
    let line_b = b.from("B", junction, shots(&[4.0]));
    let state = b.state();
    let first = shot_ids(&project(&state), a)[0];

    ok(create_shot(&state, a, shot(2.0), Some(first), None));
    let after = project(&state);
    assert_eq!(
        after.topology().slot_on(junction, a),
        Some(StationRef::new(a, 1))
    );
    assert_eq!(after.line(line_b).unwrap().start, junction);
}

#[test]
fn a_mid_line_delete_closes_the_gap_and_keeps_a_junction_on_the_head() {
    let mut b = Build::new();
    let a = b.free("A", shots(&[5.0, 7.0, 3.0]));
    let junction = b.st(a, 0);
    let line_b = b.from("B", junction, shots(&[4.0]));
    let state = b.state();
    let second = shot_ids(&project(&state), a)[1];

    let (_, after) = one_exact_entry(&state, || delete_shot(&state, a, second));
    assert_eq!(
        after.lines().len(),
        2,
        "a single delete closes the gap, it splits nothing"
    );
    assert_eq!(lengths(&after, a), vec![5.0, 3.0]);
    assert_eq!(after.line(line_b).unwrap().start, junction);
    assert!(after.validate().is_empty());
}

#[test]
fn a_prepended_shot_keeps_the_old_start_and_its_junction_in_place() {
    let mut b = Build::new();
    let a = b.free("A", shots(&[5.0, 7.0]));
    let (old_start, s1) = (b.st(a, 0), b.st(a, 1));
    let line_b = b.from("B", old_start, shots(&[4.0]));
    let state = b.state();
    let before = project(&state);
    let first = shot_ids(&before, a)[0];

    let (_, after) = one_exact_entry(&state, || {
        create_shot(&state, a, shot(3.0), None, Some(first))
    });
    assert_eq!(
        st(&after, a, 1),
        old_start,
        "the new station is the new start"
    );
    assert_eq!(after.line(line_b).unwrap().start, old_start);
    assert_still(&before, &after, &[old_start, s1]);
}

#[test]
fn deleting_a_one_shot_line_on_a_junction_leaves_the_other_line_whole() {
    let mut b = Build::new();
    let a = b.free("A", shots(&[5.0]));
    let line_b = b.from("B", b.st(a, 1), shots(&[4.0, 2.0]));
    let state = b.state();
    let before = project(&state);
    let only = shot_ids(&before, a)[0];

    ok(state.execute_delete_last_shot_and_line(a, only));
    let after = project(&state);
    assert!(after.line(a).is_none());
    assert_eq!(after.line(line_b).unwrap().shots.len(), 2);
    assert!(after.validate().is_empty());
}

#[test]
fn a_prepended_shot_leaves_a_datum_on_its_station() {
    let mut b = Build::new();
    let a = b.bare("A", shots(&[5.0, 7.0]));
    let s1 = b.st(a, 1);
    b.datum(s1, gps(45.0, -73.0));
    let state = b.state();
    let first = shot_ids(&project(&state), a)[0];

    ok(create_shot(&state, a, shot(3.0), None, Some(first)));
    let after = project(&state);
    assert_eq!(after.datums.len(), 1);
    assert_eq!(after.datums[0].station, s1);
    assert_eq!(after.topology().slot_on(s1, a), Some(StationRef::new(a, 2)));
}

// ===== Closure warnings =====================================================

/// #738 through a delete: two closures tie the same two stations of pieces
/// the delete leaves joined by them alone. The sweep promotes the first; the
/// second's ends are then one station, so it goes too, and the delete reports
/// it by the stations the user created it between (0.6.0's code and words).
#[test]
fn a_closure_whose_ends_a_promotion_joins_is_reported_by_its_stations_before_the_delete() {
    let mut b = Build::new();
    let x = b.datum_line("X", shots(&[5.0, 6.0, 7.0, 8.0, 9.0]), gps(45.0, -73.0));
    let y = b.from("Y", b.st(x, 2), vec![along(10.0, 0.0)]);
    let zl = b.from("Z", b.st(x, 3), vec![along(10.0, 90.0)]);
    let (y1, z1) = (b.st(y, 1), b.st(zl, 1));
    b.closure(y1, z1, ClosureStrategy::Batch);
    b.closure(y1, z1, ClosureStrategy::LeastSquaresWeighted);
    let state = b.state();
    let ids = shot_ids(&project(&state), x);

    let (result, after) = one_exact_entry(&state, || {
        state.execute_delete_shot_range_shift(x, ids[1], ids[2])
    });
    assert!(after.closures.is_empty());
    assert_eq!(result.warnings.len(), 1, "{:?}", result.warnings);
    let warning = &result.warnings[0];
    assert_eq!(warning.code, crate::models::CLOSURE_REMOVED_MERGED_ENDS);
    assert_eq!(
        warning.message,
        "Removed loop closure Y / 1 to Z / 1: another loop closure became a merge, which made \
         its two stations one point."
    );
}

// ----- a paste: a batch of shot updates (D2) ---------------------------------

/// A tree held by a datum (its branches entered at junctions, one with a
/// snap closure), a free tree, and a third free piece, for pastes.
fn paste_survey() -> (Build, [LineId; 5]) {
    let mut b = Build::new();
    let held = b.datum_line(
        "Held",
        (0..8)
            .map(|i| along(3.0 + f64::from(i) * 0.5, 80.0))
            .collect(),
        gps(20.5, -87.25),
    );
    let branch = b.from(
        "Branch",
        b.st(held, 3),
        (0..6).map(|i| along(2.0 + f64::from(i), 170.0)).collect(),
    );
    let snap_a = b.st(branch, 6);
    let snap_b = b.st(held, 7);
    b.closure(snap_a, snap_b, ClosureStrategy::Move);
    let free = b.free_at(
        "Free",
        (0..7).map(|i| along(4.0, 10.0 * f64::from(i))).collect(),
        [100.0, -40.0],
    );
    let twig = b.from(
        "Twig",
        b.st(free, 4),
        (0..5)
            .map(|i| along(2.5, 300.0 - 7.0 * f64::from(i)))
            .collect(),
    );
    let island = b.free_at("Island", shots(&[5.0, 6.0, 7.0]), [-50.0, 60.0]);
    (b, [held, branch, free, twig, island])
}

/// A paste over every shot of `lines`: geometry and annotation cells mixed,
/// each value different.
fn paste_commands(p: &Project, lines: &[LineId]) -> Vec<Command> {
    let mut commands = Vec::new();
    for &line in lines {
        for (k, id) in shot_ids(p, line).into_iter().enumerate() {
            let n = commands.len() as f64;
            let updates = match k % 4 {
                0 => ShotUpdates {
                    length: Some(2.0 + n * 0.37),
                    ..ShotUpdates::default()
                },
                1 => ShotUpdates {
                    azimuth_in: Some(5.0 + n * 3.1),
                    azimuth_out: Some(5.0 + n * 3.1),
                    ..ShotUpdates::default()
                },
                2 => ShotUpdates {
                    depth_out: Some(12.0 + n * 0.2),
                    left: Some(Some(n * 0.1)),
                    ..ShotUpdates::default()
                },
                _ => ShotUpdates {
                    comment: Some(Some(format!("cell {n}"))),
                    ..ShotUpdates::default()
                },
            };
            commands.push(Command::UpdateShot {
                line_id: line,
                shot_id: id,
                updates,
            });
        }
    }
    commands
}

/// A paste is its updates one after another: the batch (one survey pass)
/// lands on the stored model the same updates land on one at a time, to the
/// bit, with one entry whose undo and redo are exact. Free pieces move their
/// placements at every geometry cell, as each update does on its own.
#[test]
fn a_paste_lands_where_its_updates_one_at_a_time_land() {
    let (b, lines) = paste_survey();
    let base = b.0.clone();
    let batched = b.state();
    let single = {
        let state = AppState::new().unwrap();
        state.set_project_for_tests(base.clone());
        state
    };
    let commands = paste_commands(&base, &lines);
    assert!(commands.len() > 25);
    for command in commands.clone() {
        ok(single.execute(command));
    }
    let (_, after) = one_exact_entry(&batched, || {
        batched.execute(Command::Batch {
            commands,
            label: None,
        })
    });
    let expected = project(&single);
    assert_ne!(after.placements, base.placements, "the free pieces moved");
    assert_eq!(canonical(&after), canonical(&expected));
    let bits = |p: &Project| -> Vec<(StationId, [u64; 3])> {
        p.raw_positions()
            .iter()
            .map(|(s, [x, y, z])| (s, [x.to_bits(), y.to_bits(), z.to_bits()]))
            .collect()
    };
    assert_eq!(bits(&after), bits(&expected));
}

/// A paste that reaches a frozen shot is refused with the frozen-shot words
/// and changes nothing, as the update alone is; so is one whose last cell
/// is a zero length (the model's refusal, after every other cell applied).
#[test]
fn a_paste_with_a_refused_cell_changes_nothing() {
    let (mut b, lines) = paste_survey();
    // A tie from the branch back onto the held line closes a loop on the
    // datum's piece: the shots around it are frozen.
    let (a, z) = (b.st(lines[1], 2), b.st(lines[0], 6));
    b.closure(a, z, ClosureStrategy::Tie);
    let state = b.state();
    let p = project(&state);
    let mut frozen = paste_commands(&p, &lines[2..]);
    frozen.push(Command::UpdateShot {
        line_id: lines[1],
        shot_id: shot_ids(&p, lines[1])[1],
        updates: ShotUpdates {
            length: Some(9.0),
            ..ShotUpdates::default()
        },
    });
    let error = assert_refused_changes_nothing(&state, || {
        state.execute(Command::Batch {
            commands: frozen,
            label: None,
        })
    });
    assert_eq!(error, super::commands::FROZEN_GEOMETRY);
    let mut zero = paste_commands(&p, &lines[2..]);
    zero.push(Command::UpdateShot {
        line_id: lines[4],
        shot_id: shot_ids(&p, lines[4])[1],
        updates: ShotUpdates {
            length: Some(0.0),
            ..ShotUpdates::default()
        },
    });
    let error = assert_refused_changes_nothing(&state, || {
        state.execute(Command::Batch {
            commands: zero,
            label: None,
        })
    });
    assert!(error.contains("has zero length"), "{error}");
}
