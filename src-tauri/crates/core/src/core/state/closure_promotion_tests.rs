//! Delete Shot with two loop closures on the deleted station (#776). The first
//! closure whose loop the delete breaks becomes a merge; once it is planned, the
//! next closure's two ends may reach each other through it, and then that
//! closure is still a loop and must be re-anchored, not promoted too. Promoting
//! both welded one station onto two stations 40 m apart. Synthetic surveys only.

use super::AppState;
use crate::core::Command;
use crate::models::{
    local_to_gps, ClosureStrategy, GpsCoordinates, Project, SectionId, ShotData, StationRef,
    SurveyDirection,
};

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

fn create_section(state: &AppState, name: &str, shots: Vec<ShotData>) -> SectionId {
    state
        .execute(Command::CreateSection {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#123456".to_string(),
            direction: Some(SurveyDirection::In),
            shots,
        })
        .section_id
        .unwrap()
}

fn ok(result: crate::core::CommandResult) {
    assert!(result.success, "{:?}", result.error);
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

/// Every merge as a sorted pair of positional refs.
fn merges(p: &Project) -> Vec<(StationRef, StationRef)> {
    let mut out: Vec<_> = p
        .station_merges
        .iter()
        .map(|m| {
            let (a, b) = (
                p.resolve_key(&m.station_a).unwrap(),
                p.resolve_key(&m.station_b).unwrap(),
            );
            if (a.section_id, a.station_index) <= (b.section_id, b.station_index) {
                (a, b)
            } else {
                (b, a)
            }
        })
        .collect();
    out.sort_by_key(|(a, b)| (a.section_id, a.station_index, b.section_id, b.station_index));
    out
}

/// Every loop closure as a sorted pair of positional refs.
fn closures(p: &Project) -> Vec<(StationRef, StationRef)> {
    let mut out: Vec<_> = p
        .loop_closures
        .iter()
        .map(|c| {
            let (a, b) = (
                p.resolve_key(&c.station_a).unwrap(),
                p.resolve_key(&c.station_b).unwrap(),
            );
            if (a.section_id, a.station_index) <= (b.section_id, b.station_index) {
                (a, b)
            } else {
                (b, a)
            }
        })
        .collect();
    out.sort_by_key(|(a, b)| (a.section_id, a.station_index, b.section_id, b.station_index));
    out
}

/// The issue's survey: A runs 3 x 20 m east from its own datum; B runs 2 x 10 m
/// north from a second datum 20 m east and 30 m south of A:0; C hangs one 10 m
/// shot off B's end (C:1 merged onto B:2). B:2 is closed onto A at `closures_to`,
/// in that order. Returns the state and the three sections.
fn two_closures_on_one_station(
    closures_to: [u32; 2],
) -> (AppState, SectionId, SectionId, SectionId) {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Weld".to_string(),
        location: None,
    });
    let a = create_section(&state, "A", shots(3, 90.0, 20.0));
    let b = create_section(&state, "B", shots(2, 0.0, 10.0));
    let c = create_section(&state, "C", shots(1, 270.0, 10.0));
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
    assert!(state.integrity_check().messages.is_empty());
    (state, a, b, c)
}

fn delete_last_shot_of(state: &AppState, section: SectionId) {
    let last = project(state)
        .get_section(section)
        .unwrap()
        .shots
        .last()
        .unwrap()
        .id;
    ok(state.execute(Command::DeleteShot {
        section_id: section,
        shot_id: last,
    }));
}

#[test]
fn delete_shot_does_not_weld_two_closures_onto_one_section() {
    let (state, a, b, c) = two_closures_on_one_station([1, 3]);
    delete_last_shot_of(&state, b);
    let messages = state.integrity_check().messages;
    // Before #776's fix: ["Merged stations 2:1 and 0:3 are 40.00 m apart but
    // should be the same point"].
    assert!(messages.is_empty(), "{messages:?}");
    let after = project(&state);
    // The first closure's loop is broken, so it becomes the merge C:1 = A:1.
    // C:1 then reaches A:3 along A's own shots, so the second closure is still a
    // loop: it is re-anchored from the deleted B:2 onto C:1, not promoted.
    assert_eq!(
        merges(&after),
        vec![(StationRef::new(a, 1), StationRef::new(c, 1))]
    );
    assert_eq!(
        closures(&after),
        vec![(StationRef::new(a, 3), StationRef::new(c, 1))]
    );
}

