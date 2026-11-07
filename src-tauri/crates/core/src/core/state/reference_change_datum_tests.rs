//! [#576 RV5-F4-2] Changing the default north reference keeps every zero datum
//! where it is.
//!
//! The coordinate walk places each section's station 0 at its stored offset and
//! walks its shots with the section's north reference, else the project default.
//! A new default therefore turns every inheriting section about its OWN station 0.
//! A datum on a later station of its section moved with it: the frame datum left
//! local (0, 0) - the georeference invariant, so the whole cave slid on the map
//! and in every export - and so did a second datum in another part of the survey.
//! The RV4-R-C1 reconcile re-aligned the merges around the moved datum, and the
//! displaced frame went into the undo entry, the saved file and the reopen.
//!
//! All data is synthetic, built in memory or in a temp dir.

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{
    AzimuthReference, CorrectionSource, GpsCoordinates, SectionId, ShotData, StationRef,
};
use crate::test_fixtures::{heading_shot, state_with_project};

fn magnetic(degrees: f64) -> AzimuthReference {
    AzimuthReference::magnetic(degrees, CorrectionSource::Manual)
}

fn create_section(state: &AppState, name: &str, shots: Vec<ShotData>) -> SectionId {
    let created = state.execute(Command::CreateSection {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#336699".to_string(),
        direction: None,
        shots,
    });
    assert!(created.success, "{:?}", created.error);
    created.section_id.unwrap()
}

fn set_datum(state: &AppState, station: StationRef, lat: f64, lon: f64) {
    let set = state.execute(Command::SetZeroDatum {
        station_ref: station,
        coordinates: GpsCoordinates::new(lat, lon, 0.0).unwrap(),
    });
    assert!(set.success, "{:?}", set.error);
}

/// SectionA (two 10 m shots north), SectionB (one 5 m shot east) merged at A2 =
/// B0, and an unmerged SectionC (two 10 m shots east). The frame datum is on A1,
/// the MIDDLE of SectionA; SectionC carries a second datum on C1. No section
/// declares a north reference of its own.
fn surveyed() -> (AppState, SectionId, SectionId, SectionId) {
    let state = state_with_project("CaveA");
    let a = create_section(
        &state,
        "SectionA",
        vec![
            heading_shot(0.0, 10.0).with_depths(0.0, 0.0),
            heading_shot(0.0, 10.0).with_depths(0.0, 0.0),
        ],
    );
    let b = create_section(
        &state,
        "SectionB",
        vec![heading_shot(90.0, 5.0).with_depths(0.0, 0.0)],
    );
    let c = create_section(
        &state,
        "SectionC",
        vec![
            heading_shot(90.0, 10.0).with_depths(0.0, 0.0),
            heading_shot(90.0, 10.0).with_depths(0.0, 0.0),
        ],
    );
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "{:?}", merged.error);
    set_datum(&state, StationRef::new(a, 1), 20.5, -87.3);
    set_datum(&state, StationRef::new(c, 1), 20.501, -87.299);
    (state, a, b, c)
}

fn position(state: &AppState, station: StationRef) -> (f64, f64) {
    let project = state.get_project_state().unwrap();
    let stations = project.compute_station_coordinates();
    let found = stations.iter().find(|s| s.station_ref == station).unwrap();
    (found.coordinates.x, found.coordinates.y)
}

fn distance(p: (f64, f64), q: (f64, f64)) -> f64 {
    (p.0 - q.0).hypot(p.1 - q.1)
}

fn change_reference(state: &AppState, reference: AzimuthReference) {
    let live = state.get_project_state().unwrap();
    let set = state.update_project_settings(live.name, live.validation, Some(reference));
    assert!(set.success, "{:?}", set.error);
}

/// Live, after undo, after redo and after a save + reopen, both datums sit where
/// they were before the change, and the junction stays merged.
#[test]
fn a_reference_change_keeps_every_datum_in_place() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CaveA.stalx");
    let (state, a, b, c) = surveyed();
    let frame = StationRef::new(a, 1);
    let second = StationRef::new(c, 1);
    let frame_before = position(&state, frame);
    let second_before = position(&state, second);
    let a0_before = position(&state, StationRef::new(a, 0));

    change_reference(&state, magnetic(30.0));
    let frame_live = position(&state, frame);
    let second_live = position(&state, second);
    let gap_live = distance(
        position(&state, StationRef::new(a, 2)),
        position(&state, StationRef::new(b, 0)),
    );
    // The change did turn SectionA (about the held datum): A0 moved.
    let turned = position(&state, StationRef::new(a, 0));

    assert!(state.undo().success, "undo the settings change");
    let frame_undone = position(&state, frame);
    assert!(state.redo().success, "redo the settings change");
    let frame_redone = position(&state, frame);
    let second_redone = position(&state, second);

    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone()),
            })
            .success
    );
    let reopened = AppState::new().unwrap();
    assert!(reopened.open_project(path).success, "reopen the saved file");
    let frame_reopened = position(&reopened, frame);
    let second_reopened = position(&reopened, second);

    println!("frame datum A1: before {frame_before:?}, after the change {frame_live:?}, after undo {frame_undone:?}, after redo {frame_redone:?}, after reopen {frame_reopened:?}");
    println!("second datum C1: before {second_before:?}, after the change {second_live:?}, after redo {second_redone:?}, after reopen {second_reopened:?}");
    println!("A0: before {a0_before:?}, after the change {turned:?}; junction gap after the change {gap_live:.3} m");

    assert!(
        distance(frame_before, (0.0, 0.0)) < 1e-6,
        "the frame datum starts on the origin"
    );
    for (what, at) in [
        ("after the change", frame_live),
        ("after undo", frame_undone),
        ("after redo", frame_redone),
        ("after reopen", frame_reopened),
    ] {
        assert!(
            distance(at, frame_before) < 1e-6,
            "the frame datum moved {what}: {frame_before:?} -> {at:?}"
        );
    }
    for (what, at) in [
        ("after the change", second_live),
        ("after redo", second_redone),
        ("after reopen", second_reopened),
    ] {
        assert!(
            distance(at, second_before) < 1e-6,
            "the second datum moved {what}: {second_before:?} -> {at:?}"
        );
    }
    assert!(gap_live < 1e-6, "the junction came apart: {gap_live} m");
    assert!(
        distance(turned, a0_before) > 1.0,
        "the reference change turned SectionA: A0 {a0_before:?} -> {turned:?}"
    );
}
