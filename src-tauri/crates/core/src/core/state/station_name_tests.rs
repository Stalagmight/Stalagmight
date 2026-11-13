//! `Command::SetStationName` at the state layer, alone and batched with the
//! other metadata edits the Data workspace commits as ONE undo step.

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{
    Project, SectionData, SectionId, ShotData, ShotId, ShotUpdates, StationRef,
    MAX_STATION_NAME_CHARS,
};
use crate::persistence::Database;

fn shot(length: f64) -> ShotData {
    ShotData {
        azimuth_in: 45.0,
        azimuth_out: 225.0,
        length,
        depth_in: 10.0,
        depth_out: 11.0,
        ..Default::default()
    }
}

/// A project with one two-shot section (stations 0..=2).
fn fixture() -> (AppState, SectionId) {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Names".to_string(),
        location: None,
    });
    let section = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0), shot(6.0)],
        })
        .section_id
        .unwrap();
    (state, section)
}

fn name_of(state: &AppState, station: StationRef) -> Option<String> {
    state
        .get_project_state()
        .unwrap()
        .station_name_for_ref(station)
        .cloned()
}

fn comment_of(state: &AppState, station: StationRef) -> Option<String> {
    state
        .get_project_state()
        .unwrap()
        .station_comment_for_ref(station)
        .cloned()
}

fn set_name(state: &AppState, station: StationRef, name: Option<&str>) -> bool {
    state
        .execute(Command::SetStationName {
            station_ref: station,
            name: name.map(str::to_string),
        })
        .success
}

fn undo_len(state: &AppState) -> usize {
    state.undo_stack.lock().unwrap().len()
}

fn first_shot(state: &AppState, section: SectionId) -> (ShotId, Option<f64>) {
    let project = state.get_project_state().unwrap();
    let shot = &project.get_section(section).unwrap().shots[0];
    (shot.id, shot.left)
}

#[test]
fn set_update_and_remove_a_station_name() {
    let (state, section) = fixture();
    let station = StationRef::new(section, 1);

    assert!(set_name(&state, station, Some("A1")));
    assert_eq!(name_of(&state, station).as_deref(), Some("A1"));

    assert!(set_name(&state, station, Some("A1 bis")));
    assert_eq!(name_of(&state, station).as_deref(), Some("A1 bis"));

    // Station 0 is keyed by the section start, not by a shot.
    assert!(set_name(
        &state,
        StationRef::new(section, 0),
        Some("Entrance")
    ));
    assert_eq!(
        name_of(&state, StationRef::new(section, 0)).as_deref(),
        Some("Entrance")
    );

    for removal in [None, Some(""), Some("   \t ")] {
        assert!(set_name(&state, station, Some("temp")));
        assert!(set_name(&state, station, removal));
        assert_eq!(name_of(&state, station), None, "{removal:?} removes it");
    }
    // Removing a name that is not there is a successful no-op.
    assert!(set_name(&state, station, None));
}

#[test]
fn a_stored_name_is_trimmed_cleaned_and_clamped() {
    let (state, section) = fixture();
    let station = StationRef::new(section, 2);

    assert!(set_name(&state, station, Some("  Sump\u{202e} 2\n")));
    assert_eq!(name_of(&state, station).as_deref(), Some("Sump 2"));

    let long = "n".repeat(MAX_STATION_NAME_CHARS * 2);
    assert!(set_name(&state, station, Some(&long)));
    assert_eq!(
        name_of(&state, station).map(|n| n.chars().count()),
        Some(MAX_STATION_NAME_CHARS)
    );

    // Only invisible characters: nothing is left, so it removes the name.
    assert!(set_name(&state, station, Some("\u{200f}\u{2066}")));
    assert_eq!(name_of(&state, station), None);
}