#[test]
fn the_first_closure_in_the_list_is_the_one_promoted() {
    // Same survey, closures created the other way round: the first one decided
    // (B:2 to A:3) is promoted and the second (to A:1) re-anchored. Either way
    // one merge and one closure, and no station claimed at two places.
    let (state, a, b, c) = two_closures_on_one_station([3, 1]);
    delete_last_shot_of(&state, b);
    let messages = state.integrity_check().messages;
    assert!(messages.is_empty(), "{messages:?}");
    let after = project(&state);
    assert_eq!(
        merges(&after),
        vec![(StationRef::new(a, 3), StationRef::new(c, 1))]
    );
    assert_eq!(
        closures(&after),
        vec![(StationRef::new(a, 1), StationRef::new(c, 1))]
    );
}

#[test]
fn the_delete_dialog_promises_what_the_delete_does() {
    let (state, a, b, c) = two_closures_on_one_station([1, 3]);
    let before = project(&state);
    let last = before.get_section(b).unwrap().shots.last().unwrap().id;
    let impact = before.preview_delete_shot(b, last).unwrap();
    let promoted: Vec<_> = impact
        .closure_to_merge
        .iter()
        .map(|p| (p.station_a, p.station_b))
        .collect();
    assert_eq!(
        promoted,
        vec![(StationRef::new(c, 1), StationRef::new(a, 1))]
    );
    assert_eq!(impact.replacement_closure_updates.len(), 1, "{impact:?}");
    let update = &impact.replacement_closure_updates[0];
    assert_eq!(update.old_station, StationRef::new(b, 2));
    assert_eq!(update.new_station, StationRef::new(c, 1));

    // The delete does exactly that: the promoted pair is the one merge, and the
    // re-anchored closure keeps its id and now ends at C:1.
    ok(state.execute(Command::DeleteShot {
        section_id: b,
        shot_id: last,
    }));
    let after = project(&state);
    assert_eq!(
        merges(&after),
        vec![(StationRef::new(a, 1), StationRef::new(c, 1))]
    );
    assert_eq!(after.loop_closures.len(), 1);
    let kept = &after.loop_closures[0];
    assert_eq!(kept.id, update.closure_id);
    let ends = [
        after.resolve_key(&kept.station_a).unwrap(),
        after.resolve_key(&kept.station_b).unwrap(),
    ];
    assert!(ends.contains(&StationRef::new(c, 1)), "{ends:?}");
    assert!(ends.contains(&StationRef::new(a, 3)), "{ends:?}");
}

/// The whole project as JSON, without the edit clock, which every command
/// moves forward.
fn canonical(p: &Project) -> serde_json::Value {
    let mut value = serde_json::to_value(p).unwrap();
    if let Some(object) = value.as_object_mut() {
        object.remove("modified_at");
    }
    value
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
    assert!(state.integrity_check().messages.is_empty());
    ok(state.redo());
    assert_eq!(canonical(&project(&state)), after);
    assert!(state.integrity_check().messages.is_empty());
}

#[test]
fn one_closure_on_the_deleted_station_is_still_promoted() {
    // The control from the issue: with only the B:2 to A:1 closure, deleting B's
    // last shot breaks that loop, so the closure becomes the merge C:1 = A:1.
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Control".to_string(),
        location: None,
    });
    let a = create_section(&state, "A", shots(3, 90.0, 20.0));
    let b = create_section(&state, "B", shots(2, 0.0, 10.0));
    let c = create_section(&state, "C", shots(1, 270.0, 10.0));
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
    assert!(state.integrity_check().messages.is_empty());
    let after = project(&state);
    assert_eq!(
        merges(&after),
        vec![(StationRef::new(a, 1), StationRef::new(c, 1))]
    );
    assert!(closures(&after).is_empty());
}
