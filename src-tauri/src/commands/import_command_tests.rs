//! Command-layer tests for the Ariane, Compass and Walls file imports (#576
//! NF-IMPORT-APPHANDLE).
//!
//! `execute_tml_import`, `execute_compass_import` and `execute_walls_import`
//! are generic over the Tauri runtime (#576 C2/C9), so these tests call the
//! registered commands themselves with a `tauri::test` app: one IPC option
//! toggled both ways, the receipt the importer labels, and the file hash the
//! import records. Every file is synthetic, written into a temp dir here.

use std::path::{Path, PathBuf};

use tauri::async_runtime::block_on;
use tauri::test::MockRuntime;
use tauri::{App, Manager};
use tempfile::TempDir;

use super::command_tests::made_for;
use super::import::{
    check_duplicate_import, execute_compass_import, execute_tml_import, execute_walls_import,
};
use crate::core::state::AppState;
use crate::core::{CommandError, ImportOutcome};
use stalagmight_core::test_fixtures::state_with_project;

/// A `tauri::test` app managing a fresh app state with an open, empty project.
fn project_app() -> App<MockRuntime> {
    let state = state_with_project("CaveA");
    let app = tauri::test::mock_app();
    app.manage(state);
    app
}

fn state_of(app: &App<MockRuntime>) -> &AppState {
    app.state::<AppState>().inner()
}

/// The shots now in the project, across all sections.
fn shots_in(app: &App<MockRuntime>) -> usize {
    state_of(app)
        .get_project_state()
        .expect("project")
        .sections
        .iter()
        .map(|s| s.shots.len())
        .sum()
}

pub(super) fn write(dir: &TempDir, name: &str, body: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, body).expect("write the synthetic file");
    path
}

/// The receipt's source label, unit note and found count, after asserting the
/// import succeeded.
fn receipt_of(result: &Result<ImportOutcome, CommandError>) -> (String, Option<String>, u32) {
    let result = result.as_ref().expect("the import succeeds");
    let receipt = result.import_receipt.as_ref().expect("an import receipt");
    (
        receipt.source_format.clone(),
        receipt.unit_conversion.clone(),
        receipt.sections_found,
    )
}

fn hash_recorded(app: &App<MockRuntime>, hash: &str) -> bool {
    block_on(check_duplicate_import(
        app.handle().clone(),
        hash.to_string(),
    ))
    .expect("check the hash")
}

/// Ariane: a short survey in feet, with an entrance elevation of 328.084 ft
/// (100 m).
pub(super) fn synthetic_tmlu(dir: &TempDir) -> PathBuf {
    use crate::parsers::tml::test_helpers::{tml_shot_xml_depth, tml_shot_xml_gps, wrap_xml_with};
    let shots = [
        tml_shot_xml_gps(0, -1, "START", "Main", 20.5, -87.3),
        tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
        tml_shot_xml_depth(2, 1, "REAL", "Main", 12.0, -2.0),
        tml_shot_xml_depth(3, 2, "REAL", "Side", 13.0, -3.0),
        tml_shot_xml_depth(4, 3, "REAL", "Side", 14.0, -4.0),
    ]
    .join("\n");
    write(dir, "CaveA.tmlu", &wrap_xml_with("ft", 328.084, &shots))
}

fn tml_import(
    path: &Path,
    hash: Option<&str>,
    sections: Option<Vec<u32>>,
) -> (App<MockRuntime>, Result<ImportOutcome, CommandError>) {
    let app = project_app();
    let result = block_on(execute_tml_import(
        app.handle().clone(),
        made_for(&app),
        path.to_path_buf(),
        hash.map(str::to_string),
        false,
        false,
        None,
        sections,
        None,
    ));
    (app, result)
}

#[test]
fn the_ariane_command_honours_selected_sections_and_records_the_file() {
    let dir = TempDir::new().expect("temp dir");
    let path = synthetic_tmlu(&dir);

    let (all, result) = tml_import(&path, Some("hash-tml"), None);
    let (source, unit_note, found) = receipt_of(&result);
    assert_eq!(source, "Ariane TML");
    assert!(unit_note.is_some(), "a feet file says it converted");
    assert!(found >= 1, "the file's sections are found, got {found}");
    assert!(
        hash_recorded(&all, "hash-tml"),
        "the import records its file hash"
    );
    let elevation = state_of(&all)
        .get_project_state()
        .and_then(|p| p.surface_elevation);
    assert_eq!(
        elevation.map(|e| (e * 1000.0).round()),
        Some(100_000.0),
        "the Ariane apply path carries the entrance elevation"
    );

    let (every, result) = tml_import(&path, None, Some((0..found).collect()));
    receipt_of(&result);
    let (none, result) = tml_import(&path, None, Some(Vec::new()));
    receipt_of(&result);
    println!(
        "ariane shots: no selection {}, every section selected {}, empty selection {}",
        shots_in(&all),
        shots_in(&every),
        shots_in(&none)
    );
    assert!(shots_in(&all) > 0, "premise: the file imports shots");
    assert_eq!(
        shots_in(&every),
        shots_in(&all),
        "selecting every section is no selection"
    );
    assert_eq!(
        shots_in(&none),
        0,
        "an empty selection imports nothing: selected_sections reaches the converter"
    );
    assert!(!hash_recorded(&every, "hash-tml"), "no hash, none recorded");
}

