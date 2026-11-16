//! What the read-only queries tell the UI about GPS fixes (#741), through the
//! command layer: the merge check's datum shift (the Merge dialog) and the
//! adjustment report's far fixes (the notice above the map). Synthetic
//! surveys only.

use super::AppState;
use crate::core::Command;
use crate::models::{
    local_to_gps, GpsCoordinates, SectionId, ShotData, StationRef, SurveyDirection,
};

const ORIGIN: (f64, f64) = (20.5, -87.25);

fn origin() -> GpsCoordinates {
    GpsCoordinates::new(ORIGIN.0, ORIGIN.1, 0.0).unwrap()
}

fn gps_at(east: f64, north: f64) -> GpsCoordinates {
    let (lat, lon) = local_to_gps(east, north, &origin()).unwrap();
    GpsCoordinates::new(lat, lon, 0.0).unwrap()
}

fn section(state: &AppState, name: &str, count: usize) -> SectionId {
    let shots = (0..count)
        .map(|_| ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 50.0,
            ..Default::default()
        })
        .collect();
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

fn set_datum(state: &AppState, station_ref: StationRef, gps: GpsCoordinates) {
    let r = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps,
        accuracy_m: None,
    });
    assert!(r.success, "{:?}", r.error);
}

fn new_project(name: &str) -> AppState {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: name.to_string(),
        location: None,
    });
    state
}

#[test]
fn the_merge_check_says_which_placed_piece_moves_and_by_how_much() {
    let state = new_project("TwoPieces");
    let a = section(&state, "A", 2);
    let b = section(&state, "B", 2);
    let (end_a, start_b) = (StationRef::new(a, 2), StationRef::new(b, 0));

    // One placed piece: a merge moves the unplaced one, which says nothing.
    set_datum(&state, StationRef::new(a, 0), origin());
    let one = state.check_merge_connectivity(end_a, start_b).unwrap();
    assert!(one.can_merge);
    assert_eq!(one.datum_shift, None);

    // B placed by its own GPS, 30 m east of A:2.
    set_datum(&state, start_b, gps_at(30.0, 100.0));
    let two = state.check_merge_connectivity(start_b, end_a).unwrap();
    assert!(two.can_merge);
    let shift = two.datum_shift.expect("two placed pieces");
    assert_eq!(shift.moving_section_ids, vec![b]);
    assert_eq!(shift.datum_station, start_b);
    assert!(
        (shift.shift_m - 30.0).abs() < 0.01,
        "shift {}",
        shift.shift_m
    );
}

#[test]
fn the_adjustment_report_names_a_fix_that_fails_the_gap_check() {
    let state = new_project("Traverse");
    let a = section(&state, "A", 5);
    let b = section(&state, "B", 5);
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 5),
        station_b: StationRef::new(b, 0),
    });
    assert!(r.success, "{:?}", r.error);
    set_datum(&state, StationRef::new(a, 0), origin());
    assert!(state
        .get_adjustment_report()
        .unwrap()
        .far_gps_fixes
        .is_empty());

    // 40 m off over 500 m of survey: more than the 25 m threshold.
    let fix = StationRef::new(b, 5);
    set_datum(&state, fix, gps_at(40.0, 500.0));
    let far = state.get_adjustment_report().unwrap().far_gps_fixes;
    assert_eq!(far.len(), 1, "{far:?}");
    assert_eq!(far[0].station, fix);
    assert!((far[0].gap_m - 40.0).abs() < 0.01, "gap {}", far[0].gap_m);
    assert!(far[0].gap_m > far[0].threshold_m);
}
