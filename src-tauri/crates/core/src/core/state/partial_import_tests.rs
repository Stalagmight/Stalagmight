//! #814: one line that cannot be imported costs the user that line, not
//! the file. Everything else imports, and the receipt names what was lost.

use super::{AppState, VirtualShotMode};
use crate::core::command::{CommandResult, ImportReceipt};
use crate::core::Command;
use crate::models::{
    ImportClosure, ImportLineConfig, ImportPlacement, Project, ShotData, StationRef,
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

/// A one-line config. `length` 0.0 makes a line `create_line` refuses
/// ("has zero length"), the shape a bad row in any importer produces.
fn line(index: u32, name: &str, length: f64, placement: ImportPlacement) -> ImportLineConfig {
    ImportLineConfig {
        cave_section: None,
        reverse: false,
        line_index: index,
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
    lines: Vec<ImportLineConfig>,
    closures: Vec<ImportClosure>,
) -> CommandResult {
    state.execute_import_lines(
        lines,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    )
}

fn names(project: &Project) -> Vec<String> {
    project
        .lines
        .iter()
        .map(|s| s.name.clone().unwrap_or_default())
        .collect()
}

fn receipt(result: &CommandResult) -> &ImportReceipt {
    result.import_receipt.as_ref().expect("a receipt")
}

/// The receipt's per-line entries: what the import lost or could not connect.
fn problems(result: &CommandResult) -> &[String] {
    receipt(result).line_problems.as_deref().unwrap_or_default()
}

#[test]
fn a_refused_line_costs_only_itself() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "Good", 5.0, ImportPlacement::Origin),
            line(1, "Bad", 0.0, ImportPlacement::Origin),
            line(2, "Also good", 3.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        names(&state.get_project_state().unwrap()),
        ["Good", "Also good"]
    );

    let receipt = receipt(&result);
    assert_eq!(receipt.lines_imported, 2);
    // Two shots per line: the refused line's shots never landed.
    assert_eq!(receipt.shot_count, 4);
    assert_eq!(
        problems(&result),
        ["Line \"Bad\" was not imported: Shot Bad: 1 \u{2192} 2 has zero length"],
        "the refused line gets its own receipt entry, apart from the warnings"
    );
    assert!(
        !receipt.warnings.iter().any(|w| w.contains("\"Bad\"")),
        "{:?}",
        receipt.warnings
    );
}

#[test]
fn a_line_joining_a_refused_one_imports_on_its_own() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "Bad", 0.0, ImportPlacement::Origin),
            line(
                1,
                "Branch",
                4.0,
                ImportPlacement::FromImporting {
                    import_station: 0,
                    target_line_index: 0,
                    target_station_index: 1,
                },
            ),
        ],
        // A loop closure between the two: its far end was never created.
        vec![ImportClosure {
            from_line_index: 0,
            from_station_index: 2,
            to_line_index: 1,
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
            "Line \"Bad\" was not imported: Shot Bad: 1 \u{2192} 2 has zero length",
            "Line \"Branch\" was imported on its own: the line it joins was not imported",
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
fn a_failed_placement_keeps_the_line_unconnected() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "Good", 5.0, ImportPlacement::Origin),
            line(
                1,
                "Unmerged",
                3.0,
                ImportPlacement::FromExisting {
                    import_station: 0,
                    existing_station: StationRef::new(9999, 0),
                },
            ),
            line(
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
            "Line \"Unmerged\" was imported, but its connection to the existing survey could not be made"
        )),
        "{warnings:?}"
    );
    assert!(
        warnings.iter().any(|w| w
            .starts_with("Line \"Unplaced\" was imported, but its zero datum could not be set")),
        "{warnings:?}"
    );
}

#[test]
fn every_line_refused_fails_and_names_each() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "First bad", 0.0, ImportPlacement::Origin),
            line(1, "Second bad", -1.0, ImportPlacement::Origin),
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
    assert!(state.get_project_state().unwrap().lines.is_empty());
}

