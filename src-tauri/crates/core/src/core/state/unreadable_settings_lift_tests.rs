//! [#576 B15 follow-up, decision B15-EDIT] A stored project setting the open
//! could not read stops blocking saves once the user sets it again.
//!
//! B15 made every save refuse while a stored setting is unread, so the default
//! standing in for it can never be written over the stored value. Once the user
//! sets that setting themselves in Project Settings, the value in memory is
//! theirs, not a stand-in, and writing it is exactly what they asked for: that
//! key leaves the unreadable list, and when the list is empty saves work again.
//! The default north reference is set with the thresholds, by the Project Settings
//! save (`update_project_settings`, #576 NF-W3-2), so all three can be lifted
//! without reopening the project.
//!
//! All data is synthetic: a project this build saves, then its settings table
//! broken with SQL so the open cannot read it.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::models::{
    AzimuthReference, CorrectionSource, NorthReference, Project, SectionData, ShotData, StationRef,
};
use crate::persistence::Database;

/// A project whose settings table is broken (its `value` column renamed), so the
/// open can read none of `validation`, `surface_elevation` and
/// `default_azimuth_reference`.
fn file_with_unreadable_settings(dir: &Path) -> PathBuf {
    let path = dir.join("CaveB.stalx");
    let mut project = Project::new("CaveB".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionB".to_string()),
        color: "#654321".to_string(),
        shots: vec![ShotData {
            length: 4.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    project.validation.warn_shot_length_max = 77.0;
    project.surface_elevation = Some(123.0);
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("ALTER TABLE project_settings RENAME COLUMN value TO broken_value;")
        .unwrap();
    path
}

/// Setting the validation thresholds, the default north reference and the entrance
/// elevation again in Project Settings takes each off the unreadable list - also
/// when the value set equals the default in memory (the dialog sends it anyway
/// while it is unread). Until the last one is set, the save is refused and names
/// only what is still unread; then it goes through.
#[test]
fn setting_an_unread_setting_again_takes_it_off_the_list() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let path = file_with_unreadable_settings(dir.path());
    let state = AppState::new().unwrap();
    assert!(state.open_project(path).success);
    println!(
        "unreadable after open: {:?}",
        state.unreadable_project_settings()
    );

    let live = state.get_project_state().unwrap();
    let validation = state.update_project_settings(
        live.name.clone(),
        live.validation,
        live.default_azimuth_reference,
    );
    assert!(validation.success, "{:?}", validation.error);
    println!(
        "after Project Settings sets the thresholds: {:?}",
        state.unreadable_project_settings()
    );
    assert_eq!(
        state.unreadable_project_settings(),
        vec!["surface_elevation".to_string()],
        "the thresholds and the north reference the user set are theirs to save"
    );

    let refused = state.save_project(None);
    println!(
        "save: success={} error={:?}",
        refused.success, refused.error
    );
    assert!(!refused.success, "the elevation is still unread");
    let error = refused.error.unwrap_or_default();
    assert!(
        error.contains("stored entrance elevation could not be read"),
        "the refusal names only what is still unread: {error}"
    );
    assert!(
        error.contains("set it in Project Settings") && !error.contains("reopen"),
        "and points at Project Settings, not at a reopen: {error}"
    );

    let elevation = state.set_surface_elevation(Some(98.5));
    assert!(elevation.success, "{:?}", elevation.error);
    let left = state.unreadable_project_settings();
    println!("after Project Settings sets the elevation: {left:?}");
    assert!(left.is_empty());
    let saved = state.save_project(None);
    println!("save: success={} error={:?}", saved.success, saved.error);
    assert!(saved.success, "{:?}", saved.error);
}

/// 10 degrees: big enough that a dropped or doubled correction cannot pass as noise.
fn declination(degrees: f64) -> AzimuthReference {
    AzimuthReference::magnetic(degrees, CorrectionSource::Manual)
}

/// A project with one section that inherits the default and one that declares its own.
fn two_section_project(dir: &Path) -> PathBuf {
    let path = dir.join("CaveD.stalx");
    let mut project = Project::new("CaveD".to_string(), None);
    for (name, own) in [("Inherits", None), ("Declares", Some(declination(-5.0)))] {
        let id = project
            .add_section(SectionData {
                name: Some(name.to_string()),
                color: "#123456".to_string(),
                shots: vec![ShotData {
                    azimuth_in: 0.0,
                    azimuth_out: 0.0,
                    length: 10.0,
                    ..Default::default()
                }],
                ..Default::default()
            })
            .id;
        project.get_section_mut(id).unwrap().azimuth_reference = own;
    }
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);
    path
}

/// The section's one shot as a plan vector: its end station minus its start.
fn end_of(state: &AppState, section_index: usize) -> (f64, f64) {
    let project = state.get_project_state().unwrap();
    let id = project.sections[section_index].id;
    let at = |index| {
        project
            .compute_station_raw_coords(StationRef::new(id, index))
            .unwrap()
    };
    let (start, end) = (at(0), at(1));
    (end.x - start.x, end.y - start.y)
}

/// [#576 NF-W3-2] The Project Settings save sets the default north reference: the
/// section that declares none turns with it, the one with its own does not, the
/// value is written to the file, and it is one undo step like the rest of the save.
/// While it was unread, setting it lifts the refusal.
#[test]
fn the_project_settings_save_sets_the_default_north_reference() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let path = two_section_project(dir.path());
    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success);
    // What a load that could not read the reference commits.
    state.set_unreadable_project_settings(vec!["default_azimuth_reference".to_string()]);
    let inherits_before = end_of(&state, 0);
    let declares_before = end_of(&state, 1);

    let live = state.get_project_state().unwrap();
    let set = state.update_project_settings(live.name, live.validation, Some(declination(10.0)));
    assert!(set.success, "{:?}", set.error);
    let inherits_after = end_of(&state, 0);
    println!(
        "inheriting section end: {inherits_before:?} -> {inherits_after:?}; declaring: {declares_before:?} -> {:?}",
        end_of(&state, 1)
    );
    let turned = inherits_after.0.atan2(inherits_after.1).to_degrees();
    assert!((turned - 10.0).abs() < 1e-9, "turned by {turned}, not 10");
    assert_eq!(
        end_of(&state, 1),
        declares_before,
        "a section's own reference wins"
    );
    assert!(
        state.unreadable_project_settings().is_empty(),
        "set again: lifted"
    );

    let saved = state.save_project(None);
    assert!(saved.success, "{:?}", saved.error);
    let stored = Database::open_existing(&path)
        .unwrap()
        .load_project_default_azimuth_reference()
        .unwrap();
    assert_eq!(stored, Some(declination(10.0)));

    assert!(state.undo().success);
    assert_eq!(
        state.get_project_state().unwrap().default_azimuth_reference,
        None
    );
    assert_eq!(end_of(&state, 0), inherits_before, "undo turns it back");
}

