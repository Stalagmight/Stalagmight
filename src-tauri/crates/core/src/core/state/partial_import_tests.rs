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

/// The receipt's per-section lines: what the import lost or could not connect.
fn problems(result: &CommandResult) -> &[String] {
    receipt(result)
        .section_problems
        .as_deref()
        .unwrap_or_default()
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
    // Two shots per section: the refused section's shots never landed.
    assert_eq!(receipt.shot_count, 4);
    assert_eq!(
        problems(&result),
        ["Section \"Bad\" was not imported: Shot 2 has zero length"],
        "the refused section gets its own receipt line, apart from the warnings"
    );
    assert!(
        !receipt.warnings.iter().any(|w| w.contains("\"Bad\"")),
        "{:?}",
        receipt.warnings
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

    assert_eq!(
        problems(&result),
        [
            "Section \"Bad\" was not imported: Shot 2 has zero length",
            "Section \"Branch\" was imported on its own: the section it joins was not imported",
        ]
    );
    let warnings = &receipt(&result).warnings;
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

    let warnings = problems(&result);
    assert!(
        warnings.iter().any(|w| w.starts_with(
            "Section \"Unmerged\" was imported, but its connection to the existing survey could not be made"
        )),
        "{warnings:?}"
    );
    assert!(
        warnings.iter().any(|w| w
            .starts_with("Section \"Unplaced\" was imported, but its zero datum could not be set")),
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

#[test]
fn a_refused_section_names_its_shot_by_position_only() {
    // The refused section never gets the id `create_section` would have given
    // it, so "Shot <id>:2" pointed at whichever section landed next.
    let state = new_state();
    let result = import(
        &state,
        vec![
            section(0, "Good", 5.0, ImportPlacement::Origin),
            section(1, "Bad", 0.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert_eq!(
        problems(&result)[0],
        "Section \"Bad\" was not imported: Shot 2 has zero length"
    );
}

#[test]
fn a_move_that_cannot_be_made_keeps_the_section_where_it_was_created() {
    let state = new_state();
    let result = import(
        &state,
        vec![section(
            0,
            "Unmoved",
            3.0,
            ImportPlacement::Manual {
                align_station_index: 0,
                offset: Some(crate::models::Coordinates {
                    x: f64::NAN,
                    y: 0.0,
                    z: 0.0,
                }),
            },
        )],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    let project = state.get_project_state().unwrap();
    assert_eq!(names(&project), ["Unmoved"]);
    assert!(
        !project
            .section_offsets
            .contains_key(&project.sections[0].id),
        "a refused move leaves the section where it was created"
    );
    let warnings = problems(&result);
    assert!(
        warnings.iter().any(|w| w.starts_with(
            "Section \"Unmoved\" was imported, but it could not be moved to where it was placed"
        )),
        "{warnings:?}"
    );
}

/// The dedup hashes are one per SELECTED section, not per landed one, so a
/// partial import records none: marking a refused section "already imported"
/// would deselect it on the next import of the same file.
#[test]
fn a_partial_import_records_no_section_hashes() {
    let state = new_state();
    let result = state.execute_import_sections(
        vec![
            section(0, "Good", 5.0, ImportPlacement::Origin),
            section(1, "Bad", 0.0, ImportPlacement::Origin),
        ],
        Vec::new(),
        None,
        vec!["h0".to_string(), "h1".to_string()],
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "{:?}", result.error);
    let tracked = state
        .check_section_hashes(&["h0".to_string(), "h1".to_string()])
        .unwrap();
    assert_eq!(tracked, [false, false]);
}

/// The DMP wizard's route keeps the whole import or nothing: it maps the
/// returned ids onto its configs by position and sends overlap edits tied to
/// particular sections, so a skipped or unconnected section must roll back.
#[test]
fn the_all_or_nothing_route_rolls_back_on_a_refused_or_unconnected_section() {
    for bad in [
        section(1, "Bad", 0.0, ImportPlacement::Origin),
        section(
            1,
            "Bad",
            3.0,
            ImportPlacement::FromExisting {
                import_station: 0,
                existing_station: StationRef::new(9999, 0),
            },
        ),
    ] {
        let state = new_state();
        let result = state.execute_import_sections_all_or_nothing(
            vec![section(0, "Good", 5.0, ImportPlacement::Origin), bad],
            None,
            vec!["h0".to_string(), "h1".to_string()],
            Vec::new(),
            VirtualShotMode::Auto,
        );
        assert!(!result.success);
        let error = result.error.unwrap_or_default();
        assert!(
            error.starts_with("The import was undone and nothing was kept")
                && error.contains("Section \"Bad\""),
            "{error}"
        );
        assert!(state.get_project_state().unwrap().sections.is_empty());
        assert_eq!(
            state
                .check_section_hashes(&["h0".to_string(), "h1".to_string()])
                .unwrap(),
            [false, false]
        );
    }
}

fn not_connected(state: &AppState) -> Vec<crate::models::ProjectWarning> {
    let project = state.get_project_state().unwrap();
    state
        .compute_project_warnings(&project, None)
        .into_iter()
        .filter(|w| w.code == crate::models::SECTION_NOT_CONNECTED)
        .collect()
}

fn datum_at_origin() -> ImportPlacement {
    ImportPlacement::ZeroDatum {
        coordinates: crate::models::GpsCoordinates {
            latitude: 20.5,
            longitude: -87.3,
            depth_reference: 0.0,
        },
        station_index: 0,
    }
}

/// A section whose import connection failed stays findable after the receipt
/// is closed: the Warnings tab lists it, on its first shot, as advisory.
#[test]
fn an_unconnected_section_is_listed_in_the_warnings() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            section(0, "Placed", 5.0, datum_at_origin()),
            section(
                1,
                "Unmerged",
                3.0,
                ImportPlacement::FromExisting {
                    import_station: 0,
                    existing_station: StationRef::new(9999, 0),
                },
            ),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    let project = state.get_project_state().unwrap();
    let unmerged = project.sections[1].id;

    let warnings = not_connected(&state);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let w = &warnings[0];
    assert_eq!((w.section_id, w.shot_index), (unmerged, 0));
    assert_eq!(
        w.severity,
        crate::core::WarningSeverity::Warning,
        "it can be ignored"
    );
    assert!(
        w.message
            .starts_with("Section \"Unmerged\" is not connected to the rest of the survey"),
        "{}",
        w.message
    );

    // Merging it onto the survey is the fix, and the warning goes with it.
    let merged = state.merge_stations(
        StationRef::new(unmerged, 0),
        StationRef::new(project.sections[0].id, 2),
    );
    assert!(merged.success, "{:?}", merged.error);
    assert!(not_connected(&state).is_empty());
}

/// Only an odd one out is flagged: in a project with no zero datum anywhere,
/// every piece floats and there is nothing to connect them to.
#[test]
fn no_unconnected_warning_without_any_zero_datum() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            section(0, "One", 5.0, ImportPlacement::Origin),
            section(1, "Two", 3.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    assert!(not_connected(&state).is_empty());
}

/// A piece of several merged sections is listed once, on its first section.
#[test]
fn a_merged_floating_piece_is_listed_once() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            section(0, "Placed", 5.0, datum_at_origin()),
            section(1, "Loose", 3.0, ImportPlacement::Origin),
            section(
                2,
                "Loose branch",
                3.0,
                ImportPlacement::FromImporting {
                    import_station: 0,
                    target_section_index: 1,
                    target_station_index: 2,
                },
            ),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    let warnings = not_connected(&state);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0]
            .message
            .starts_with("Section \"Loose\" and 1 section merged with it are not connected"),
        "{}",
        warnings[0].message
    );
}

/// The section name in the warning is file-controlled text: filtered and
/// bounded like every receipt string.
#[test]
fn the_unconnected_warning_sanitises_the_section_name() {
    let state = new_state();
    let hostile = format!("Bidi\u{202e}name{}", "x".repeat(10_000));
    let result = import(
        &state,
        vec![
            section(0, "Placed", 5.0, datum_at_origin()),
            section(1, &hostile, 3.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    let warnings = not_connected(&state);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let message = &warnings[0].message;
    assert!(!message.contains('\u{202e}'), "bidi override survived");
    assert!(
        message.chars().count() < 1_000,
        "{} chars",
        message.chars().count()
    );
}
