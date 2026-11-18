//! The undo-history labels of the edits recorded as one `Command::Batch`:
//! every composite edit and every frontend batch is named by the lines and
//! stations it touched (`undo::history_batch`), never `Batch of N commands`,
//! and keeps that label through undo, redo, a save and a reopen. An entry
//! written before labels existed keeps the old wording.

use std::path::Path;

use super::AppState;
use crate::core::undo::history_view::UndoHistoryItem;
use crate::core::{Command, UndoData, UndoEntry, UndoStack};
use crate::models::{
    ClosureStrategy, CombineLinesOptions, CombinePick, ImportLineConfig, ImportPlacement, LineId,
    ShotData, ShotUpdates, SplitAt, SplitLineOptions, SplitSide, StationRef,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn shots(n: usize) -> Vec<ShotData> {
    (0..n).map(|i| shot(90.0, 5.0 + i as f64)).collect()
}

fn new_state() -> AppState {
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Labels".to_string(),
                location: None,
            })
            .success
    );
    state
}

fn create(state: &AppState, name: &str, shots: Vec<ShotData>) -> LineId {
    let created = state.execute(Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots,
        cave_section_id: None,
    });
    assert!(created.success, "{:?}", created.error);
    created.line_id.unwrap()
}

fn top(state: &AppState) -> UndoHistoryItem {
    state.get_undo_stack().into_iter().next().unwrap()
}

/// A station's own name, as the project holds it now.
fn station_name(state: &AppState, line_id: LineId, index: u32) -> String {
    state
        .get_project_state()
        .unwrap()
        .station_name_at(StationRef::new(line_id, index))
        .unwrap()
        .to_string()
}

fn shot_id(state: &AppState, line_id: LineId, position: usize) -> u32 {
    state
        .get_project_state()
        .unwrap()
        .get_line(line_id)
        .unwrap()
        .shots[position]
        .id
}

/// The label holds through an undo and a redo (the entry moves between the
/// stacks with its command).
fn assert_label_survives_undo_redo(state: &AppState, label: &str) {
    assert!(state.undo().success);
    assert_eq!(state.get_redo_stack()[0].description, label);
    assert!(state.redo().success);
    assert_eq!(top(state).description, label);
}

#[test]
fn insert_station_names_the_station_it_made() {
    let state = new_state();
    let main = create(&state, "Main Line", shots(4));
    let shot = shot_id(&state, main, 2);
    assert!(
        state
            .execute_split_shot_at_distance(main, shot, 2.0)
            .success
    );
    // The new station sits after the inserted head shot, at index 3.
    let label = format!(
        "Insert station Main Line / {}",
        station_name(&state, main, 3)
    );
    let item = top(&state);
    assert_eq!(item.description, label);
    assert_eq!(item.command_type, "batch");
    assert_label_survives_undo_redo(&state, &label);
}

#[test]
fn combine_lines_names_both_lines() {
    let state = new_state();
    let main = create(&state, "Main Line", vec![shot(90.0, 10.0), shot(90.0, 5.0)]);
    let down = create(&state, "Downstream", vec![shot(0.0, 5.0), shot(0.0, 5.0)]);
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(main, 2),
                station_b: StationRef::new(down, 0),
            })
            .success
    );
    let options = CombineLinesOptions {
        name: "Main Line".to_string(),
        color: "#abcdef".to_string(),
        swap_lr: true,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: None,
    };
    let result = state.execute_combine_lines(main, down, options);
    assert!(result.success, "{:?}", result.error);
    let label = "Combine lines \"Main Line\" and \"Downstream\"";
    let item = top(&state);
    assert_eq!(item.description, label);
    // The preview names the line the combine deleted from the entry's
    // before-snapshot, though the project no longer holds it.
    let rows: Vec<String> = item
        .preview_data
        .unwrap()
        .changes
        .into_iter()
        .map(|c| c.field)
        .collect();
    assert!(
        rows.contains(&"Delete line \"Downstream\"".to_string()),
        "{rows:?}"
    );
    assert_label_survives_undo_redo(&state, label);
}

