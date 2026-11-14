//! An undo after a save and a reopen lands on exactly the state it recorded
//! when the undone command moved shots from one section to another (#732).
//!
//! The reopen translates the persisted history through the load's shot UUID
//! heal (`undo/uuid_remap.rs`). A shot that a combine or a split moved is held
//! by one section in an older entry (the one that created or added it) and by
//! another live: that is a move, not a repeat, and must keep its uuid. Each
//! test records the project before the move, moves shots, saves, reopens,
//! undoes, and compares the whole project, shot uuids and station keys
//! included; then redoes and compares with the moved state.

use std::path::{Path, PathBuf};

use super::AppState;
use crate::core::Command;
use crate::models::{
    CombinePick, CombineSectionsOptions, SectionId, ShotData, SplitAt, SplitSectionOptions,
    SplitSide, StationRef,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: 10.0,
        depth_out: 11.0,
        ..Default::default()
    }
}

fn create(state: &AppState, name: &str, shots: Vec<ShotData>) -> SectionId {
    let created = state.execute(Command::CreateSection {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots,
    });
    assert!(created.success, "{:?}", created.error);
    created.section_id.unwrap()
}

fn new_project(name: &str, path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: name.to_string(),
                location: None,
            })
            .success
    );
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.to_path_buf())
            })
            .success
    );
    state
}

fn save_close_reopen(state: &AppState, path: &Path) {
    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path.to_path_buf()).success, "reopen");
}

/// The whole project as JSON, without the edit timestamp an undo or a redo
/// stamps afresh.
fn snapshot(state: &AppState) -> serde_json::Value {
    let project = state.get_project_state().unwrap();
    let json = AppState::serialize_project_snapshot(&project).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    if let Some(object) = value.as_object_mut() {
        object.remove("modified_at");
    }
    value
}

fn shot_uuids(state: &AppState) -> Vec<(SectionId, Vec<String>)> {
    state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .map(|s| {
            (
                s.id,
                s.shots
                    .iter()
                    .map(|shot| shot.uuid.clone().unwrap())
                    .collect(),
            )
        })
        .collect()
}

/// Run `moving` (which moves shots between sections), save, reopen, and check
/// that undo and redo land exactly on the recorded states.
fn assert_exact_undo_and_redo_after_reopen(
    state: &AppState,
    path: &Path,
    what: &str,
    moving: impl FnOnce(&AppState),
) {
    // `before` is read after a reopen (the caller saved and reopened), so it
    // carries what a load settles (the shot-id counter, the captured gauge
    // pair); the undo restores the snapshot the move recorded from it.
    let before = snapshot(state);
    let before_uuids = shot_uuids(state);
    moving(state);
    let after_uuids = shot_uuids(state);

    save_close_reopen(state, path);
    assert_eq!(
        shot_uuids(state),
        after_uuids,
        "{what}: the reopen changed a shot uuid"
    );
    let after = snapshot(state);

    assert!(state.undo().success, "{what}: undo after the reopen");
    assert_eq!(
        shot_uuids(state),
        before_uuids,
        "{what}: undo after a reopen gave moved shots new uuids"
    );
    assert_eq!(snapshot(state), before, "{what}: undo after a reopen");

    assert!(state.redo().success, "{what}: redo after the reopen");
    assert_eq!(snapshot(state), after, "{what}: redo after a reopen");
}

fn split_options() -> SplitSectionOptions {
    SplitSectionOptions {
        name_a: "Main-A".to_string(),
        name_b: "Main-B".to_string(),
        color_a: "#111111".to_string(),
        color_b: "#222222".to_string(),
        comment_side: SplitSide::A,
        datum_side: SplitSide::A,
    }
}

fn temp_path(dir: &tempfile::TempDir, file: &str) -> PathBuf {
    dir.path().join(file)
}

#[test]
fn a_combine_undone_after_a_reopen_keeps_every_shot_uuid() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = temp_path(&dir, "combine.stalx");
    let state = new_project("Combine", &path);
    let k = create(&state, "K", vec![shot(90.0, 10.0), shot(90.0, 5.0)]);
    let o = create(&state, "O", vec![shot(0.0, 5.0), shot(0.0, 4.0)]);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(k, 2),
        station_b: StationRef::new(o, 0),
    });
    assert!(merged.success, "{:?}", merged.error);
    save_close_reopen(&state, &path);

    assert_exact_undo_and_redo_after_reopen(&state, &path, "combine", |state| {
        let options = CombineSectionsOptions {
            name: "Joined".to_string(),
            color: "#abcdef".to_string(),
            swap_lr: false,
            comment_pick: CombinePick::Kept,
            name_pick: CombinePick::Kept,
            join: None,
        };
        let result = state.execute_combine_sections(k, o, options);
        assert!(result.success, "{:?}", result.error);
    });
}

