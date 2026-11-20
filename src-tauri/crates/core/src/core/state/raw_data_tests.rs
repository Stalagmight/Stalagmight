//! The data table's "Raw data" view on a version 2 file: the saved file's
//! members as tables, one row per record, as of the last save.
//!
//! All data is synthetic: projects built in memory and saved by this build.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: panics on a failed assumption are the point"
)]

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::survey::{LineMeta, Project, StartAt};
use crate::models::{ShotData, ShotUpdates};
use crate::persistence::stalx_v2::{self, Extras, Mode};
use crate::persistence::RawTableData;

/// Two lines through a junction, one station with a comment, placed.
fn survey() -> Project {
    let shot = |length: f64| ShotData {
        length,
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        left: Some(1.5),
        ..ShotData::default()
    };
    let mut p = Project::new("Raw", None);
    let main = p
        .add_line(
            LineMeta {
                name: Some("Main".to_string()),
                ..LineMeta::default()
            },
            StartAt::New,
            vec![shot(3.0), shot(4.0)],
        )
        .unwrap();
    let junction = p.line(main).unwrap().station(1).unwrap();
    p.add_line(
        LineMeta {
            name: Some("Side".to_string()),
            ..LineMeta::default()
        },
        StartAt::Station(junction),
        vec![shot(2.0)],
    )
    .unwrap();
    p.station_mut(junction).unwrap().comment = Some("tie-off".to_string());
    let start = p.line(main).unwrap().start;
    p.set_placement(start, 0.0, 0.0).unwrap();
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    p
}

fn saved(dir: &Path) -> (PathBuf, AppState) {
    let path = dir.join("raw.stalx");
    std::fs::write(
        &path,
        stalx_v2::encode(&survey(), &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject { path: path.clone() });
    assert!(r.success, "{:?}", r.error);
    (path, state)
}

fn column<'a>(table: &'a RawTableData, name: &str) -> Vec<&'a str> {
    assert!(
        table.columns.iter().any(|c| c == name),
        "no column {name} in {:?}",
        table.columns
    );
    let at = table.columns.iter().position(|c| c == name).unwrap();
    table.rows.iter().map(|row| row[at].as_str()).collect()
}

#[test]
fn the_tables_are_the_files_members_in_file_order() {
    let dir = TempDir::new().unwrap();
    let (_path, state) = saved(dir.path());

    let names = state.list_raw_table_names().unwrap();

    assert_eq!(
        names,
        [
            "file",
            "summary",
            "project",
            "settings",
            "counters",
            "stations",
            "lines",
            "shots",
            "placements"
        ]
    );
}

#[test]
fn every_shot_is_a_row_with_its_line_and_index() {
    let dir = TempDir::new().unwrap();
    let (_path, state) = saved(dir.path());
    let project = state.get_project_state().unwrap();

    let shots = state.read_raw_table("shots", 1000, 0).unwrap();

    assert_eq!(shots.total_rows, 3);
    assert_eq!(
        &shots.columns[..9],
        [
            "line",
            "index",
            "uuid",
            "to",
            "length",
            "azimuth_in",
            "azimuth_out",
            "depth_in",
            "depth_out"
        ]
    );
    let main = project.lines()[0].id.to_string();
    let side = project.lines()[1].id.to_string();
    assert_eq!(
        column(&shots, "line"),
        [main.as_str(), main.as_str(), side.as_str()]
    );
    assert_eq!(column(&shots, "index"), ["0", "1", "0"]);
    assert_eq!(column(&shots, "length"), ["3.0", "4.0", "2.0"]);
    assert_eq!(column(&shots, "left"), ["1.5", "1.5", "1.5"]);
    let uuids: Vec<String> = project
        .lines()
        .iter()
        .flat_map(|l| l.shots.iter().map(|s| s.uuid.clone()))
        .collect();
    assert_eq!(column(&shots, "uuid"), uuids);
}