#[test]
fn create_line_and_merge_names_the_line_and_the_station() {
    let state = new_state();
    let main = create(&state, "Main Line", shots(3));
    let target = StationRef::new(main, 2);
    let result = state.execute_create_and_merge(
        Some("Side Passage".to_string()),
        None,
        None,
        "#00ff00".to_string(),
        None,
        shots(2),
        0,
        target,
        ClosureStrategy::Batch,
    );
    assert!(result.success, "{:?}", result.error);
    let label = format!(
        "Create line \"Side Passage\" at Main Line / {}",
        station_name(&state, main, 2)
    );
    assert_eq!(top(&state).description, label);
    assert_label_survives_undo_redo(&state, &label);
}

#[test]
fn split_line_names_the_line_and_the_station() {
    let state = new_state();
    let main = create(&state, "Main Line", shots(4));
    let at = station_name(&state, main, 2);
    let result = state.execute_split_line(
        main,
        SplitAt::Station { station_index: 2 },
        SplitLineOptions {
            name_a: "Main Line A".to_string(),
            name_b: "Main Line B".to_string(),
            color_a: "#ff0000".to_string(),
            color_b: "#00ff00".to_string(),
            comment_side: SplitSide::A,
            datum_side: SplitSide::A,
        },
    );
    assert!(result.success, "{:?}", result.error);
    let label = format!("Split line \"Main Line\" at {at}");
    assert_eq!(top(&state).description, label);
    assert_label_survives_undo_redo(&state, &label);
}

#[test]
fn split_line_inside_a_shot_names_the_station_it_made() {
    let state = new_state();
    let main = create(&state, "Main Line", shots(4));
    let shot = shot_id(&state, main, 1);
    let result = state.execute_split_line(
        main,
        SplitAt::Shot {
            shot_id: shot,
            distance: 2.5,
        },
        SplitLineOptions {
            name_a: "Main Line A".to_string(),
            name_b: "Main Line B".to_string(),
            color_a: "#ff0000".to_string(),
            color_b: "#00ff00".to_string(),
            comment_side: SplitSide::A,
            datum_side: SplitSide::A,
        },
    );
    assert!(result.success, "{:?}", result.error);
    // The first half ends at the station the cut made.
    let at = station_name(&state, main, 2);
    assert_eq!(
        top(&state).description,
        format!("Split line \"Main Line\" at {at}")
    );
}

#[test]
fn delete_and_split_names_the_shots_and_the_line() {
    let state = new_state();
    let main = create(&state, "Main Line", shots(5));
    let (from, to) = (station_name(&state, main, 2), station_name(&state, main, 3));
    let shot = shot_id(&state, main, 2);
    let result = state.execute_delete_shot_and_split(
        main,
        shot,
        "Main Line A".to_string(),
        "Main Line B".to_string(),
    );
    assert!(result.success, "{:?}", result.error);
    let label = format!("Delete shot {from} \u{2192} {to} and split line \"Main Line\"");
    assert_eq!(top(&state).description, label);
    assert_label_survives_undo_redo(&state, &label);
}

#[test]
fn range_deletes_name_the_span_and_the_line() {
    let state = new_state();
    let main = create(&state, "Main Line", shots(6));
    let name = |i| station_name(&state, main, i);
    let (from, to) = (name(1), name(4));
    let (first, last) = (shot_id(&state, main, 1), shot_id(&state, main, 3));
    let result = state.execute_delete_shot_range_split(
        main,
        first,
        last,
        "Main Line A".to_string(),
        "Main Line B".to_string(),
    );
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        top(&state).description,
        format!("Delete 3 shots ({from} \u{2192} {to}) and split line \"Main Line\"")
    );

    let side = create(&state, "Side", shots(5));
    let (from, to) = (station_name(&state, side, 0), station_name(&state, side, 2));
    let (first, last) = (shot_id(&state, side, 0), shot_id(&state, side, 1));
    let result = state.execute_delete_shot_range_shift(side, first, last);
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        top(&state).description,
        format!("Delete 2 shots ({from} \u{2192} {to}) from line \"Side\"")
    );

    let stub = create(&state, "Stub", shots(1));
    let only = shot_id(&state, stub, 0);
    assert!(state.execute_delete_last_shot_and_line(stub, only).success);
    let label = "Delete line \"Stub\" with its last shot";
    assert_eq!(top(&state).description, label);
    assert_label_survives_undo_redo(&state, label);
}

fn import_config(index: u32, name: &str) -> ImportLineConfig {
    ImportLineConfig {
        cave_section: None,
        line_index: index,
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        surveyors: None,
        explorers: None,
        override_date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(90.0, 5.0 + f64::from(index))],
        placement: ImportPlacement::Origin,
        station_comments: Vec::new(),
        station_names: Vec::new(),
        reverse: false,
        azimuth_reference: None,
    }
}

