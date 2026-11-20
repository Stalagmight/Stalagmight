//! The undo-history labels of the edits recorded as one step of several
//! commands: every composite edit and every frontend batch is named by the
//! lines and stations it touched (`labels::batch_summary` and the recipe
//! labels), never `Batch of N commands`, and keeps that label through undo,
//! redo, a save and a reopen.
//!
//! The recipe cases (Insert Station, Combine, Split, the delete recipes,
//! Create Line At, Import) drive their recipe and are ignored until the
//! package that ports the recipe lands; the wording itself is pinned against
//! the label helpers in `history_labels_tests.rs`.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::commands::Record;
use super::labels;
use super::AppState;
use crate::core::{Command, UndoHistoryItem};
use crate::models::survey::changeset::{decode_entry, encode_entry, EntryLimits};
use crate::models::survey::{LineMeta, Project, StartAt};
use crate::models::{
    ClosureStrategy, CombineLinesOptions, CombinePick, Coordinates, GpsCoordinates,
    ImportLineConfig, ImportPlacement, LineId, LineUpdates, ShotData, ShotKind, ShotUpdates,
    SplitAt, SplitLineOptions, StationRef,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};

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

/// A survey of free lines named `names`, `shots` shots each, laid 50 m
/// apart. Line ids are 0, 1, ... in order.
fn survey(names: &[(&str, usize)]) -> Project {
    let mut p = Project::new("Labels", None);
    for (i, (name, n)) in names.iter().enumerate() {
        let id = p
            .add_line(
                LineMeta {
                    name: Some((*name).to_string()),
                    color: "#ff0000".to_string(),
                    ..LineMeta::default()
                },
                StartAt::New,
                shots(*n),
            )
            .unwrap();
        let start = p.line(id).unwrap().start;
        p.set_placement(start, 0.0, 50.0 * i as f64).unwrap();
    }
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    p
}