#[test]
fn one_undo_removes_a_partial_import_and_redo_restores_it() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "Good", 5.0, ImportPlacement::Origin),
            line(1, "Bad", 0.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);

    assert!(state.undo().success);
    assert!(state.get_project_state().unwrap().lines.is_empty());
    assert!(state.redo().success);
    assert_eq!(names(&state.get_project_state().unwrap()), ["Good"]);
}

#[test]
fn a_refused_line_names_its_shot_by_its_line_and_stations() {
    // The refused line never gets the id `create_line` would have given
    // it, so a shot named by that id ("Shot <id>:2") would point at whichever
    // line landed next. The shot is named by the line's own name and its
    // station positions instead.
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "Good", 5.0, ImportPlacement::Origin),
            line(1, "Bad", 0.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert_eq!(
        problems(&result)[0],
        "Line \"Bad\" was not imported: Shot Bad: 1 \u{2192} 2 has zero length"
    );
}

#[test]
fn a_move_that_cannot_be_made_keeps_the_line_where_it_was_created() {
    let state = new_state();
    let result = import(
        &state,
        vec![line(
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
        !project.line_offsets.contains_key(&project.lines[0].id),
        "a refused move leaves the line where it was created"
    );
    let warnings = problems(&result);
    assert!(
        warnings.iter().any(|w| w.starts_with(
            "Line \"Unmoved\" was imported, but it could not be moved to where it was placed"
        )),
        "{warnings:?}"
    );
}

/// The dedup hashes are one per SELECTED line, not per landed one, so a
/// partial import records none: marking a refused line "already imported"
/// would deselect it on the next import of the same file.
#[test]
fn a_partial_import_records_no_line_hashes() {
    let state = new_state();
    let result = state.execute_import_lines(
        vec![
            line(0, "Good", 5.0, ImportPlacement::Origin),
            line(1, "Bad", 0.0, ImportPlacement::Origin),
        ],
        Vec::new(),
        None,
        vec!["h0".to_string(), "h1".to_string()],
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "{:?}", result.error);
    let tracked = state
        .check_line_hashes(&["h0".to_string(), "h1".to_string()])
        .unwrap();
    assert_eq!(tracked, [false, false]);
}

/// The DMP wizard's route keeps the whole import or nothing: it maps the
/// returned ids onto its configs by position and sends overlap edits tied to
/// particular lines, so a skipped or unconnected line must roll back.
#[test]
fn the_all_or_nothing_route_rolls_back_on_a_refused_or_unconnected_line() {
    for bad in [
        line(1, "Bad", 0.0, ImportPlacement::Origin),
        line(
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
        let result = state.execute_import_lines_all_or_nothing(
            vec![line(0, "Good", 5.0, ImportPlacement::Origin), bad],
            None,
            vec!["h0".to_string(), "h1".to_string()],
            Vec::new(),
            VirtualShotMode::Auto,
        );
        assert!(!result.success);
        let error = result.error.unwrap_or_default();
        assert!(
            error.starts_with("The import was undone and nothing was kept")
                && error.contains("Line \"Bad\""),
            "{error}"
        );
        assert!(state.get_project_state().unwrap().lines.is_empty());
        assert_eq!(
            state
                .check_line_hashes(&["h0".to_string(), "h1".to_string()])
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
        .filter(|w| w.code == crate::models::LINE_NOT_CONNECTED)
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

/// A line whose import connection failed stays findable after the receipt
/// is closed: the Warnings tab lists it, on its first shot, as advisory.
#[test]
fn an_unconnected_line_is_listed_in_the_warnings() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "Placed", 5.0, datum_at_origin()),
            line(
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
    let unmerged = project.lines[1].id;

    let warnings = not_connected(&state);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let w = &warnings[0];
    assert_eq!((w.line_id, w.shot_index), (unmerged, 0));
    assert_eq!(
        w.severity,
        crate::core::WarningSeverity::Warning,
        "it can be ignored"
    );
    assert!(
        w.message
            .starts_with("Line \"Unmerged\" is not connected to the rest of the survey"),
        "{}",
        w.message
    );

    // Merging it onto the survey is the fix, and the warning goes with it.
    let merged = state.merge_stations(
        StationRef::new(unmerged, 0),
        StationRef::new(project.lines[0].id, 2),
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
            line(0, "One", 5.0, ImportPlacement::Origin),
            line(1, "Two", 3.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    assert!(not_connected(&state).is_empty());
}

/// A piece of several merged lines is listed once, on its first line.
#[test]
fn a_merged_floating_piece_is_listed_once() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "Placed", 5.0, datum_at_origin()),
            line(1, "Loose", 3.0, ImportPlacement::Origin),
            line(
                2,
                "Loose branch",
                3.0,
                ImportPlacement::FromImporting {
                    import_station: 0,
                    target_line_index: 1,
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
            .starts_with("Line \"Loose\" and 1 line merged with it are not connected"),
        "{}",
        warnings[0].message
    );
}

/// The line name in the warning is file-controlled text: filtered and
/// bounded like every receipt string.
#[test]
fn the_unconnected_warning_sanitises_the_line_name() {
    let state = new_state();
    let hostile = format!("Bidi\u{202e}name{}", "x".repeat(10_000));
    let result = import(
        &state,
        vec![
            line(0, "Placed", 5.0, datum_at_origin()),
            line(1, &hostile, 3.0, ImportPlacement::Origin),
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

/// A refused line's reason names the line and its stations from the file's
/// own text (`Shot <name>: 1 → 2 has zero length`), so its receipt entry, and
/// the failure message an all-refused import builds from the entries, filter
/// that text like the line's name: no bidi override reaches the dialog.
#[test]
fn a_refused_lines_reason_is_filtered_like_its_name() {
    let hostile = "Bad\u{202e}name";

    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "Good", 5.0, ImportPlacement::Origin),
            line(1, hostile, 0.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    let entry = &problems(&result)[0];
    assert!(entry.contains("has zero length"), "{entry:?}");
    assert!(
        !entry.contains('\u{202e}'),
        "bidi override survived: {entry:?}"
    );

    let state = new_state();
    let result = import(
        &state,
        vec![line(0, hostile, 0.0, ImportPlacement::Origin)],
        Vec::new(),
    );
    assert!(!result.success);
    let error = result.error.unwrap_or_default();
    assert!(error.starts_with("Nothing was imported"), "{error:?}");
    assert!(
        !error.contains('\u{202e}'),
        "bidi override survived: {error:?}"
    );
}

/// The unplaced-piece warning's code is stored with every Ignore, and master
/// (0.5.x) builds store it as `SECTION_NOT_CONNECTED`, the spelling from before
/// the Section -> Line rename; the frontend matches the same string. An Ignore
/// stored under that literal code must still resolve the warning here.
#[test]
fn the_not_connected_code_keeps_masters_spelling() {
    assert_eq!(crate::models::LINE_NOT_CONNECTED, "SECTION_NOT_CONNECTED");

    let temp = tempfile::TempDir::new().unwrap();
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "Placed", 5.0, datum_at_origin()),
            line(1, "Loose", 3.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(temp.path().join("cave.stalx")),
            })
            .success
    );
    let warnings = not_connected(&state);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(!warnings[0].is_resolved);

    state
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            line_id: warnings[0].line_id,
            shot_id: warnings[0].shot_id,
            warning_code: "SECTION_NOT_CONNECTED".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: warnings[0].shot_uuid.clone(),
        })
        .unwrap();
    let warnings = not_connected(&state);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].is_resolved,
        "an Ignore stored under the literal code resolves the warning"
    );
}