#[test]
fn an_import_names_the_lines_it_made() {
    let state = new_state();
    let result = state.execute_import_lines(
        ["Gallery", "Sump", "Loose", "Upper", "Lower"]
            .iter()
            .enumerate()
            .map(|(i, name)| import_config(i as u32, name))
            .collect(),
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        super::VirtualShotMode::Auto,
    );
    assert!(result.success, "{:?}", result.error);
    let label = "Import 5 lines (\"Gallery\", \"Sump\", \"Loose\" and 2 more)";
    let item = top(&state);
    assert_eq!(item.description, label);
    assert_label_survives_undo_redo(&state, label);
}

#[test]
fn a_frontend_batch_is_summarized_by_name() {
    let state = new_state();
    let main = create(&state, "Main", shots(3));
    let side = create(&state, "Side", shots(2));
    // A data-table paste: one line, two shots.
    let paste = (0..2)
        .map(|i| Command::UpdateShot {
            line_id: main,
            shot_id: shot_id(&state, main, i),
            updates: ShotUpdates {
                comment: Some(Some(format!("pasted {i}"))),
                ..Default::default()
            },
        })
        .collect();
    assert!(
        state
            .execute(Command::Batch {
                commands: paste,
                label: None,
            })
            .success
    );
    assert_eq!(top(&state).description, "Update 2 shots in line \"Main\"");

    // Reverse Lines: the names they had, not ids.
    let reverse = [main, side]
        .into_iter()
        .map(|line_id| Command::ReverseLine {
            line_id,
            swap_left_right: false,
        })
        .collect();
    assert!(
        state
            .execute(Command::Batch {
                commands: reverse,
                label: None,
            })
            .success
    );
    let label = "Reverse lines \"Main\" and \"Side\"";
    assert_eq!(top(&state).description, label);
    assert_label_survives_undo_redo(&state, label);

    // Deleting lines is named by the names they had before the batch ran.
    let delete = vec![
        Command::DeleteLine { line_id: main },
        Command::DeleteLine { line_id: side },
    ];
    assert!(
        state
            .execute(Command::Batch {
                commands: delete,
                label: None,
            })
            .success
    );
    assert_eq!(
        top(&state).description,
        "Delete lines \"Main\" and \"Side\""
    );
}

#[test]
fn the_label_rides_the_persisted_command_and_an_old_entry_keeps_its_wording() {
    let entry = |label: Option<&str>| UndoEntry {
        command: Command::Batch {
            commands: vec![
                Command::DeleteLine { line_id: 1 },
                Command::DeleteLine { line_id: 2 },
            ],
            label: label.map(str::to_string),
        },
        undo_data: UndoData::None,
        timestamp: None,
        command_bytes: None,
    };
    let (command_json, undo_json) =
        UndoStack::serialize_entry(&entry(Some("Combine lines \"Main\" and \"Side\""))).unwrap();
    assert!(command_json.contains("\"label\":\"Combine lines \\\"Main\\\" and \\\"Side\\\"\""));
    let back = UndoStack::deserialize_entry(&command_json, &undo_json).unwrap();
    assert_eq!(
        back.command.description(),
        "Combine lines \"Main\" and \"Side\""
    );

    // No label: no key at all, so an older build reads the entry as it
    // always did, and the old wording stays.
    let (command_json, undo_json) = UndoStack::serialize_entry(&entry(None)).unwrap();
    assert!(!command_json.contains("label"), "{command_json}");
    let old = UndoStack::deserialize_entry(&command_json, &undo_json).unwrap();
    assert_eq!(old.command.description(), "Batch of 2 commands");
}

fn save_close_reopen(state: &AppState, path: &Path) {
    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path.to_path_buf()).success, "reopen");
}

#[test]
fn a_label_survives_a_save_and_a_reopen() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("labels.stalx");
    let state = new_state();
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    let main = create(&state, "Main Line", shots(4));
    let shot = shot_id(&state, main, 1);
    assert!(
        state
            .execute_split_shot_at_distance(main, shot, 2.0)
            .success
    );
    let label = top(&state).description;
    assert!(label.starts_with("Insert station Main Line / "), "{label}");
    save_close_reopen(&state, &path);
    assert_eq!(top(&state).description, label);
}
