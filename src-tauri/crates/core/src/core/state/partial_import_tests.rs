//! #814: one section that cannot be imported costs the user that section, not
//! the file. Everything else imports, and the receipt names what was lost.

use super::{AppState, VirtualShotMode};
use crate::core::command::{CommandResult, ImportReceipt};
use crate::core::Command;
use crate::models::{
    ImportClosure, ImportPlacement, ImportSectionConfig, Project, ShotData, StationRef,
};

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

/// A one-section config. `length` 0.0 makes a section `create_section` refuses
/// ("has zero length"), the shape a bad row in any importer produces.
fn section(index: u32, name: &str, length: f64, placement: ImportPlacement) -> ImportSectionConfig {
    ImportSectionConfig {
        reverse: false,
        section_index: index,
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![shot(10.0), shot(length)],
        station_comments: Vec::new(),
        station_names: Vec::new(),
        placement,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }
}

fn new_state() -> AppState {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Partial".to_string(),
        location: None,
    });
    state
}

fn import(
    state: &AppState,
    sections: Vec<ImportSectionConfig>,
    closures: Vec<ImportClosure>,
) -> CommandResult {
    state.execute_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    )
}

fn names(project: &Project) -> Vec<String> {
    project
        .sections
        .iter()
        .map(|s| s.name.clone().unwrap_or_default())
        .collect()
}

fn receipt(result: &CommandResult) -> &ImportReceipt {
    result.import_receipt.as_ref().expect("a receipt")
}

#[test]
fn a_refused_section_costs_only_itself() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            section(0, "Good", 5.0, ImportPlacement::Origin),
            section(1, "Bad", 0.0, ImportPlacement::Origin),
            section(2, "Also good", 3.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        names(&state.get_project_state().unwrap()),
        ["Good", "Also good"]
    );

    let receipt = receipt(&result);
    assert_eq!(receipt.sections_imported, 2);
    let line = &receipt.warnings[0];
    assert!(
        line.starts_with("Section \"Bad\" was not imported: ") && line.contains("zero length"),
        "the receipt must lead with the refused section and why: {line}"
    );
}

#[test]
fn a_section_joining_a_refused_one_imports_on_its_own() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            section(0, "Bad", 0.0, ImportPlacement::Origin),
            section(
                1,
                "Branch",
                4.0,
                ImportPlacement::FromImporting {
                    import_station: 0,
                    target_section_index: 0,
                    target_station_index: 1,
                },
            ),
        ],
        // A loop closure between the two: its far end was never created.
        vec![ImportClosure {
            from_section_index: 0,
            from_station_index: 2,
            to_section_index: 1,
            to_station_index: 2,
        }],
    );
    assert!(result.success, "{:?}", result.error);
    let project = state.get_project_state().unwrap();
    assert_eq!(names(&project), ["Branch"]);
    assert!(project.station_merges.is_empty(), "nothing to merge onto");

    let warnings = &receipt(&result).warnings;
    assert!(
        warnings.iter().any(|w| w
            == "Section \"Branch\" was imported but not connected: the section it joins was not imported"),
        "{warnings:?}"
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("1 loop closure could not be created")),
        "{warnings:?}"
    );
}

#[test]
fn a_failed_placement_keeps_the_section_unconnected() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            section(0, "Good", 5.0, ImportPlacement::Origin),
            section(
                1,
                "Unmerged",
                3.0,
                ImportPlacement::FromExisting {
                    import_station: 0,
                    existing_station: StationRef::new(9999, 0),
                },
            ),
            section(
                2,
                "Unplaced",
                3.0,
                ImportPlacement::ZeroDatum {
                    coordinates: crate::models::GpsCoordinates {
                        latitude: 20.5,
                        longitude: -87.3,
                        depth_reference: 0.0,
                    },
                    station_index: 99,
                },
            ),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    let project = state.get_project_state().unwrap();
    assert_eq!(names(&project), ["Good", "Unmerged", "Unplaced"]);
    assert!(project.station_merges.is_empty());
    assert!(project.zero_datums.is_empty());

    let warnings = &receipt(&result).warnings;
    assert!(
        warnings.iter().any(|w| w.starts_with(
            "Section \"Unmerged\" was imported but not connected to the existing survey"
        )),
        "{warnings:?}"
    );
    assert!(
        warnings
            .iter()
            .any(|w| w
                .starts_with("Section \"Unplaced\" was imported but not placed at its zero datum")),
        "{warnings:?}"
    );
}

#[test]
fn every_section_refused_fails_and_names_each() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            section(0, "First bad", 0.0, ImportPlacement::Origin),
            section(1, "Second bad", -1.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert!(!result.success);
    let error = result.error.unwrap_or_default();
    assert!(error.starts_with("Nothing was imported"), "{error}");
    assert!(
        error.contains("\"First bad\"") && error.contains("\"Second bad\""),
        "{error}"
    );
    assert!(state.get_project_state().unwrap().sections.is_empty());
}

#[test]
fn one_undo_removes_a_partial_import_and_redo_restores_it() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            section(0, "Good", 5.0, ImportPlacement::Origin),
            section(1, "Bad", 0.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);

    assert!(state.undo().success);
    assert!(state.get_project_state().unwrap().sections.is_empty());
    assert!(state.redo().success);
    assert_eq!(names(&state.get_project_state().unwrap()), ["Good"]);
}