/// Compass: one survey in feet, two legs plus one `#|X#`-excluded leg.
pub(super) fn synthetic_compass_dat(dir: &TempDir) -> PathBuf {
    write(
        dir,
        "CaveA.dat",
        "CaveA\r\n\
         SURVEY NAME: A\r\n\
         SURVEY DATE: 7 10 1979\r\n\
         SURVEY TEAM:\r\n\
         Nobody\r\n\
         DECLINATION: 0.00  FORMAT: DMMDLRUDLADN  CORRECTIONS: 0.00 0.00 0.00\r\n\
         \r\n\
         FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT FLAGS COMMENTS\r\n\
         \r\n\
         A1 A2 10.00 90.00 0.00 1.00 2.00 3.00 4.00\r\n\
         A2 A3 12.50 180.00 -10.00 1.00 2.00 3.00 4.00\r\n\
         A3 A4 5.00 0.00 0.00 1.00 2.00 3.00 4.00 #|X#\r\n\
         \x0c\r\n",
    )
}

fn compass_import(
    path: &Path,
    hash: Option<&str>,
    include_excluded: bool,
) -> (App<MockRuntime>, Result<ImportOutcome, CommandError>) {
    let app = project_app();
    let result = block_on(execute_compass_import(
        app.handle().clone(),
        made_for(&app),
        path.to_path_buf(),
        hash.map(str::to_string),
        include_excluded,
        true,
        false,
    ));
    (app, result)
}

#[test]
fn the_compass_command_honours_include_excluded_and_records_the_file() {
    let dir = TempDir::new().expect("temp dir");
    let path = synthetic_compass_dat(&dir);

    let (default, result) = compass_import(&path, Some("hash-dat"), false);
    let (source, unit_note, found) = receipt_of(&result);
    assert_eq!(source, "Compass");
    assert_eq!(
        unit_note.as_deref(),
        Some("Lengths converted from feet to meters")
    );
    assert_eq!(found, 1);
    assert!(
        hash_recorded(&default, "hash-dat"),
        "the import records its file hash"
    );

    let (with_excluded, result) = compass_import(&path, None, true);
    receipt_of(&result);
    println!(
        "compass shots: default {}, include_excluded {}",
        shots_in(&default),
        shots_in(&with_excluded)
    );
    assert_eq!(shots_in(&default), 2, "the #|X# leg is left out by default");
    assert_eq!(
        shots_in(&with_excluded),
        3,
        "include_excluded reaches the converter"
    );
}

/// Walls: one survey in feet, two legs and a splay off the second station.
pub(super) fn synthetic_walls_srv(dir: &TempDir) -> PathBuf {
    write(
        dir,
        "CaveA.srv",
        "#UNITS feet order=DAV\nA1 A2 10 90 0\nA2 A3 12.5 180 -10\nA2 - 3 0 0\n",
    )
}

fn walls_import(
    path: &Path,
    hash: Option<&str>,
    include_splays: bool,
) -> (App<MockRuntime>, Result<ImportOutcome, CommandError>) {
    let app = project_app();
    let result = block_on(execute_walls_import(
        app.handle().clone(),
        made_for(&app),
        path.to_path_buf(),
        hash.map(str::to_string),
        false,
        include_splays,
        None,
    ));
    (app, result)
}

#[test]
fn the_walls_command_honours_include_splays_and_records_the_file() {
    let dir = TempDir::new().expect("temp dir");
    let path = synthetic_walls_srv(&dir);

    let (default, result) = walls_import(&path, Some("hash-srv"), false);
    let (source, unit_note, found) = receipt_of(&result);
    assert_eq!(source, "Walls");
    assert!(unit_note.is_some(), "a feet file says it converted");
    assert_eq!(found, 1);
    assert!(
        hash_recorded(&default, "hash-srv"),
        "the import records its file hash"
    );

    let (with_splays, result) = walls_import(&path, None, true);
    receipt_of(&result);
    println!(
        "walls shots: default {}, include_splays {}",
        shots_in(&default),
        shots_in(&with_splays)
    );
    assert!(
        shots_in(&with_splays) > shots_in(&default),
        "include_splays reaches the converter"
    );

    let none = project_app();
    let result = block_on(execute_walls_import(
        none.handle().clone(),
        made_for(&none),
        path.clone(),
        None,
        false,
        false,
        Some(Vec::new()),
    ));
    receipt_of(&result);
    assert_eq!(shots_in(&none), 0, "selected_surveys reaches the converter");
}