/// What arrives off the wire is re-sanitised like every ingress: a "true north"
/// reference carries no correction, and nothing non-finite reaches geometry.
#[test]
fn the_default_north_reference_is_sanitised_on_the_way_in() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    assert!(state.open_project(two_section_project(dir.path())).success);
    let live = state.get_project_state().unwrap();
    let asserted = AzimuthReference {
        north: NorthReference::True,
        correction: 5.0,
        source: CorrectionSource::Manual,
    };
    assert!(
        state
            .update_project_settings(live.name, live.validation, Some(asserted))
            .success
    );
    let stored = state
        .get_project_state()
        .unwrap()
        .default_azimuth_reference
        .unwrap();
    assert_eq!(stored.north, NorthReference::True);
    assert_eq!(stored.correction, 0.0);
}

/// When every unread setting has been set again, saving works again: the values
/// the user set are what the file now gets.
#[test]
fn once_nothing_is_unread_the_save_goes_through() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CaveC.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("CaveC".to_string(), None))
        .unwrap();
    drop(db);
    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success);
    // What a load that could not read the elevation commits.
    state.set_unreadable_project_settings(vec!["surface_elevation".to_string()]);
    assert!(
        !state.save_project(None).success,
        "premise: refused while unread"
    );

    assert!(state.set_surface_elevation(Some(12.0)).success);
    let saved = state.save_project(None);
    println!("save: success={} error={:?}", saved.success, saved.error);
    assert!(saved.success, "{:?}", saved.error);
    let stored = Database::open_existing(&path)
        .unwrap()
        .load_project_surface_elevation()
        .unwrap();
    assert_eq!(stored, Some(12.0));
}