#[test]
fn a_combine_across_a_closure_undone_after_a_reopen_keeps_every_shot_uuid() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = temp_path(&dir, "closure.stalx");
    let state = new_project("Loop", &path);
    let k = create(&state, "K", vec![shot(90.0, 10.0)]);
    let o = create(&state, "O", vec![shot(270.0, 9.6)]);
    for command in [
        Command::MergeStations {
            station_a: StationRef::new(k, 0),
            station_b: StationRef::new(o, 1),
        },
        Command::CreateLoopClosure {
            station_a: StationRef::new(k, 1),
            station_b: StationRef::new(o, 0),
            closure_strategy: crate::models::ClosureStrategy::Batch,
        },
    ] {
        let result = state.execute(command);
        assert!(result.success, "{:?}", result.error);
    }
    save_close_reopen(&state, &path);

    assert_exact_undo_and_redo_after_reopen(&state, &path, "closure combine", |state| {
        let options = CombineSectionsOptions {
            name: "Joined".to_string(),
            color: "#abcdef".to_string(),
            swap_lr: false,
            comment_pick: CombinePick::Kept,
            name_pick: CombinePick::Kept,
            join: Some(crate::models::CombineJoinEnds {
                kept: crate::models::SectionEnd::End,
                other: crate::models::SectionEnd::Start,
            }),
        };
        let result = state.execute_combine_sections(k, o, options);
        assert!(result.success, "{:?}", result.error);
    });
}

#[test]
fn a_split_undone_after_a_reopen_keeps_every_shot_uuid() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = temp_path(&dir, "split.stalx");
    let state = new_project("Split", &path);
    let main = create(
        &state,
        "Main",
        vec![shot(45.0, 5.0), shot(45.0, 7.0), shot(45.0, 3.0)],
    );
    save_close_reopen(&state, &path);

    assert_exact_undo_and_redo_after_reopen(&state, &path, "split", |state| {
        let result = state.execute_split_section(
            main,
            SplitAt::Station { station_index: 1 },
            split_options(),
        );
        assert!(result.success, "{:?}", result.error);
    });
}

#[test]
fn a_delete_and_split_undone_after_a_reopen_keeps_every_shot_uuid() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = temp_path(&dir, "delete-split.stalx");
    let state = new_project("Delete split", &path);
    let main = create(
        &state,
        "Main",
        vec![
            shot(45.0, 5.0),
            shot(45.0, 7.0),
            shot(45.0, 3.0),
            shot(45.0, 4.0),
        ],
    );
    save_close_reopen(&state, &path);
    let middle = state
        .get_project_state()
        .unwrap()
        .get_section(main)
        .unwrap()
        .shots[1]
        .id;

    assert_exact_undo_and_redo_after_reopen(&state, &path, "delete and split", |state| {
        let result =
            state.execute_delete_shot_and_split(main, middle, "A".to_string(), "B".to_string());
        assert!(result.success, "{:?}", result.error);
    });
}

/// The section a split moves shots out of came in through Add .stalx, whose
/// undo entry reinstalls it whole.
#[test]
fn a_split_of_an_added_section_undone_after_a_reopen_keeps_every_shot_uuid() {
    let dir = tempfile::TempDir::new().unwrap();
    let source_path = temp_path(&dir, "source.stalx");
    let source = new_project("Source", &source_path);
    create(
        &source,
        "Added",
        vec![shot(10.0, 5.0), shot(10.0, 7.0), shot(10.0, 3.0)],
    );
    assert!(source.execute(Command::SaveProject { path: None }).success);
    assert!(source.execute(Command::CloseProject).success);

    let path = temp_path(&dir, "target.stalx");
    let state = new_project("Target", &path);
    create(&state, "Own", vec![shot(200.0, 6.0)]);
    let added = state.execute(Command::add_project_file(source_path));
    assert!(added.success, "{:?}", added.error);
    let section = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .find(|s| s.name.as_deref() == Some("Added"))
        .unwrap()
        .id;
    save_close_reopen(&state, &path);

    assert_exact_undo_and_redo_after_reopen(&state, &path, "split of an added section", |state| {
        let result = state.execute_split_section(
            section,
            SplitAt::Station { station_index: 1 },
            split_options(),
        );
        assert!(result.success, "{:?}", result.error);
    });
}
