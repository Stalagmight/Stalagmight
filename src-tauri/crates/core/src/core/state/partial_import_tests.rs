//! #814: one line that cannot be imported costs the user that line, not
//! the file. Everything else imports, and the receipt names what was lost.
//!
//! Ported to the station graph: a line whose placement cannot be made is a
//! free piece at the origin (0.6.0: `create_line` left it there), a joining
//! placement that works is a junction (0.6.0: a merge), and every partial
//! import is one undo entry that lands bit for bit.

use super::{AppState, VirtualShotMode};
use crate::core::command::{CommandResult, ImportReceipt};
use crate::core::Command;
use crate::models::survey::Project;
use crate::models::{
    GpsCoordinates, ImportClosure, ImportLineConfig, ImportPlacement, LineId, ShotData, StationRef,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};

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

/// A two-shot config. `length` 0.0 makes a line the funnel refuses ("has
/// zero length"), the shape a bad row in any importer produces.
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
        station_external: Vec::new(),
        placement,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }
}

fn new_state() -> AppState {
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Partial".to_string(),
                location: None,
            })
            .success
    );
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

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

fn names(project: &Project) -> Vec<String> {
    project
        .lines()
        .iter()
        .map(|l| l.name.clone().unwrap_or_default())
        .collect()
}

fn receipt(result: &CommandResult) -> &ImportReceipt {
    result.import_receipt.as_ref().expect("a receipt")
}

/// The receipt's per-line entries: what the import lost or could not connect.
fn problems(result: &CommandResult) -> &[String] {
    receipt(result).line_problems.as_deref().unwrap_or_default()
}