fn write(dir: &Path, project: &Project) -> PathBuf {
    let path = dir.join("labels.stalx");
    std::fs::write(
        &path,
        stalx_v2::encode(project, &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();
    path
}

fn open(path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(r.success, "{:?}", r.error);
    state
}

fn top(state: &AppState) -> UndoHistoryItem {
    state.get_undo_stack().into_iter().next().unwrap()
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

/// A station's own name, as the project holds it now.
fn station_name(state: &AppState, line_id: LineId, index: u32) -> String {
    labels::NameBook::of(&project(state))
        .station_name(StationRef::new(line_id, index))
        .unwrap()
}

fn shot_id(state: &AppState, line_id: LineId, position: usize) -> u32 {
    project(state).line(line_id).unwrap().shots[position].id
}

/// The label holds through an undo and a redo (the entry moves between the
/// stacks as it is).
fn assert_label_survives_undo_redo(state: &AppState, label: &str) {
    assert!(state.undo().success);
    assert_eq!(state.get_redo_stack()[0].description, label);
    assert!(state.redo().success);
    assert_eq!(top(state).description, label);
}

fn batch(state: &AppState, commands: Vec<Command>) {
    let r = state.execute(Command::Batch {
        commands,
        label: None,
    });
    assert!(r.success, "{:?}", r.error);
}

#[test]
fn insert_station_names_the_station_it_made() {
    let dir = TempDir::new().unwrap();
    let state = open(&write(dir.path(), &survey(&[("Main Line", 4)])));
    let shot = shot_id(&state, 0, 2);
    assert!(state.execute_split_shot_at_distance(0, shot, 2.0).success);
    // The new station sits after the inserted head shot, at index 3.
    let label = format!("Insert station Main Line / {}", station_name(&state, 0, 3));
    let item = top(&state);
    assert_eq!(item.description, label);
    assert_eq!(item.command_type, "batch");
    assert_label_survives_undo_redo(&state, &label);
}

#[test]
fn combine_lines_names_both_lines() {
    let dir = TempDir::new().unwrap();
    let mut p = survey(&[("Main Line", 2)]);
    let end = p.line(0).unwrap().stations().nth(2).unwrap();
    p.add_line(
        LineMeta {
            name: Some("Downstream".to_string()),
            ..LineMeta::default()
        },
        StartAt::Station(end),
        vec![shot(0.0, 5.0), shot(0.0, 5.0)],
    )
    .unwrap();
    let state = open(&write(dir.path(), &p));
    let options = CombineLinesOptions {
        name: "Main Line".to_string(),
        color: "#abcdef".to_string(),
        swap_lr: true,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: None,
    };
    let result = state.execute_combine_lines(0, 1, options);
    assert!(result.success, "{:?}", result.error);
    let label = "Combine lines \"Main Line\" and \"Downstream\"";
    let item = top(&state);
    assert_eq!(item.description, label);
    // The preview names the line the combine deleted, though the project no
    // longer holds it.
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
    let dir = TempDir::new().unwrap();
    let state = open(&write(dir.path(), &survey(&[("Main Line", 3)])));
    let target = StationRef::new(0, 2);
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
        station_name(&state, 0, 2)
    );
    assert_eq!(top(&state).description, label);
    assert_label_survives_undo_redo(&state, &label);
}

fn split_options() -> SplitLineOptions {
    SplitLineOptions {
        name_a: "Main Line A".to_string(),
        name_b: "Main Line B".to_string(),
        color_a: "#ff0000".to_string(),
        color_b: "#00ff00".to_string(),
    }
}

#[test]
fn split_line_names_the_line_and_the_station() {
    let dir = TempDir::new().unwrap();
    let state = open(&write(dir.path(), &survey(&[("Main Line", 4)])));
    let at = station_name(&state, 0, 2);
    let result =
        state.execute_split_line(0, SplitAt::Station { station_index: 2 }, split_options());
    assert!(result.success, "{:?}", result.error);
    let label = format!("Split line \"Main Line\" at {at}");
    assert_eq!(top(&state).description, label);
    assert_label_survives_undo_redo(&state, &label);
}

#[test]
fn split_line_inside_a_shot_names_the_station_it_made() {
    let dir = TempDir::new().unwrap();
    let state = open(&write(dir.path(), &survey(&[("Main Line", 4)])));
    let shot = shot_id(&state, 0, 1);
    let result = state.execute_split_line(
        0,
        SplitAt::Shot {
            shot_id: shot,
            distance: 2.5,
        },
        split_options(),
    );
    assert!(result.success, "{:?}", result.error);
    // The first half ends at the station the cut made.
    let at = station_name(&state, 0, 2);
    assert_eq!(
        top(&state).description,
        format!("Split line \"Main Line\" at {at}")
    );
}

#[test]
fn delete_and_split_names_the_shots_and_the_line() {
    let dir = TempDir::new().unwrap();
    let state = open(&write(dir.path(), &survey(&[("Main Line", 5)])));
    let (from, to) = (station_name(&state, 0, 2), station_name(&state, 0, 3));
    let shot = shot_id(&state, 0, 2);
    let result = state.execute_delete_shot_and_split(
        0,
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
    let dir = TempDir::new().unwrap();
    let state = open(&write(
        dir.path(),
        &survey(&[("Main Line", 6), ("Side", 5), ("Stub", 1)]),
    ));
    let name = |line, i| station_name(&state, line, i);
    let (from, to) = (name(0, 1), name(0, 4));
    let (first, last) = (shot_id(&state, 0, 1), shot_id(&state, 0, 3));
    let result = state.execute_delete_shot_range_split(
        0,
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

    let (from, to) = (name(1, 0), name(1, 2));
    let (first, last) = (shot_id(&state, 1, 0), shot_id(&state, 1, 1));
    let result = state.execute_delete_shot_range_shift(1, first, last);
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        top(&state).description,
        format!("Delete 2 shots ({from} \u{2192} {to}) from line \"Side\"")
    );

    let only = shot_id(&state, 2, 0);
    assert!(state.execute_delete_last_shot_and_line(2, only).success);
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
        station_external: Vec::new(),
        reverse: false,
        azimuth_reference: None,
    }
}

#[test]
fn an_import_names_the_lines_it_made() {
    let dir = TempDir::new().unwrap();
    let state = open(&write(dir.path(), &survey(&[("Existing", 1)])));
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
    let dir = TempDir::new().unwrap();
    let state = open(&write(dir.path(), &survey(&[("Main", 3), ("Side", 2)])));
    // A data-table paste: one line, two shots.
    let paste = (0..2)
        .map(|i| Command::UpdateShot {
            line_id: 0,
            shot_id: shot_id(&state, 0, i),
            updates: ShotUpdates {
                comment: Some(Some(format!("pasted {i}"))),
                ..Default::default()
            },
        })
        .collect();
    batch(&state, paste);
    assert_eq!(top(&state).description, "Update 2 shots in line \"Main\"");

    // Reverse Lines: the names they had, not ids.
    let reverse = [0, 1]
        .into_iter()
        .map(|line_id| Command::ReverseLine {
            line_id,
            swap_left_right: false,
        })
        .collect();
    batch(&state, reverse);
    let label = "Reverse lines \"Main\" and \"Side\"";
    assert_eq!(top(&state).description, label);
    assert_label_survives_undo_redo(&state, label);

    // Deleting lines is named by the names they had before the batch ran.
    batch(
        &state,
        vec![
            Command::DeleteLine { line_id: 0 },
            Command::DeleteLine { line_id: 1 },
        ],
    );
    assert_eq!(
        top(&state).description,
        "Delete lines \"Main\" and \"Side\""
    );
    assert_label_survives_undo_redo(&state, "Delete lines \"Main\" and \"Side\"");
}

/// 0.6.0's `history_batch::summarize` cases, worded on the station graph.
#[test]
fn summaries_name_what_a_batch_did_together() {
    let p = survey(&[("Main", 3), ("Side", 3)]);
    let summary = |cmds: &[Command]| labels::batch_summary(&p, cmds);
    let shot = |line: LineId, i: usize| p.line(line).unwrap().shots[i].id;
    let update = |line: LineId, i: usize| Command::UpdateShot {
        line_id: line,
        shot_id: shot(line, i),
        updates: ShotUpdates::default(),
    };
    assert_eq!(
        summary(&[update(0, 0), update(0, 1), update(0, 1)]),
        "Update 2 shots in line \"Main\""
    );
    assert_eq!(
        summary(&[update(0, 0), update(1, 1)]),
        "Update 2 shots in 2 lines"
    );
    let kind = |i: usize, kind| Command::SetShotKind {
        line_id: 0,
        shot_id: shot(0, i),
        kind,
    };
    assert_eq!(
        summary(&[kind(0, ShotKind::Jump), kind(1, ShotKind::Jump)]),
        "Mark 2 shots in line \"Main\" as jump"
    );
    assert_eq!(
        summary(&[kind(0, ShotKind::Jump), kind(1, ShotKind::Reference)]),
        "Change the kind of 2 shots in line \"Main\""
    );
    assert_eq!(
        summary(&[kind(0, ShotKind::Jump), update(0, 2)]),
        "Edit 2 shots in line \"Main\""
    );
    let rename = |i: u32| Command::SetStationName {
        station_ref: StationRef::new(0, i),
        name: format!("S{i}"),
    };
    assert_eq!(
        summary(&[rename(1), rename(2)]),
        "Rename 2 stations in line \"Main\""
    );

    // A withdrawal: the deletes stand for the import bookkeeping.
    let withdraw = [
        Command::DeleteLine { line_id: 0 },
        Command::DeleteLine { line_id: 1 },
        Command::UntrackImport {
            file_hashes: vec![],
            line_hashes: vec![],
        },
    ];
    assert_eq!(summary(&withdraw), "Delete lines \"Main\" and \"Side\"");
    // Edit Line's Save: the metadata edit stands for its credit edit.
    let edit = [
        Command::UpdateLineMetadata {
            line_id: 0,
            updates: LineUpdates::default(),
        },
        Command::UpdateLineMetadata {
            line_id: 0,
            updates: LineUpdates::default(),
        },
    ];
    assert_eq!(summary(&edit), "Update metadata of line \"Main\"");

    // A move snapped onto a station is the merge.
    let (a, b) = (StationRef::new(0, 1), StationRef::new(1, 0));
    let snapped = [
        Command::MoveStation {
            station_ref: b,
            offset: Coordinates::default(),
        },
        Command::MergeStations {
            station_a: b,
            station_b: a,
        },
    ];
    assert_eq!(summary(&snapped), "Merge stations Side / 0 and Main / 1");
    let moves = [
        Command::MoveStation {
            station_ref: a,
            offset: Coordinates::default(),
        },
        Command::MoveStation {
            station_ref: b,
            offset: Coordinates::default(),
        },
    ];
    assert_eq!(summary(&moves), "Move stations Main / 1 and Side / 0");

    // Unrelated kinds name the first and count the rest.
    let datum = [
        Command::SetZeroDatum {
            station_ref: a,
            coordinates: GpsCoordinates::new(20.0, -87.0, 0.0).unwrap(),
            accuracy_m: None,
            entrance: None,
        },
        Command::RemoveZeroDatum { station_ref: a },
    ];
    assert_eq!(
        summary(&datum),
        "Set zero datum at Main / 1 and 1 more change"
    );
    assert_eq!(summary(&[]), "Batch of 0 commands");
    // Many lines: three names, then a count.
    let p = survey(&[("A", 1), ("B", 1), ("C", 1), ("D", 1), ("E", 1)]);
    let reverse: Vec<Command> = (0..5)
        .map(|line_id| Command::ReverseLine {
            line_id,
            swap_left_right: false,
        })
        .collect();
    assert_eq!(
        labels::batch_summary(&p, &reverse),
        "Reverse lines \"A\", \"B\", \"C\" and 2 more"
    );
}

#[test]
fn the_label_rides_the_persisted_entry() {
    let dir = TempDir::new().unwrap();
    let state = open(&write(dir.path(), &survey(&[("Main", 2), ("Side", 2)])));
    let r = state.execute(Command::Batch {
        commands: vec![
            Command::DeleteLine { line_id: 0 },
            Command::DeleteLine { line_id: 1 },
        ],
        label: Some("Combine lines \"Main\" and \"Side\"".to_string()),
    });
    assert!(r.success, "{:?}", r.error);
    let stack = state.undo_stack.lock().unwrap();
    let text = encode_entry(stack.undo_entries().next().unwrap().entry()).unwrap();
    assert!(
        text.contains("\"label\":\"Combine lines \\\"Main\\\" and \\\"Side\\\"\""),
        "{text}"
    );
    let back = decode_entry(&text, &EntryLimits::default()).unwrap();
    assert_eq!(back.label, "Combine lines \"Main\" and \"Side\"");
    assert_eq!(back.kind, "batch");
}

fn save_close_reopen(state: &AppState, path: &Path) {
    assert!(state.execute(Command::SaveProject { path: None }).success);
    assert!(state.execute(Command::CloseProject).success);
    let r = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(r.success, "reopen: {:?}", r.error);
}

#[test]
fn a_label_survives_a_save_and_a_reopen() {
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), &survey(&[("Main Line", 4)]));
    let state = open(&path);
    let paste = (0..3)
        .map(|i| Command::UpdateShot {
            line_id: 0,
            shot_id: shot_id(&state, 0, i),
            updates: ShotUpdates {
                left: Some(Some(1.0)),
                ..Default::default()
            },
        })
        .collect();
    batch(&state, paste);
    let item = top(&state);
    assert_eq!(item.description, "Update 3 shots in line \"Main Line\"");
    save_close_reopen(&state, &path);
    let back = top(&state);
    assert_eq!(back.description, item.description);
    assert_eq!(
        serde_json::to_value(&back.preview_data).unwrap(),
        serde_json::to_value(&item.preview_data).unwrap()
    );
}