#[test]
fn an_invalid_station_fails_and_records_nothing() {
    let (state, section) = fixture();
    let before = undo_len(&state);

    // Two shots: stations 0..=2, so 3 is one past the end.
    assert!(!set_name(&state, StationRef::new(section, 3), Some("oob")));
    assert!(!set_name(&state, StationRef::new(9999, 0), Some("ghost")));

    assert_eq!(
        undo_len(&state),
        before,
        "a failed edit pushes no undo entry"
    );
    assert!(state.get_project_state().unwrap().station_names.is_empty());
}

#[test]
fn undo_and_redo_a_single_name_edit() {
    let (state, section) = fixture();
    let station = StationRef::new(section, 1);

    assert!(set_name(&state, station, Some("first")));
    assert!(set_name(&state, station, Some("second")));

    assert!(state.undo().success);
    assert_eq!(name_of(&state, station).as_deref(), Some("first"));
    assert!(state.undo().success);
    assert_eq!(name_of(&state, station), None);

    assert!(state.redo().success);
    assert_eq!(name_of(&state, station).as_deref(), Some("first"));
    assert!(state.redo().success);
    assert_eq!(name_of(&state, station).as_deref(), Some("second"));
}

/// An inverse restores rather than re-derives: a name a file stored untrimmed,
/// or with a character the edit path never admits, comes back byte for byte.
#[test]
fn undo_restores_a_non_canonical_previous_name_verbatim() {
    let (state, section) = fixture();
    let station = StationRef::new(section, 1);
    let raw = "  17\u{0007} ".to_string();
    {
        let mut project = state.get_project_state().unwrap();
        let key = project.key_for_ref(station).unwrap();
        project.station_names.insert(key, raw.clone());
        state.set_project_for_tests(project);
    }

    assert!(set_name(&state, station, Some("18")));
    assert_eq!(name_of(&state, station).as_deref(), Some("18"));

    assert!(state.undo().success);
    assert_eq!(name_of(&state, station), Some(raw));

    assert!(state.redo().success);
    assert_eq!(name_of(&state, station).as_deref(), Some("18"));
}

#[test]
fn a_name_set_by_the_command_survives_save_and_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("names.stalx");
    let (state, section) = fixture();
    let station = StationRef::new(section, 2);
    assert!(set_name(&state, station, Some("Junction")));

    let project = state.get_project_state().unwrap();
    Database::open(&path)
        .unwrap()
        .save_project(&project)
        .unwrap();
    let reopened = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();
    assert_eq!(
        reopened.station_name_for_ref(station).map(String::as_str),
        Some("Junction")
    );
}

/// The Data workspace's multi-cell edit / find-replace: shot and station cells
/// in ONE batch, ONE undo entry, undone and redone as a unit.
#[test]
fn a_mixed_metadata_batch_is_one_undo_step() {
    let (state, section) = fixture();
    let station = StationRef::new(section, 1);
    let (shot_id, left_before) = first_shot(&state, section);
    assert!(set_name(&state, station, Some("old name")));
    let before = undo_len(&state);

    let result = state.execute(Command::Batch {
        commands: vec![
            Command::UpdateShot {
                section_id: section,
                shot_id,
                updates: ShotUpdates {
                    left: Some(Some(1.5)),
                    ..Default::default()
                },
            },
            Command::SetStationComment {
                station_ref: station,
                comment: Some("restriction".to_string()),
            },
            Command::SetStationName {
                station_ref: station,
                name: Some("  new name ".to_string()),
            },
        ],
    });
    assert!(result.success, "{:?}", result.error);
    assert_eq!(undo_len(&state), before + 1, "one batch, one undo entry");
    assert_eq!(first_shot(&state, section).1, Some(1.5));
    assert_eq!(comment_of(&state, station).as_deref(), Some("restriction"));
    assert_eq!(name_of(&state, station).as_deref(), Some("new name"));

    assert!(state.undo().success);
    assert_eq!(first_shot(&state, section).1, left_before);
    assert_eq!(comment_of(&state, station), None);
    assert_eq!(name_of(&state, station).as_deref(), Some("old name"));
    assert_eq!(
        undo_len(&state),
        before,
        "one undo takes the whole batch back"
    );

    assert!(state.redo().success);
    assert_eq!(first_shot(&state, section).1, Some(1.5));
    assert_eq!(comment_of(&state, station).as_deref(), Some("restriction"));
    assert_eq!(name_of(&state, station).as_deref(), Some("new name"));
}

