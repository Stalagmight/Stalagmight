//! Delete Shot with two loop closures on the deleted station (#776). The first
//! closure whose loop the delete breaks becomes a merge; once it is made, the
//! next closure's two ends may reach each other through it, and then that
//! closure is still a loop and must stay one, not be promoted too. 0.6.0
//! promoted both and welded one station onto two stations 40 m apart.
//! Synthetic surveys only.
//!
//! Ported to the station graph. Its sweep judges each closure on the topology
//! the promotions before it left (`survey::sweep`), so the second closure is
//! kept by construction; these pin it through the commands, the preview the
//! dialog shows and the undo entry. The junction the promotion makes is one
//! station (`C:1` IS `A:1`), so "re-anchored onto C:1" reads as "kept on the
//! junction", and a merge "out of alignment" is the junction's lines
//! disagreeing about where it is.

use super::AppState;
use crate::core::Command;
use crate::models::survey::Project;
use crate::models::{
    local_to_gps, ClosureStrategy, GpsCoordinates, LineId, ShotData, StationId, StationRef,
    SurveyDirection,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};

const ORIGIN: (f64, f64) = (20.5, -87.25);

fn origin() -> GpsCoordinates {
    GpsCoordinates::new(ORIGIN.0, ORIGIN.1, 0.0).unwrap()
}

/// The GPS position `east`/`north` metres from the origin.
fn gps_at(east: f64, north: f64) -> GpsCoordinates {
    let (lat, lon) = local_to_gps(east, north, &origin()).unwrap();
    GpsCoordinates::new(lat, lon, 0.0).unwrap()
}

fn shots(count: usize, azimuth: f64, length: f64) -> Vec<ShotData> {
    (0..count)
        .map(|_| ShotData {
            azimuth_in: azimuth,
            azimuth_out: azimuth,
            length,
            ..Default::default()
        })
        .collect()
}

fn create_line(state: &AppState, name: &str, shots: Vec<ShotData>) -> LineId {
    state
        .execute(Command::CreateLine {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#123456".to_string(),
            direction: Some(SurveyDirection::In),
            shots,
            cave_section_id: None,
        })
        .line_id
        .unwrap()
}

