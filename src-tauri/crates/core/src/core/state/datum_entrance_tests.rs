//! The Entrance answer on a zero datum through the command layer (#741):
//! `SetZeroDatum { entrance }` stores it, `None` keeps it, undo restores it,
//! and a command persisted before it still loads. Synthetic surveys only.
//!
//! Retired: `an_undo_entry_persisted_before_the_answer_still_loads` read the
//! old `UndoData::ZeroDatumInfo`; the changeset engine records the datum list
//! itself, and histories older than the station-graph format are dropped on
//! open (plan 7.2.8).

use super::topology_kit::{empty, line_at, north, open, origin, project, run, slot, Opened};
use super::AppState;
use crate::core::Command;
use crate::models::{local_to_gps, GpsCoordinates, StationRef};

fn gps_north(north: f64) -> GpsCoordinates {
    let (lat, lon) = local_to_gps(0.0, north, &origin()).unwrap();
    GpsCoordinates::new(lat, lon, 0.0).unwrap()
}

/// One line of six 50 m shots due north, placed by a datum on station 0.
/// Station 3 is mid-passage.
fn line() -> (Opened, StationRef) {
    let mut p = empty("Line");
    let a = line_at(&mut p, "A", [0.0, 0.0], north(6, 50.0));
    let opened = open(&p);
    set(&opened.state, slot(a, 0), origin(), None);
    (opened, slot(a, 3))
}

fn set(state: &AppState, station_ref: StationRef, gps: GpsCoordinates, entrance: Option<bool>) {
    run(
        state,
        Command::SetZeroDatum {
            station_ref,
            coordinates: gps,
            accuracy_m: None,
            entrance,
        },
    );
}

/// `(stored answer, resolved entrance)` of the datum on `at`.
fn answer(state: &AppState, at: StationRef) -> (Option<bool>, bool) {
    let p = project(state);
    let station = p.topology().station_at(at).unwrap();
    let stored = p
        .datums
        .iter()
        .find(|d| d.station == station)
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
    let (opened, mid) = line();
    let state = &opened.state;
    set(state, mid, gps_north(150.0), None);
    assert_eq!(answer(state, mid), (None, false), "mid-passage default");

    set(state, mid, gps_north(150.0), Some(true));
    assert_eq!(answer(state, mid), (Some(true), true));

    // Re-saving the GPS without an answer keeps the stored one.
    set(state, mid, gps_north(152.0), None);
    assert_eq!(answer(state, mid), (Some(true), true));
}

#[test]
fn undo_restores_the_previous_answer_and_redo_reapplies_it() {
    let (opened, mid) = line();
    let state = &opened.state;
    set(state, mid, gps_north(150.0), Some(true));
    set(state, mid, gps_north(150.0), Some(false));
    assert_eq!(answer(state, mid), (Some(false), false));

    assert!(state.undo().success);
    assert_eq!(answer(state, mid), (Some(true), true));
    assert!(state.redo().success);
    assert_eq!(answer(state, mid), (Some(false), false));
}

#[test]
fn undo_of_removing_a_datum_puts_its_answer_back() {
    let (opened, mid) = line();
    let state = &opened.state;
    set(state, mid, gps_north(150.0), Some(true));
    run(state, Command::RemoveZeroDatum { station_ref: mid });
    assert!(state.undo().success);
    assert_eq!(answer(state, mid), (Some(true), true));
}

#[test]
fn an_entrance_answer_on_a_new_datum_is_one_entry() {
    let (opened, mid) = line();
    let state = &opened.state;
    let entries = state.get_undo_stack().len();
    set(state, mid, gps_north(150.0), Some(true));
    assert_eq!(state.get_undo_stack().len(), entries + 1);
    assert!(state.undo().success);
    assert!(project(state)
        .datums
        .iter()
        .all(|d| Some(d.station) != project(state).topology().station_at(mid)));
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