#[test]
fn an_invalid_station_rolls_the_whole_batch_back() {
    let (state, section) = fixture();
    let station = StationRef::new(section, 1);
    let (shot_id, left_before) = first_shot(&state, section);
    let before = undo_len(&state);

    let result = state.execute(Command::Batch {
        commands: vec![
            Command::UpdateShot {
                section_id: section,
                shot_id,
                updates: ShotUpdates {
                    left: Some(Some(2.5)),
                    ..Default::default()
                },
            },
            Command::SetStationComment {
                station_ref: station,
                comment: Some("kept?".to_string()),
            },
            Command::SetStationName {
                station_ref: station,
                name: Some("kept?".to_string()),
            },
            // One past the last station: the batch must fail as a whole.
            Command::SetStationName {
                station_ref: StationRef::new(section, 3),
                name: Some("oob".to_string()),
            },
        ],
    });
    assert!(!result.success);
    assert_eq!(undo_len(&state), before, "nothing is recorded");
    assert_eq!(first_shot(&state, section).1, left_before);
    assert_eq!(comment_of(&state, station), None);
    assert_eq!(name_of(&state, station), None);
}

/// A batch of station metadata must not run the reconcile epilogue: healing an
/// already-drifted merge as a side effect of a text edit would leave an undo
/// that restores the text but not the geometry. The same gate as
/// `metadata_only_update_shot_does_not_heal_drift`, for the batch path.
#[test]
fn metadata_batch_does_not_heal_drift() {
    let state = AppState::new().unwrap();
    let mut project = Project::new("Drift".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(5.0), shot(5.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(5.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    project.resolve_offsets();
    assert!(project.check_merge_alignment().is_empty(), "starts aligned");
    // Drift WITHOUT reconciling: lengthen a shot upstream of the merge.
    project.get_section_mut(a).unwrap().shots[0].length = 9.0;
    assert!(!project.check_merge_alignment().is_empty(), "drifted");
    state.set_project_for_tests(project);

    let result = state.execute(Command::Batch {
        commands: vec![
            Command::SetStationName {
                station_ref: StationRef::new(a, 1),
                name: Some("A1".to_string()),
            },
            Command::SetStationComment {
                station_ref: StationRef::new(b, 0),
                comment: Some("note".to_string()),
            },
        ],
    });
    assert!(result.success, "{:?}", result.error);
    assert!(
        !state
            .get_project_state()
            .unwrap()
            .check_merge_alignment()
            .is_empty(),
        "a metadata-only batch must not reconcile (heal) a drifted merge"
    );
}

/// The nested batch payload the frontend sends (`BatchCommand` in
/// src/types/commands.ts): snake_case tags and fields, `null` for a removal.
#[test]
fn station_metadata_batch_commands_deserialize_from_the_frontend_shape() {
    let json = serde_json::json!([
        {
            "type": "set_station_name",
            "station_ref": { "section_id": 3, "station_index": 2 },
            "name": "A2"
        },
        {
            "type": "set_station_name",
            "station_ref": { "section_id": 3, "station_index": 2 },
            "name": null
        },
        {
            "type": "set_station_comment",
            "station_ref": { "section_id": 3, "station_index": 0 },
            "comment": null
        }
    ]);
    let commands: Vec<Command> = serde_json::from_value(json).unwrap();
    assert!(matches!(
        &commands[0],
        Command::SetStationName { station_ref, name: Some(n) }
            if *station_ref == StationRef::new(3, 2) && n == "A2"
    ));
    assert!(matches!(
        &commands[1],
        Command::SetStationName { name: None, .. }
    ));
    assert!(matches!(
        &commands[2],
        Command::SetStationComment { comment: None, .. }
    ));
}