#[test]
fn stations_lines_and_single_records_read_as_tables() {
    let dir = TempDir::new().unwrap();
    let (_path, state) = saved(dir.path());

    let stations = state.read_raw_table("stations", 1000, 0).unwrap();
    assert_eq!(stations.total_rows, 4, "a junction is one station");
    assert_eq!(&stations.columns[..3], ["id", "name", "comment"]);
    assert_eq!(
        column(&stations, "comment")
            .into_iter()
            .filter(|c| !c.is_empty())
            .collect::<Vec<_>>(),
        ["tie-off"]
    );

    let lines = state.read_raw_table("lines", 1000, 0).unwrap();
    assert_eq!(column(&lines, "name"), ["Main", "Side"]);
    assert_eq!(column(&lines, "shots"), ["2", "1"]);

    let project = state.read_raw_table("project", 1000, 0).unwrap();
    assert_eq!(project.total_rows, 1);
    assert_eq!(column(&project, "name"), ["Raw"]);

    let file = state.read_raw_table("file", 1000, 0).unwrap();
    assert_eq!(column(&file, "format"), ["stalx"]);
    assert_eq!(column(&file, "version"), ["2"]);
}

#[test]
fn pages_slice_the_rows_and_count_them_all() {
    let dir = TempDir::new().unwrap();
    let (_path, state) = saved(dir.path());

    let page = state.read_raw_table("shots", 2, 1).unwrap();

    assert_eq!(page.total_rows, 3);
    assert_eq!(page.rows.len(), 2);
    assert_eq!(column(&page, "length"), ["4.0", "2.0"]);
    assert!(state
        .read_raw_table("shots", 10, 10)
        .unwrap()
        .rows
        .is_empty());
}

/// The view is the last save: an edit shows once it is saved, with the
/// history entry that recorded it.
#[test]
fn the_view_is_as_of_the_last_save() {
    let dir = TempDir::new().unwrap();
    let (_path, state) = saved(dir.path());
    let (line_id, shot_id) = {
        let p = state.get_project_state().unwrap();
        (p.lines()[0].id, p.lines()[0].shots[0].id)
    };
    let edit = state.execute(Command::UpdateShot {
        line_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(3.25),
            ..ShotUpdates::default()
        },
    });
    assert!(edit.success, "{:?}", edit.error);
    let before_save = state.read_raw_table("shots", 1, 0).unwrap();
    assert_eq!(column(&before_save, "length"), ["3.0"]);
    assert!(!state
        .list_raw_table_names()
        .unwrap()
        .contains(&"history".to_string()));

    assert!(state.execute(Command::SaveProject { path: None }).success);

    let after_save = state.read_raw_table("shots", 1, 0).unwrap();
    assert_eq!(column(&after_save, "length"), ["3.25"]);
    let history = state.read_raw_table("history", 1000, 0).unwrap();
    assert_eq!(history.total_rows, 1);
    assert_eq!(column(&history, "list"), ["undo"]);
    assert_eq!(column(&history, "k"), ["update_shot"]);
}

#[test]
fn a_project_without_a_version_2_file_has_no_raw_data() {
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Unsaved".to_string(),
                location: None,
            })
            .success
    );
    let err = state.list_raw_table_names().unwrap_err().to_string();
    assert!(err.contains("save the project"), "{err}");

    let dir = TempDir::new().unwrap();
    let legacy = dir.path().join("legacy.stalx");
    let mut db = crate::persistence::Database::open(&legacy).unwrap();
    db.save_project(&crate::models::Project::new("Legacy".to_string(), None))
        .unwrap();
    drop(db);
    let state = AppState::new().unwrap();
    assert!(state.execute(Command::OpenProject { path: legacy }).success);
    let err = state
        .read_raw_table("project", 10, 0)
        .unwrap_err()
        .to_string();
    assert!(err.contains("old file format"), "{err}");
}

#[test]
fn an_unknown_table_is_refused_and_a_long_cell_is_cut() {
    let dir = TempDir::new().unwrap();
    let (path, state) = saved(dir.path());
    let err = state.read_raw_table("sqlite_master", 10, 0).unwrap_err();
    assert_eq!(err.to_string(), "No such table: sqlite_master");

    // A long comment, saved: the cell is cut and says how long it was.
    let mut project = survey();
    let first = *project.stations.keys().next().unwrap();
    project.station_mut(first).unwrap().comment = Some("x".repeat(2000));
    std::fs::write(
        &path,
        stalx_v2::encode(&project, &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();
    let stations = state.read_raw_table("stations", 1, 0).unwrap();
    let cell = column(&stations, "comment")[0];
    assert!(cell.ends_with("... (2000 bytes)"), "{cell}");
    assert_eq!(
        cell.chars().filter(|c| *c == 'x').count(),
        super::raw_data::MAX_CELL_CHARS
    );
}