fn ok(result: crate::core::CommandResult) {
    assert!(result.success, "{:?}", result.error);
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

/// The station at slot `(line, index)`.
fn st(p: &Project, line: LineId, index: u32) -> StationId {
    p.station_at(StationRef::new(line, index)).unwrap()
}

/// Every closure as its sorted pair of stations.
fn closures(p: &Project) -> Vec<(StationId, StationId)> {
    let mut out: Vec<_> = p
        .closures
        .iter()
        .map(|c| (c.a.min(c.b), c.a.max(c.b)))
        .collect();
    out.sort_unstable();
    out
}

fn pair(a: StationId, b: StationId) -> (StationId, StationId) {
    (a.min(b), a.max(b))
}

/// The raw horizontal position of slot `(line, index)`.
fn xy(p: &Project, line: LineId, index: u32) -> [f64; 2] {
    p.raw_positions().xy(st(p, line, index)).unwrap()
}

fn near(a: [f64; 2], b: [f64; 2], what: &str) {
    assert!(
        (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6,
        "{what}: {a:?} vs {b:?}"
    );
}

/// No invariant broken, and every junction's lines agree where it is: each
/// line laid out on its own from its first station puts every one of its
/// stations where the survey does (0.6.0's "merged stations N m apart").
fn assert_sound(state: &AppState) {
    let messages = state.integrity_check().messages;
    assert!(messages.is_empty(), "{messages:?}");
    let p = project(state);
    let raw = p.raw_positions();
    for line in p.lines() {
        let mut at = raw.xy(line.start).unwrap();
        for shot in &line.shots {
            let h = shot.horizontal_length();
            let azimuth = shot.azimuth_in.to_radians();
            at = [at[0] + h * azimuth.sin(), at[1] + h * azimuth.cos()];
            near(
                raw.xy(shot.to).unwrap(),
                at,
                &format!("line {:?} station {}", line.name, shot.to),
            );
        }
    }
}

/// The issue's survey: A runs 3 x 20 m east from its own datum; B runs 2 x 10 m
/// north from a second datum 20 m east and 30 m south of A:0; C hangs one 10 m
/// shot off B's end (C:1 merged onto B:2: a junction). B:2 is closed onto A at
/// `closures_to`, in that order. Returns the state and the three lines.
fn two_closures_on_one_station(closures_to: [u32; 2]) -> (AppState, LineId, LineId, LineId) {
    let state = AppState::new().unwrap();
    ok(state.execute(Command::CreateProject {
        name: "Weld".to_string(),
        location: None,
    }));
    let a = create_line(&state, "A", shots(3, 90.0, 20.0));
    let b = create_line(&state, "B", shots(2, 0.0, 10.0));
    let c = create_line(&state, "C", shots(1, 270.0, 10.0));
    for (station_ref, gps) in [
        (StationRef::new(a, 0), origin()),
        (StationRef::new(b, 0), gps_at(20.0, -30.0)),
    ] {
        ok(state.execute(Command::SetZeroDatum {
            station_ref,
            coordinates: gps,
            accuracy_m: None,
            entrance: None,
        }));
    }
    ok(state.execute(Command::MergeStations {
        station_a: StationRef::new(c, 1),
        station_b: StationRef::new(b, 2),
    }));
    for to in closures_to {
        ok(state.execute(Command::CreateLoopClosure {
            station_a: StationRef::new(b, 2),
            station_b: StationRef::new(a, to),
            closure_strategy: ClosureStrategy::Batch,
        }));
    }
    assert_sound(&state);
    let p = project(&state);
    assert_eq!(st(&p, c, 1), st(&p, b, 2), "C:1 is the junction B:2");
    near(xy(&p, c, 0), [30.0, -10.0], "C:0 before the delete");
    assert_eq!(p.closures.len(), 2);
    (state, a, b, c)
}

fn last_shot_of(p: &Project, line: LineId) -> u32 {
    p.line(line).unwrap().shots.last().unwrap().id
}

fn delete_last_shot_of(state: &AppState, line: LineId) {
    let last = last_shot_of(&project(state), line);
    ok(state.execute(Command::DeleteShot {
        line_id: line,
        shot_id: last,
    }));
}

#[test]
fn delete_shot_does_not_weld_two_closures_onto_one_line() {
    let (state, a, b, c) = two_closures_on_one_station([1, 3]);
    delete_last_shot_of(&state, b);
    // Before #776's fix (0.6.0): ["Merged stations 2:1 and 0:3 are 40.00 m
    // apart but should be the same point"].
    assert_sound(&state);
    let after = project(&state);
    // The first closure's loop is broken, so it becomes the junction
    // C:1 = A:1 (C moves 10 m north). C:1 then reaches A:3 along A's own
    // shots, so the second closure is still a loop: kept on the junction.
    assert_eq!(st(&after, c, 1), st(&after, a, 1));
    assert_eq!(after.merge_pairs().len(), 1, "{:?}", after.merge_pairs());
    assert_eq!(
        closures(&after),
        vec![pair(st(&after, a, 1), st(&after, a, 3))]
    );
    near(xy(&after, c, 0), [30.0, 0.0], "C moved onto A:1");
    near(xy(&after, a, 3), [60.0, 0.0], "A stays");
    near(xy(&after, b, 0), [20.0, -30.0], "B stays");
    assert_eq!(after.line(b).unwrap().shots.len(), 1);
}

#[test]
fn the_first_closure_in_the_list_is_the_one_promoted() {
    // Same survey, closures created the other way round: the first one decided
    // (B:2 to A:3) is promoted and the second (to A:1) kept. Either way one
    // junction and one closure, and no station claimed at two places.
    let (state, a, b, c) = two_closures_on_one_station([3, 1]);
    delete_last_shot_of(&state, b);
    assert_sound(&state);
    let after = project(&state);
    assert_eq!(st(&after, c, 1), st(&after, a, 3));
    assert_eq!(after.merge_pairs().len(), 1);
    assert_eq!(
        closures(&after),
        vec![pair(st(&after, a, 1), st(&after, a, 3))]
    );
    near(xy(&after, c, 0), [70.0, 0.0], "C moved onto A:3");
}

#[test]
fn the_delete_dialog_promises_what_the_delete_does() {
    let (state, a, b, c) = two_closures_on_one_station([1, 3]);
    let before = project(&state);
    let last = last_shot_of(&before, b);
    let (first, second) = (before.closures[0].id, before.closures[1].id);
    // What the IPC `preview_delete_shot` returns.
    let preview = before.preview_delete_shot(b, last).unwrap();
    let impact = before.delete_shot_impact(&preview);
    let promoted: Vec<_> = impact
        .closure_to_merge
        .iter()
        .map(|p| (p.station_a, p.station_b))
        .collect();
    assert_eq!(
        promoted,
        vec![(StationRef::new(b, 2), StationRef::new(a, 1))],
        "{impact:?}"
    );
    assert_eq!(preview.promoted_closures.len(), 1);
    assert_eq!(preview.promoted_closures[0].closure.id, first);
    assert!(impact.removed_closures.is_empty(), "{impact:?}");
    assert!(!impact.shift_blocked);

    // The delete does exactly that: the promoted pair is the one junction,
    // and the kept closure keeps its id and now ends at C:1 = A:1.
    ok(state.execute(Command::DeleteShot {
        line_id: b,
        shot_id: last,
    }));
    let after = project(&state);
    assert_eq!(after.closures.len(), 1);
    let kept = &after.closures[0];
    assert_eq!(kept.id, second);
    assert_eq!(
        pair(kept.a, kept.b),
        pair(st(&after, c, 1), st(&after, a, 3))
    );
    assert!(after.closures.iter().all(|c| c.id != first));
    // The preview ran the delete itself on a copy: the same project.
    let mut previewed = before;
    previewed.delete_shot(b, last).unwrap();
    assert_eq!(canonical(&previewed), canonical(&after));
}

/// The v2 text of `p` without the edit clock and the id allocators (which
/// stand higher after an undo).
fn canonical(p: &Project) -> String {
    let mut p = p.clone();
    p.modified_at = p.created_at;
    p.file_path = None;
    let bytes = stalx_v2::encode(&p, &Extras::default(), Mode::Normal).unwrap();
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with("\"counters\""))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn undo_and_redo_of_the_delete_are_exact() {
    let (state, _a, b, _c) = two_closures_on_one_station([1, 3]);
    let before = canonical(&project(&state));
    delete_last_shot_of(&state, b);
    let after = canonical(&project(&state));
    assert_ne!(before, after);
    ok(state.undo());
    assert_eq!(canonical(&project(&state)), before);
    assert_sound(&state);
    ok(state.redo());
    assert_eq!(canonical(&project(&state)), after);
    assert_sound(&state);
}

#[test]
fn one_closure_on_the_deleted_station_is_still_promoted() {
    // The control from the issue: with only the B:2 to A:1 closure, deleting B's
    // last shot breaks that loop, so the closure becomes the junction C:1 = A:1.
    let state = AppState::new().unwrap();
    ok(state.execute(Command::CreateProject {
        name: "Control".to_string(),
        location: None,
    }));
    let a = create_line(&state, "A", shots(3, 90.0, 20.0));
    let b = create_line(&state, "B", shots(2, 0.0, 10.0));
    let c = create_line(&state, "C", shots(1, 270.0, 10.0));
    ok(state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: origin(),
        accuracy_m: None,
        entrance: None,
    }));
    ok(state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(b, 0),
        coordinates: gps_at(20.0, -30.0),
        accuracy_m: None,
        entrance: None,
    }));
    ok(state.execute(Command::MergeStations {
        station_a: StationRef::new(c, 1),
        station_b: StationRef::new(b, 2),
    }));
    ok(state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(b, 2),
        station_b: StationRef::new(a, 1),
        closure_strategy: ClosureStrategy::Batch,
    }));
    delete_last_shot_of(&state, b);
    assert_sound(&state);
    let after = project(&state);
    assert_eq!(st(&after, c, 1), st(&after, a, 1));
    assert_eq!(after.merge_pairs().len(), 1);
    assert!(closures(&after).is_empty());
}