/// The line named `name`.
fn line_named(project: &Project, name: &str) -> LineId {
    project
        .lines()
        .iter()
        .find(|l| l.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no line {name}"))
        .id
}

/// The raw horizontal position of slot `(line, index)`.
fn xy(project: &Project, line: LineId, index: usize) -> [f64; 2] {
    let station = project.line(line).unwrap().station(index).unwrap();
    project.raw_positions().xy(station).unwrap()
}

/// Whether the line is a free piece of its own with its first station at
/// the origin: what 0.6.0's `create_line` left when the placement failed.
fn floats_at_origin(project: &Project, line: LineId) -> bool {
    let [x, y] = xy(project, line, 0);
    let start = project.line(line).unwrap().start;
    let topology = project.topology();
    let alone = topology
        .component_of(start)
        .and_then(|c| topology.component(c))
        .is_some_and(|c| c.lines == [line]);
    x.abs() < 1e-9 && y.abs() < 1e-9 && alone
}

/// The v2 text of `p` without what a step may change by design and without
/// the id allocators, which stand higher after an undo (plan 8.1).
fn canonical(p: &Project) -> String {
    let mut p = p.clone();
    p.modified_at = p.created_at;
    p.file_path = None;
    let bytes = stalx_v2::encode(&p, &Extras::default(), Mode::Normal).unwrap();
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with("\"counters\""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn origin_datum(station_index: u32) -> ImportPlacement {
    ImportPlacement::ZeroDatum {
        coordinates: GpsCoordinates {
            latitude: 20.5,
            longitude: -87.3,
            depth_reference: 0.0,
        },
        station_index,
    }
}

fn from_nowhere() -> ImportPlacement {
    ImportPlacement::FromExisting {
        import_station: 0,
        existing_station: StationRef::new(9999, 0),
    }
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
    assert_eq!(names(&project(&state)), ["Good", "Also good"]);
    assert_eq!(result.line_ids.len(), 2);

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
    // The history entry names the lines that landed, never the refused one.
    let stack = state.get_undo_stack();
    assert_eq!(stack.len(), 1, "ONE entry");
    let label = &stack[0].description;
    assert!(
        label.contains("Good") && !label.contains("\"Bad\""),
        "{label}"
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
    let p = project(&state);
    assert_eq!(names(&p), ["Branch"]);
    assert!(p.merge_pairs().is_empty(), "nothing to join onto");
    assert!(floats_at_origin(&p, line_named(&p, "Branch")));
    assert!(p.closures.is_empty());

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

/// A same-batch placement onto a line later in the list cannot be made
/// either (0.6.0 refused the whole import): the line imports on its own.
#[test]
fn a_line_joining_one_later_in_the_list_imports_on_its_own() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(
                0,
                "Early",
                3.0,
                ImportPlacement::FromImporting {
                    import_station: 0,
                    target_line_index: 1,
                    target_station_index: 0,
                },
            ),
            line(1, "Late", 3.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    let p = project(&state);
    assert_eq!(names(&p), ["Early", "Late"]);
    assert!(p.merge_pairs().is_empty());
    assert_eq!(
        problems(&result),
        ["Line \"Early\" was imported on its own: the line it joins was not imported"]
    );
}

#[test]
fn a_failed_placement_keeps_the_line_unconnected() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "Good", 5.0, ImportPlacement::Origin),
            line(1, "Unmerged", 3.0, from_nowhere()),
            line(2, "Unplaced", 3.0, origin_datum(99)),
            line(
                3,
                "Unsnapped",
                3.0,
                ImportPlacement::ManualSnap {
                    align_station_index: 0,
                    snapped_to_station: StationRef::new(9999, 0),
                },
            ),
            line(
                4,
                "Misjoined",
                3.0,
                ImportPlacement::FromImporting {
                    import_station: 0,
                    target_line_index: 0,
                    target_station_index: 99,
                },
            ),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    let p = project(&state);
    assert_eq!(
        names(&p),
        ["Good", "Unmerged", "Unplaced", "Unsnapped", "Misjoined"]
    );
    assert!(p.merge_pairs().is_empty());
    assert!(p.datums.is_empty());
    for name in ["Unmerged", "Unplaced", "Unsnapped", "Misjoined"] {
        assert!(
            floats_at_origin(&p, line_named(&p, name)),
            "{name} is a free piece at the origin"
        );
    }
    assert!(
        receipt(&result).zero_datum_created.is_none(),
        "no datum was set"
    );
    assert_eq!(receipt(&result).shot_count, 10, "every line landed");

    let entries = problems(&result);
    assert_eq!(entries.len(), 4, "{entries:?}");
    for prefix in [
        "Line \"Unmerged\" was imported, but its connection to the existing survey could not \
         be made: Station 0 of line 9999 does not exist",
        "Line \"Unplaced\" was imported, but its zero datum could not be set: Station 99 of line",
        "Line \"Unsnapped\" was imported, but it could not be snapped to its station: Station 0 \
         of line 9999 does not exist",
        "Line \"Misjoined\" was imported, but its connection to the line it joins could not be \
         made: Station 99 of line",
    ] {
        assert!(
            entries.iter().any(|w| w.starts_with(prefix)),
            "{prefix}: {entries:?}"
        );
    }
}

#[test]
fn every_line_refused_fails_and_names_each() {
    let state = new_state();
    let before = project(&state);
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
    assert!(project(&state).lines().is_empty());
    assert_eq!(canonical(&project(&state)), canonical(&before));
    assert!(state.get_undo_stack().is_empty(), "no entry");
}

/// More refused lines than the message names: the first five, then a count.
#[test]
fn the_nothing_imported_message_names_five_and_counts_the_rest() {
    let state = new_state();
    let lines = (0..8)
        .map(|i| line(i, &format!("Bad {i}"), 0.0, ImportPlacement::Origin))
        .collect();
    let result = import(&state, lines, Vec::new());
    let error = result.error.unwrap_or_default();
    assert!(error.contains("\"Bad 4\""), "{error}");
    assert!(!error.contains("\"Bad 5\""), "{error}");
    assert!(
        error.ends_with("(3 more lines were not imported.)"),
        "{error}"
    );
}

#[test]
fn one_undo_removes_a_partial_import_and_redo_restores_it() {
    let state = new_state();
    let before = project(&state);
    let result = import(
        &state,
        vec![
            line(0, "Good", 5.0, ImportPlacement::Origin),
            line(1, "Bad", 0.0, ImportPlacement::Origin),
            line(2, "Unmerged", 3.0, from_nowhere()),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    let after = project(&state);
    assert_eq!(state.get_undo_stack().len(), 1, "ONE entry");

    assert!(state.undo().success);
    assert!(project(&state).lines().is_empty());
    assert_eq!(
        canonical(&project(&state)),
        canonical(&before),
        "undo is exact"
    );
    assert!(state.redo().success);
    assert_eq!(names(&project(&state)), ["Good", "Unmerged"]);
    assert_eq!(
        canonical(&project(&state)),
        canonical(&after),
        "redo is exact"
    );
}

#[test]
fn a_refused_line_names_its_shot_by_its_line_and_stations() {
    // The refused line never gets a line id, so a shot named by an id
    // ("Shot <id>:2") would point at whichever line landed next. The shot is
    // named by the line's own name and its station positions instead.
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
    let p = project(&state);
    assert_eq!(names(&p), ["Unmoved"]);
    assert!(
        floats_at_origin(&p, line_named(&p, "Unmoved")),
        "a refused move leaves the line where it was created"
    );
    let entries = problems(&result);
    assert!(
        entries.iter().any(|w| w.starts_with(
            "Line \"Unmoved\" was imported, but it could not be moved to where it was placed: \
             Move offset must be finite"
        )),
        "{entries:?}"
    );
}

/// The dedup hashes are one per SELECTED line, not per landed one, so a
/// partial import records none: marking a refused line "already imported"
/// would deselect it on the next import of the same file. The file hash is
/// kept: part of the file landed.
#[test]
fn a_partial_import_records_no_line_hashes_but_the_file_hash() {
    let state = new_state();
    let result = state.execute_import_lines(
        vec![
            line(0, "Good", 5.0, ImportPlacement::Origin),
            line(1, "Bad", 0.0, ImportPlacement::Origin),
        ],
        Vec::new(),
        Some("file".to_string()),
        vec!["h0".to_string(), "h1".to_string()],
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "{:?}", result.error);
    let tracked = state
        .check_line_hashes(&["h0".to_string(), "h1".to_string()])
        .unwrap();
    assert_eq!(tracked, [false, false]);
    assert_eq!(project(&state).imported_file_hashes, ["file"]);
}

/// An unconnected line is not a lost one: the line hashes are recorded when
/// every line landed, connected or not.
#[test]
fn an_unconnected_line_still_records_the_line_hashes() {
    let state = new_state();
    let result = state.execute_import_lines(
        vec![
            line(0, "Good", 5.0, ImportPlacement::Origin),
            line(1, "Unmerged", 3.0, from_nowhere()),
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
    assert_eq!(tracked, [true, true]);
}

/// The DMP wizard's route keeps the whole import or nothing: it maps the
/// returned ids onto its configs by position and sends overlap edits tied to
/// particular lines, so a skipped or unconnected line must roll back.
#[test]
fn the_all_or_nothing_route_rolls_back_on_a_refused_or_unconnected_line() {
    for bad in [
        line(1, "Bad", 0.0, ImportPlacement::Origin),
        line(1, "Bad", 3.0, from_nowhere()),
        line(
            1,
            "Bad",
            3.0,
            ImportPlacement::FromImporting {
                import_station: 0,
                target_line_index: 7,
                target_station_index: 0,
            },
        ),
    ] {
        let state = new_state();
        let before = project(&state);
        let result = state.execute_import_lines_all_or_nothing(
            vec![line(0, "Good", 5.0, ImportPlacement::Origin), bad],
            Some("file".to_string()),
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
        assert!(project(&state).lines().is_empty());
        assert_eq!(canonical(&project(&state)), canonical(&before));
        assert_eq!(
            state
                .check_line_hashes(&["h0".to_string(), "h1".to_string()])
                .unwrap(),
            [false, false]
        );
        assert!(state.get_undo_stack().is_empty(), "no entry");
    }
}

/// The all-or-nothing route imports a sound file exactly as the other one
/// does.
#[test]
fn the_all_or_nothing_route_imports_a_sound_file() {
    let state = new_state();
    let result = state.execute_import_lines_all_or_nothing(
        vec![
            line(0, "One", 5.0, origin_datum(0)),
            line(
                1,
                "Two",
                3.0,
                ImportPlacement::FromImporting {
                    import_station: 0,
                    target_line_index: 0,
                    target_station_index: 2,
                },
            ),
        ],
        None,
        vec!["h0".to_string(), "h1".to_string()],
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "{:?}", result.error);
    assert!(result.import_receipt.unwrap().line_problems.is_none());
    let p = project(&state);
    assert_eq!(names(&p), ["One", "Two"]);
    assert_eq!(p.merge_pairs().len(), 1, "a junction");
}

fn not_connected(state: &AppState) -> Vec<crate::models::ProjectWarning> {
    let project = project(state);
    state
        .compute_project_warnings(&project)
        .into_iter()
        .filter(|w| w.code == crate::models::LINE_NOT_CONNECTED)
        .collect()
}

/// A line whose import connection failed stays findable after the receipt
/// is closed: the Warnings tab lists it, on its first shot, as advisory.
#[test]
fn an_unconnected_line_is_listed_in_the_warnings() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "Placed", 5.0, origin_datum(0)),
            line(1, "Unmerged", 3.0, from_nowhere()),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    let p = project(&state);
    let placed = line_named(&p, "Placed");
    let unmerged = line_named(&p, "Unmerged");

    let warnings = not_connected(&state);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let w = &warnings[0];
    assert_eq!((w.line_id, w.shot_index), (unmerged, 0));
    assert_eq!(
        w.shot_uuid.as_deref(),
        Some(p.line(unmerged).unwrap().shots[0].uuid.as_str())
    );
    assert_eq!(
        w.severity,
        crate::core::WarningSeverity::Warning,
        "it can be ignored"
    );
    assert_eq!(
        w.message,
        "Line \"Unmerged\" is not connected to the rest of the survey and has no zero datum, \
         so nothing places it: merge a station onto the survey or give it a zero datum"
    );
    // The open-project entry point (the IPC's) lists it too.
    assert_eq!(
        state
            .open_project_warnings()
            .iter()
            .filter(|w| w.code == crate::models::LINE_NOT_CONNECTED)
            .count(),
        1
    );

    // Merging it onto the survey is the fix, and the warning goes with it.
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(unmerged, 0),
        station_b: StationRef::new(placed, 2),
    });
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

/// A piece of several joined lines is listed once, on its first line.
#[test]
fn a_merged_floating_piece_is_listed_once() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "Placed", 5.0, origin_datum(0)),
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
    assert_eq!(warnings[0].line_id, line_named(&project(&state), "Loose"));
    assert!(
        warnings[0]
            .message
            .starts_with("Line \"Loose\" and 1 line merged with it are not connected"),
        "{}",
        warnings[0].message
    );
    assert!(warnings[0]
        .message
        .contains("have no zero datum, so nothing places them"));
}

/// Each floating piece is listed once, in line order; a zero datum on one
/// places it and its warning goes; the last datum's removal floats every
/// piece, and then none is the odd one out.
#[test]
fn each_floating_piece_is_listed_until_something_places_it() {
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "Placed", 5.0, origin_datum(0)),
            line(1, "Loose A", 3.0, ImportPlacement::Origin),
            line(2, "Loose B", 3.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    let p = project(&state);
    let (a, b) = (line_named(&p, "Loose A"), line_named(&p, "Loose B"));
    let listed = |state: &AppState| -> Vec<LineId> {
        not_connected(state).iter().map(|w| w.line_id).collect()
    };
    assert_eq!(listed(&state), [a, b]);

    let datum = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(b, 0),
        coordinates: GpsCoordinates {
            latitude: 20.6,
            longitude: -87.3,
            depth_reference: 0.0,
        },
        accuracy_m: None,
        entrance: None,
    });
    assert!(datum.success, "{:?}", datum.error);
    assert_eq!(listed(&state), [a], "a datum places its piece");
    assert!(state.undo().success);
    assert_eq!(listed(&state), [a, b], "and its undo floats it again");
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
            line(0, "Placed", 5.0, origin_datum(0)),
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
/// own text (`Shot <name>: 1 -> 2 has zero length`), so its receipt entry, and
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

/// The receipt bounds the per-line entries like the warnings (#814): a
/// hostile file can refuse one line per entry.
#[test]
fn the_line_problems_are_capped_in_the_receipt() {
    let state = new_state();
    let mut lines: Vec<ImportLineConfig> = (0..60)
        .map(|i| line(i, &format!("Bad {i}"), 0.0, ImportPlacement::Origin))
        .collect();
    lines.push(line(60, "Good", 5.0, ImportPlacement::Origin));
    let result = import(&state, lines, Vec::new());
    assert!(result.success, "{:?}", result.error);
    let entries = problems(&result);
    assert_eq!(entries.len(), 51, "50 entries and the count of the rest");
    assert_eq!(entries.last().unwrap(), "... and 10 more lines");
}

/// The unplaced-piece warning's code is stored with every Ignore, and master
/// (0.5.x) builds store it as `SECTION_NOT_CONNECTED`, the spelling from before
/// the Section -> Line rename; the frontend matches the same string. An Ignore
/// stored under that literal code must still resolve the warning here, and
/// ride the save.
#[test]
fn the_not_connected_code_keeps_masters_spelling() {
    assert_eq!(crate::models::LINE_NOT_CONNECTED, "SECTION_NOT_CONNECTED");

    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("cave.stalx");
    let state = new_state();
    let result = import(
        &state,
        vec![
            line(0, "Placed", 5.0, origin_datum(0)),
            line(1, "Loose", 3.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone()),
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

    // The Ignore rides the save and is read back on open.
    assert!(state.execute(Command::SaveProject { path: None }).success);
    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path });
    assert!(opened.success, "{:?}", opened.error);
    let warnings = not_connected(&reopened);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].is_resolved,
        "the Ignore survives save and reopen"
    );
}