/// A Remove Loop Closure entry names its two stations from its own record,
/// whatever else is on the stack. 0.6.0 read the names only for the lines
/// some command on the stack addressed, so alone on a stack (a save that
/// kept a short persisted window, then a reopen) its preview fell back to
/// `-`; now the names are worded when the entry is recorded.
#[test]
fn a_removed_closure_names_its_stations_alone_on_the_stack() {
    let dir = TempDir::new().unwrap();
    let mut p = survey(&[("Upstream", 3)]);
    let branch = p.line(0).unwrap().stations().nth(1).unwrap();
    p.add_line(
        LineMeta {
            name: Some("Downstream".to_string()),
            ..LineMeta::default()
        },
        StartAt::Station(branch),
        vec![shot(0.0, 5.0), shot(10.0, 5.0)],
    )
    .unwrap();
    p.create_closure_at(
        StationRef::new(0, 3),
        StationRef::new(1, 2),
        crate::models::survey::ClosureStrategy::Batch,
    )
    .unwrap();
    let path = write(dir.path(), &p);
    let state = open(&path);
    let closure_id = project(&state).closures[0].id;
    let cmd = Command::RemoveLoopClosure { closure_id };
    let r = state.run_recorded(
        |p| Record::of(p, &cmd),
        |p, _| {
            p.remove_closure(closure_id)
                .map(|_| ())
                .map_err(|e| e.to_string())
        },
    );
    assert!(r.is_ok());
    save_close_reopen(&state, &path);
    assert_eq!(state.get_undo_stack().len(), 1);
    let stations = top(&state)
        .preview_data
        .unwrap()
        .changes
        .into_iter()
        .find(|change| change.field == "Stations")
        .unwrap()
        .new_value;
    assert_eq!(
        stations,
        format!(
            "Upstream / 3 \u{2194} Downstream / {}",
            station_name(&state, 1, 2)
        )
    );
}
