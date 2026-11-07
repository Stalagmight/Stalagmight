//! [#576 RV4-R-C1] Changing the default north reference keeps merged sections
//! together.
//!
//! The coordinate walk places each section's station 0 at its stored section
//! offset and walks its shots with the section's own north reference, else the
//! project default. A new default therefore turns every inheriting section about
//! its OWN origin; unless the stored offsets are re-derived (the reconcile), two
//! sections merged at a junction come apart - in the live project, in the saved
//! file and in the undo entry's after-snapshot.
//!
//! All data is synthetic, built in memory or in a temp dir.

use tempfile::TempDir;

use super::command_epilogue::{EpilogueSteps, EPILOGUE_RUNS};
use super::AppState;
use crate::core::Command;
use crate::models::{AzimuthReference, CorrectionSource, SectionId, ShotData, StationRef};
use crate::test_fixtures::{heading_shot, state_with_project};

fn magnetic(degrees: f64) -> AzimuthReference {
    AzimuthReference::magnetic(degrees, CorrectionSource::Manual)
}

/// A project with SectionA (two 10 m shots north) and SectionB (one 5 m shot
/// east) whose station 0 is merged onto SectionA's last station. Neither section
/// declares a north reference, so both turn with the default.
fn merged_pair() -> (AppState, SectionId, SectionId) {
    let state = state_with_project("CaveA");
    let section = |name: &str, shots: Vec<ShotData>| {
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
    };
    let a = section(
        "SectionA",
        vec![
            heading_shot(0.0, 10.0).with_depths(0.0, 0.0),
            heading_shot(0.0, 10.0).with_depths(0.0, 0.0),
        ],
    );
    let b = section(
        "SectionB",
        vec![heading_shot(90.0, 5.0).with_depths(0.0, 0.0)],
    );
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "{:?}", merged.error);
    (state, a, b)
}

/// Plan distance between the two merged stations, as the coordinate walk draws them.
fn junction_gap(state: &AppState, a: SectionId, b: SectionId) -> f64 {
    let project = state.get_project_state().unwrap();
    let stations = project.compute_station_coordinates();
    let at = |r: StationRef| {
        stations
            .iter()
            .find(|s| s.station_ref == r)
            .map(|s| (s.coordinates.x, s.coordinates.y))
            .unwrap()
    };
    let (ax, ay) = at(StationRef::new(a, 2));
    let (bx, by) = at(StationRef::new(b, 0));
    (ax - bx).hypot(ay - by)
}

fn reference_of(state: &AppState) -> Option<AzimuthReference> {
    state.get_project_state()?.default_azimuth_reference
}

fn change_reference(state: &AppState, reference: AzimuthReference) {
    let live = state.get_project_state().unwrap();
    let set = state.update_project_settings(live.name, live.validation, Some(reference));
    assert!(set.success, "{:?}", set.error);
}

/// The junction stays coincident after the change, in the saved file and after a
/// reopen, and the new reference is what is stored.
#[test]
fn merged_sections_stay_together_after_a_reference_change_and_a_reopen() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CaveA.stalx");
    let (state, a, b) = merged_pair();
    let gap_before = junction_gap(&state, a, b);
    change_reference(&state, magnetic(30.0));
    let gap_live = junction_gap(&state, a, b);
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone()),
            })
            .success
    );
    let reopened = AppState::new().unwrap();
    assert!(reopened.open_project(path).success, "reopen the saved file");
    let gap_reopened = junction_gap(&reopened, a, b);
    let reference_reopened = reference_of(&reopened);
    println!("junction gap before the change: {gap_before:.3} m");
    println!("junction gap after the change (live): {gap_live:.3} m");
    println!("junction gap after save + reopen: {gap_reopened:.3} m");
    println!("reference after reopen: {reference_reopened:?}");
    assert!(gap_before < 1e-6, "the fixture starts merged");
    assert!(
        gap_live < 1e-6,
        "a reference change split the merged stations by {gap_live} m"
    );
    assert!(
        gap_reopened < 1e-6,
        "the saved file holds split offsets: {gap_reopened} m apart after reopen"
    );
    assert_eq!(reference_reopened, Some(magnetic(30.0)));
}

/// Undo restores the old reference AND the old geometry; redo restores the new
/// reference with the junction still coincident (the undo entry's after-snapshot
/// holds the re-aligned offsets).
#[test]
fn undo_and_redo_of_a_reference_change_keep_the_junction() {
    let _cfg = crate::test_helpers::isolated_config();
    let (state, a, b) = merged_pair();
    change_reference(&state, magnetic(30.0));
    let gap_changed = junction_gap(&state, a, b);
    assert!(state.undo().success, "undo the settings change");
    let reference_undone = reference_of(&state);
    let gap_undone = junction_gap(&state, a, b);
    assert!(state.redo().success, "redo the settings change");
    let reference_redone = reference_of(&state);
    let gap_redone = junction_gap(&state, a, b);
    println!("after the change: gap {gap_changed:.3} m");
    println!("after undo: reference {reference_undone:?}, gap {gap_undone:.3} m");
    println!("after redo: reference {reference_redone:?}, gap {gap_redone:.3} m");
    assert!(gap_changed < 1e-6, "split by the change: {gap_changed} m");
    assert_eq!(reference_undone, None, "undo restores the old reference");
    assert!(gap_undone < 1e-6, "split after undo: {gap_undone} m");
    assert_eq!(reference_redone, Some(magnetic(30.0)));
    assert!(gap_redone < 1e-6, "split after redo: {gap_redone} m");
}

/// The reference change settles geometry through the shared forward epilogue; a
/// rename alone still needs no derived-state step.
#[test]
fn a_reference_change_runs_the_geometry_steps_and_a_rename_none() {
    let _cfg = crate::test_helpers::isolated_config();
    let (state, _, _) = merged_pair();
    let take = || EPILOGUE_RUNS.with(|runs| std::mem::take(&mut *runs.borrow_mut()));
    let _ = take();
    change_reference(&state, magnetic(12.0));
    let reference_runs = take();
    let live = state.get_project_state().unwrap();
    let renamed = state.update_project_settings(
        "CaveRenamed".to_string(),
        live.validation,
        live.default_azimuth_reference,
    );
    assert!(renamed.success, "{:?}", renamed.error);
    let rename_runs = take();
    println!("reference change epilogue runs: {reference_runs:?}");
    println!("rename epilogue runs: {rename_runs:?}");
    assert_eq!(reference_runs, vec![EpilogueSteps::GEOMETRY]);
    assert_eq!(rename_runs, vec![EpilogueSteps::NONE]);
}