/// A distance past the measurement bound (a unit or typing mistake in the
/// file: a thousand kilometres and more) costs only its line, in words, as
/// any other refused line does (#814): it never refuses the whole file.
#[test]
fn a_line_with_a_distance_past_the_bound_costs_only_itself() {
    let state = new_state();
    let mut deep = line(2, "Deep", 3.0, ImportPlacement::Origin);
    deep.shots[1].depth_out = -1_500_000.0;
    let mut wide = line(3, "Wide", 3.0, ImportPlacement::Origin);
    wide.shots[0].left = Some(2_500_000.0);
    let result = import(
        &state,
        vec![
            line(0, "Good", 5.0, ImportPlacement::Origin),
            line(1, "Long", 2_000_000.0, ImportPlacement::Origin),
            deep,
            wide,
            line(4, "Also good", 3.0, ImportPlacement::Origin),
        ],
        Vec::new(),
    );
    assert!(result.success, "{:?}", result.error);
    assert_eq!(names(&project(&state)), ["Good", "Also good"]);
    assert_eq!(
        problems(&result),
        [
            "Line \"Long\" was not imported: A shot's length of 2000000 m is beyond the \
             1000000 m a survey measurement may be",
            "Line \"Deep\" was not imported: A shot's depth out of -1500000 m is beyond the \
             1000000 m a survey measurement may be",
            "Line \"Wide\" was not imported: A shot's left of 2500000 m is beyond the \
             1000000 m a survey measurement may be",
        ]
    );
}
