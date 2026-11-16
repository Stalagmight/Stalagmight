//! The Entrance answer on a zero datum through the command layer (#741):
//! `SetZeroDatum { entrance }` stores it, `None` keeps it, undo restores it,
//! and history persisted before it still loads. Synthetic surveys only.

use super::AppState;
use crate::core::{Command, UndoData};
use crate::models::{local_to_gps, GpsCoordinates, Project, ShotData, StationRef, SurveyDirection};

fn origin() -> GpsCoordinates {
    GpsCoordinates::new(20.5, -87.25, 0.0).unwrap()
}

fn gps_north(north: f64) -> GpsCoordinates {
    let (lat, lon) = local_to_gps(0.0, north, &origin()).unwrap();
    GpsCoordinates::new(lat, lon, 0.0).unwrap()
}

/// One section of six 50 m shots due north, placed by a datum on station 0.
/// Station 3 is mid-passage.
fn line() -> (AppState, StationRef) {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Line".to_string(),
        location: None,
    });
    let shots = (0..6)
        .map(|_| ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 50.0,
            ..Default::default()
        })
        .collect();
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#123456".to_string(),
            direction: Some(SurveyDirection::In),
            shots,
        })
        .section_id
        .unwrap();
    set(&state, StationRef::new(a, 0), origin(), None);
    (state, StationRef::new(a, 3))
}

fn set(state: &AppState, station_ref: StationRef, gps: GpsCoordinates, entrance: Option<bool>) {
    let r = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps,
        accuracy_m: None,
        entrance,
    });
    assert!(r.success, "{:?}", r.error);
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

/// `(stored answer, resolved entrance)` of the datum on `at`.
fn answer(state: &AppState, at: StationRef) -> (Option<bool>, bool) {
    let p = project(state);
    let stored = p
        .zero_datums
        .iter()
        .find(|zd| p.resolve_key(&zd.station_ref) == Some(at))
        .unwrap()
        .entrance;
    let wire = p
        .zero_datums_wire()
        .into_iter()
        .find(|w| w.station_ref == at)
        .unwrap();
    (stored, wire.is_entrance)
}

#[test]
fn the_command_stores_the_answer_and_none_keeps_it() {
    let (state, mid) = line();
    set(&state, mid, gps_north(150.0), None);
    assert_eq!(answer(&state, mid), (None, false), "mid-passage default");

    set(&state, mid, gps_north(150.0), Some(true));
    assert_eq!(answer(&state, mid), (Some(true), true));

    // Re-saving the GPS without an answer keeps the stored one.
    set(&state, mid, gps_north(152.0), None);
    assert_eq!(answer(&state, mid), (Some(true), true));
}

#[test]
fn undo_restores_the_previous_answer_and_redo_reapplies_it() {
    let (state, mid) = line();
    set(&state, mid, gps_north(150.0), Some(true));
    set(&state, mid, gps_north(150.0), Some(false));
    assert_eq!(answer(&state, mid), (Some(false), false));

    assert!(state.undo().success);
    assert_eq!(answer(&state, mid), (Some(true), true));
    assert!(state.redo().success);
    assert_eq!(answer(&state, mid), (Some(false), false));
}

#[test]
fn undo_of_removing_a_datum_puts_its_answer_back() {
    let (state, mid) = line();
    set(&state, mid, gps_north(150.0), Some(true));
    let r = state.execute(Command::RemoveZeroDatum { station_ref: mid });
    assert!(r.success, "{:?}", r.error);
    assert!(state.undo().success);
    assert_eq!(answer(&state, mid), (Some(true), true));
}

#[test]
fn a_set_zero_datum_command_persisted_before_the_answer_still_loads() {
    let cmd = Command::SetZeroDatum {
        station_ref: StationRef::new(3, 2),
        coordinates: origin(),
        accuracy_m: None,
        entrance: Some(false),
    };
    let mut json = serde_json::to_value(&cmd).unwrap();
    assert_eq!(json["entrance"], serde_json::Value::Bool(false));
    json.as_object_mut().unwrap().remove("entrance");
    let legacy: Command = serde_json::from_value(json).unwrap();
    assert!(matches!(
        legacy,
        Command::SetZeroDatum { entrance: None, .. }
    ));
}

#[test]
fn an_undo_entry_persisted_before_the_answer_still_loads() {
    let (state, mid) = line();
    set(&state, mid, gps_north(150.0), Some(true));
    let entry = state.undo_stack.lock().unwrap().pop_undo().unwrap();
    let mut json = serde_json::to_value(&entry.undo_data).unwrap();
    let fields = json
        .as_object_mut()
        .and_then(|o| o.values_mut().next())
        .and_then(serde_json::Value::as_object_mut)
        .unwrap();
    assert!(fields.remove("previous_entrance").is_some());
    let legacy: UndoData = serde_json::from_value(json).unwrap();
    assert!(matches!(
        legacy,
        UndoData::ZeroDatumInfo {
            previous_entrance: None,
            ..
        }
    ));
}
