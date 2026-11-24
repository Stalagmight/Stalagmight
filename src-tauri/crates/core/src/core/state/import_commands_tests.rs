//! The import funnel, create-and-merge and the import tracking on the
//! station graph (C7). Ported from `tests.rs` ("Coverage gap:
//! execute_create_and_merge", "execute_import_lines", "Import hash tracking
//! tests", the TML profiler harness) and extended with what the station
//! graph changes: a placement is a junction, one undo entry per import that
//! lands bit for bit, the receipt, closures, uuids, Sections, overlap
//! updates and the placement suggestions.

use std::path::Path;

use tempfile::TempDir;

use super::{AppState, VirtualShotMode};
use crate::core::{Command, CommandResult};
use crate::models::survey::Project;
use crate::models::{
    ClosureStrategy, Coordinates, ExistingShotUpdate, GpsCoordinates, ImportClosure,
    ImportLineConfig, ImportPlacement, LineId, ShotData, ShotKind, ShotUpdates, StationRef,
};
use crate::persistence::stalx_v2::{self, Extras, Mode};

fn test_shot_data(length: f64) -> ShotData {
    ShotData {
        azimuth_in: 45.0,
        azimuth_out: 225.0,
        length,
        depth_in: 10.0,
        depth_out: 11.0,
        ..Default::default()
    }
}

/// A shot of `length` metres on `azimuth`, level.
fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: 5.0,
        depth_out: 5.0,
        ..Default::default()
    }
}

/// An import line config with the defaults every importer sends.
fn config(
    index: u32,
    name: &str,
    shots: Vec<ShotData>,
    placement: ImportPlacement,
) -> ImportLineConfig {
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
        shots,
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
                name: "Test".to_string(),
                location: None,
            })
            .success
    );
    state
}

/// `execute_import_lines` with no closures, hashes or updates.
fn import(state: &AppState, lines: Vec<ImportLineConfig>) -> CommandResult {
    state.execute_import_lines(
        lines,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    )
}

/// A project holding one line `Existing` of `shots` at the origin (made by
/// an import, the CreateLine command being another package's); its id.
fn state_with_existing(shots: Vec<ShotData>) -> (AppState, LineId) {
    let state = new_state();
    let r = import(
        &state,
        vec![config(0, "Existing", shots, ImportPlacement::Origin)],
    );
    assert!(r.success, "{:?}", r.error);
    let id = state.get_project_state().unwrap().lines()[0].id;
    (state, id)
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

/// The v2 text of `p` without what a step may change by design
/// (`modified_at`, `file_path`) and without the id allocators, which stand
/// higher after an undo (plan 8.1).
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

/// The raw horizontal position of slot `(line, index)`.
fn xy(p: &Project, line: LineId, index: usize) -> [f64; 2] {
    let station = p.line(line).unwrap().station(index).unwrap();
    p.raw_positions().xy(station).unwrap()
}

fn close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
}

/// One undo removes exactly what the last edit did and one redo brings it
/// back, to the bit (session shot ids included).
fn assert_one_exact_entry(state: &AppState, before: &Project) {
    let after = project(state);
    assert!(state.undo().success, "undo failed");
    let undone = project(state);
    assert_eq!(canonical(&undone), canonical(before), "undo is exact");
    assert!(state.redo().success, "redo failed");
    let redone = project(state);
    assert_eq!(canonical(&redone), canonical(&after), "redo is exact");
    let ids = |p: &Project| -> Vec<Vec<u32>> {
        p.lines()
            .iter()
            .map(|l| l.shots.iter().map(|s| s.id).collect())
            .collect()
    };
    assert_eq!(ids(&redone), ids(&after), "session shot ids come back");
}

// ============================================================================
// execute_create_and_merge
// ============================================================================

#[test]
fn test_execute_create_and_merge_success() {
    let (state, existing_line_id) = state_with_existing(vec![test_shot_data(5.0)]);
    let before = project(&state);
    let target = before.line(existing_line_id).unwrap().station(1).unwrap();
    let target_xy = xy(&before, existing_line_id, 1);
    let entries = state.get_undo_stack().len();

    let result = state.execute_create_and_merge(
        Some("Imported".to_string()),
        None,
        None,
        "#00ff00".to_string(),
        None,
        vec![test_shot_data(3.0)],
        0,                                    // import station 0
        StationRef::new(existing_line_id, 1), // onto station 1 of existing
        ClosureStrategy::Batch,
    );
    assert!(
        result.success,
        "create_and_merge failed: {:?}",
        result.error
    );
    let new_line = result.line_id.unwrap();

    let p = project(&state);
    assert_eq!(p.lines().len(), 2);
    // The new line starts ON the target: one station, a junction.
    assert_eq!(p.line(new_line).unwrap().start, target);
    assert!(p.topology().is_junction(target));
    assert!(
        close(xy(&p, existing_line_id, 1), target_xy),
        "nothing moves"
    );
    assert_eq!(p.stations.len(), before.stations.len() + 1);
    assert_eq!(state.get_undo_stack().len(), entries + 1, "ONE entry");
    let top = &state.get_undo_stack()[0];
    assert_eq!(top.command_type, "batch");
    assert_eq!(top.description, "Create line \"Imported\" at Existing / 1");
    let rows: Vec<String> = top
        .preview_data
        .as_ref()
        .unwrap()
        .changes
        .iter()
        .map(|c| c.field.clone())
        .collect();
    assert_eq!(
        rows,
        vec![
            "Create line \"Imported\"".to_string(),
            "Merge Imported / 1 into Existing / 1 (keeps 1)".to_string()
        ]
    );

    // Undo reverts both halves atomically.
    assert_one_exact_entry(&state, &before);
    assert!(state.undo().success);
    let p = project(&state);
    assert_eq!(p.lines().len(), 1, "undo removes the created line");
    assert!(!p.topology().is_junction(target), "and its junction");
}

#[test]
fn test_execute_create_and_merge_rollback_on_merge_failure() {
    let (state, _) = state_with_existing(vec![test_shot_data(5.0)]);
    let before = project(&state);
    let entries = state.get_undo_stack().len();

    let result = state.execute_create_and_merge(
        Some("Imported".to_string()),
        None,
        None,
        "#00ff00".to_string(),
        None,
        vec![test_shot_data(3.0)],
        0,
        StationRef::new(9999, 0), // nonexistent line
        ClosureStrategy::Batch,
    );
    assert!(!result.success, "a target that does not exist is refused");
    assert_eq!(canonical(&project(&state)), canonical(&before));
    assert_eq!(state.get_undo_stack().len(), entries, "no entry");

    // An import station past the new line's end is refused too.
    let result = state.execute_create_and_merge(
        Some("Imported".to_string()),
        None,
        None,
        "#00ff00".to_string(),
        None,
        vec![test_shot_data(3.0)],
        5,
        StationRef::new(before.lines()[0].id, 0),
        ClosureStrategy::Batch,
    );
    assert!(!result.success);
    assert_eq!(canonical(&project(&state)), canonical(&before));
}

#[test]
fn test_execute_create_and_merge_no_project_open() {
    let state = AppState::new().unwrap();
    let result = state.execute_create_and_merge(
        Some("Test".to_string()),
        None,
        None,
        "#ff0000".to_string(),
        None,
        vec![test_shot_data(3.0)],
        0,
        StationRef::new(1, 0),
        ClosureStrategy::Batch,
    );
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("No project open"));
}

/// A New Line snapped onto a station joins the dialog's Section in the same
/// step; a Section that is gone refuses the whole step (0.6.0's words).
#[test]
fn create_and_merge_puts_the_line_in_its_section_or_refuses_a_gone_one() {
    let (state, existing) = state_with_existing(vec![test_shot_data(5.0)]);
    let mut p = project(&state);
    let section = p.mint_section_id().unwrap();
    p.sections.push(crate::models::Section {
        id: section,
        name: "Upstream".to_string(),
        color: "#4ecdc4".to_string(),
    });
    state.set_project_for_tests(p);

    let r = state.execute_create_and_merge_in_cave_section(
        Some("Side".to_string()),
        None,
        None,
        "#00ff00".to_string(),
        None,
        vec![test_shot_data(2.0)],
        1,
        StationRef::new(existing, 0),
        ClosureStrategy::Batch,
        Some(section),
    );
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    let side = p.line(r.line_id.unwrap()).unwrap();
    assert_eq!(side.section_id, Some(section));
    // Placed by its LAST station onto the existing start.
    assert_eq!(
        side.station(1),
        p.line(existing).unwrap().station(0),
        "the chosen chain station is the target"
    );

    let r = state.execute_create_and_merge_in_cave_section(
        Some("Other".to_string()),
        None,
        None,
        "#00ff00".to_string(),
        None,
        vec![test_shot_data(2.0)],
        0,
        StationRef::new(existing, 1),
        ClosureStrategy::Batch,
        Some(section + 7),
    );
    assert!(!r.success);
    assert_eq!(
        r.error.as_deref(),
        Some("The section for the new line no longer exists; it may have been deleted")
    );
}

// ============================================================================
// execute_import_lines
// ============================================================================

#[test]
fn test_execute_import_lines_origin_placement() {
    let state = new_state();
    let before = project(&state);
    let lines = vec![
        config(
            0,
            "Import1",
            vec![test_shot_data(5.0)],
            ImportPlacement::Origin,
        ),
        config(
            1,
            "Import2",
            vec![test_shot_data(3.0)],
            ImportPlacement::Origin,
        ),
    ];
    let result = import(&state, lines);
    assert!(result.success, "import failed: {:?}", result.error);

    let p = project(&state);
    assert_eq!(p.lines().len(), 2);
    for line in p.lines() {
        assert!(close(xy(&p, line.id, 0), [0.0, 0.0]), "station 0 at origin");
    }
    assert_eq!(p.placements.len(), 2, "two free pieces, one placement each");
    assert_eq!(state.get_undo_stack().len(), 1);
    assert_eq!(
        state.get_undo_stack()[0].description,
        "Import lines \"Import1\" and \"Import2\""
    );

    // Undo reverts all lines atomically, bit for bit.
    assert_one_exact_entry(&state, &before);
    assert!(state.undo().success);
    assert_eq!(project(&state).lines().len(), 0);
}

/// 0.6.0's label names the lines it made: one, up to three, or the first
/// three and a count; the preview lists the batch's first eight rows and
/// counts the rest.
#[test]
fn the_import_entry_is_labelled_and_previewed_as_0_6_0_did() {
    let state = new_state();
    let lines: Vec<ImportLineConfig> = (0..5)
        .map(|i| {
            config(
                i,
                &format!("L{i}"),
                vec![test_shot_data(2.0)],
                ImportPlacement::Origin,
            )
        })
        .collect();
    assert!(import(&state, lines).success);
    let top = &state.get_undo_stack()[0];
    assert_eq!(
        top.description,
        "Import 5 lines (\"L0\", \"L1\", \"L2\" and 2 more)"
    );

    let state = new_state();
    let lines = vec![config(
        0,
        "Solo",
        vec![test_shot_data(2.0)],
        ImportPlacement::ZeroDatum {
            coordinates: GpsCoordinates::new(20.0, -87.0, 0.0).unwrap(),
            station_index: 0,
        },
    )];
    assert!(import(&state, lines).success);
    let top = &state.get_undo_stack()[0];
    assert_eq!(top.description, "Import line \"Solo\"");
    let preview = top.preview_data.as_ref().unwrap();
    let rows: Vec<(String, String)> = preview
        .changes
        .iter()
        .map(|c| (c.field.clone(), c.new_value.clone()))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("Create line \"Solo\"".to_string(), "Name: Solo".to_string()),
            (
                "Set zero datum at Solo / 0".to_string(),
                "Lat: 20.000000, Lon: -87.000000".to_string()
            ),
        ]
    );
}

/// A closure referencing a line index that never resolves must be counted
/// as skipped on the receipt, not silently dropped.
#[test]
fn test_execute_import_lines_unresolvable_closure_is_skipped_on_receipt() {
    let state = new_state();
    let lines = vec![config(
        0,
        "Import1",
        vec![test_shot_data(5.0)],
        ImportPlacement::Origin,
    )];
    let closures = vec![ImportClosure {
        from_line_index: 0,
        from_station_index: 0,
        to_line_index: 99,
        to_station_index: 0,
    }];
    let result = state.execute_import_lines(
        lines,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);
    let receipt = result
        .import_receipt
        .as_ref()
        .expect("import must attach an import receipt");
    assert_eq!(receipt.closures_requested, 1);
    assert_eq!(receipt.closures_created, 0);
    assert_eq!(
        receipt.warnings,
        vec!["1 loop closure could not be created and was skipped".to_string()]
    );
}

/// The closure refusals run on the placed survey in one pass: a loop is
/// created; one across one shot, one twice for a pair and one between two
/// free pieces are refused and counted.
#[test]
fn import_closures_are_refused_as_a_closure_is_and_counted() {
    let state = new_state();
    let lines = vec![
        config(
            0,
            "Main",
            vec![shot(0.0, 10.0), shot(90.0, 10.0), shot(180.0, 10.0)],
            ImportPlacement::Origin,
        ),
        config(1, "Free", vec![shot(0.0, 4.0)], ImportPlacement::Origin),
    ];
    let closure = |a: (u32, u32), b: (u32, u32)| ImportClosure {
        from_line_index: a.0,
        from_station_index: a.1,
        to_line_index: b.0,
        to_station_index: b.1,
    };
    let closures = vec![
        closure((0, 0), (0, 3)), // a real loop
        closure((0, 3), (0, 0)), // the same pair again
        closure((0, 1), (0, 2)), // the two ends of one shot
        closure((0, 0), (1, 1)), // two free pieces
    ];
    let result = state.execute_import_lines(
        lines,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "{:?}", result.error);
    let receipt = result.import_receipt.unwrap();
    assert_eq!(
        (receipt.closures_requested, receipt.closures_created),
        (4, 1)
    );
    assert_eq!(
        receipt.warnings,
        vec!["3 loop closures could not be created and were skipped".to_string()]
    );
    let p = project(&state);
    assert_eq!(p.closures.len(), 1);
    assert_eq!(
        p.closures[0].strategy,
        crate::models::survey::ClosureStrategy::Batch
    );
}

/// #27: the Jump/Reference pass runs INSIDE the import, so redo restores the
/// classified kinds.
#[test]
fn imported_shot_kinds_survive_an_undo_redo_round_trip() {
    let state = new_state();
    // Real - virtual - real: the chain rule calls the middle shot a Jump.
    let virtual_shot = ShotData {
        kind: ShotKind::Reference,
        ..test_shot_data(4.0)
    };
    let lines = vec![config(
        0,
        "Imported",
        vec![test_shot_data(5.0), virtual_shot, test_shot_data(6.0)],
        ImportPlacement::Origin,
    )];
    let result = import(&state, lines);
    assert!(result.success, "import failed: {:?}", result.error);

    let kind_of_middle = |state: &AppState| project(state).lines()[0].shots[1].kind;
    assert_eq!(
        kind_of_middle(&state),
        ShotKind::Jump,
        "a virtual run bounded by real survey classifies as Jump at import"
    );
    assert!(state.undo().success);
    assert!(state.redo().success);
    assert_eq!(
        kind_of_middle(&state),
        ShotKind::Jump,
        "redo must restore the CLASSIFIED kind, not the parser's Reference seed"
    );
}

/// #27: the per-file override is per *file*: a second import never re-kinds
/// shots the user set in an earlier session.
#[test]
fn importing_a_second_file_leaves_existing_shot_kinds_alone() {
    let state = new_state();
    let reference_shot = ShotData {
        kind: ShotKind::Reference,
        ..test_shot_data(4.0)
    };
    let first = import(
        &state,
        vec![config(
            0,
            "First",
            vec![
                test_shot_data(5.0),
                reference_shot.clone(),
                test_shot_data(6.0),
            ],
            ImportPlacement::Origin,
        )],
    );
    assert!(first.success, "first import failed: {:?}", first.error);

    let (line_id, shot_id) = {
        let p = project(&state);
        (p.lines()[0].id, p.lines()[0].shots[1].id)
    };
    let r = state.execute(Command::SetShotKind {
        line_id,
        shot_id,
        kind: ShotKind::Reference,
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(
        project(&state).lines()[0].shots[1].kind,
        ShotKind::Reference
    );

    let second = state.execute_import_lines(
        vec![config(
            0,
            "Second",
            vec![reference_shot],
            ImportPlacement::Origin,
        )],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::AllJumps,
    );
    assert!(second.success, "second import failed: {:?}", second.error);

    let p = project(&state);
    assert_eq!(
        p.lines()[0].shots[1].kind,
        ShotKind::Reference,
        "the hand-set kind in the FIRST file must survive a later import"
    );
    assert_eq!(
        p.lines()[1].shots[0].kind,
        ShotKind::Jump,
        "the override still applies to the file being imported"
    );
}

/// #275: overlap-interpolation updates (parsed file content) hold the same
/// finite invariant as UpdateShot: a NaN fails the whole import before any
/// change.
#[test]
fn test_execute_import_lines_rejects_non_finite_shot_update() {
    let (state, line_id) = state_with_existing(vec![test_shot_data(5.0)]);
    let before = project(&state);
    let shot_id = before.lines()[0].shots[0].id;

    let result = state.execute_import_lines(
        vec![config(
            0,
            "Incoming",
            vec![test_shot_data(3.0)],
            ImportPlacement::Origin,
        )],
        Vec::new(),
        None,
        Vec::new(),
        vec![ExistingShotUpdate {
            line_id,
            shot_id,
            updates: ShotUpdates {
                length: Some(f64::NAN),
                ..Default::default()
            },
        }],
        VirtualShotMode::Auto,
    );
    assert!(
        !result.success,
        "a non-finite shot update must fail the import"
    );
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("non-finite"),
        "error should name the invariant: {:?}",
        result.error
    );
    assert_eq!(canonical(&project(&state)), canonical(&before));
}

/// An overlap update past the measurement bound is refused in words before
/// any change, as an UpdateShot is: left to the import's final validation it
/// was "would corrupt the survey structure".
#[test]
fn test_execute_import_lines_refuses_a_shot_update_past_the_bound() {
    let (state, line_id) = state_with_existing(vec![test_shot_data(5.0)]);
    let before = project(&state);
    let shot_id = before.lines()[0].shots[0].id;

    let result = state.execute_import_lines(
        vec![config(
            0,
            "Incoming",
            vec![test_shot_data(3.0)],
            ImportPlacement::Origin,
        )],
        Vec::new(),
        None,
        Vec::new(),
        vec![ExistingShotUpdate {
            line_id,
            shot_id,
            updates: ShotUpdates {
                length: Some(3_000_000.0),
                ..Default::default()
            },
        }],
        VirtualShotMode::Auto,
    );
    assert!(
        !result.success,
        "an absurd shot update must fail the import"
    );
    assert_eq!(
        result.error.as_deref(),
        Some(
            format!(
                "Import shot update for line {line_id} shot {shot_id}: A shot's length of \
                 3000000 m is beyond the 1000000 m a survey measurement may be"
            )
            .as_str()
        )
    );
    assert_eq!(canonical(&project(&state)), canonical(&before));
}

#[test]
fn test_execute_import_lines_from_importing_placement() {
    let state = new_state();
    let lines = vec![
        config(
            0,
            "First",
            vec![test_shot_data(5.0), test_shot_data(3.0)],
            ImportPlacement::Origin,
        ),
        config(
            1,
            "Second",
            vec![test_shot_data(4.0)],
            ImportPlacement::FromImporting {
                import_station: 0,
                target_line_index: 0,
                target_station_index: 1,
            },
        ),
    ];
    let result = state.execute_import_lines(
        lines,
        Vec::new(),
        Some("abc123".to_string()),
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);

    let p = project(&state);
    assert_eq!(p.lines().len(), 2);
    let (first, second) = (&p.lines()[0], &p.lines()[1]);
    assert_eq!(second.start, first.station(1).unwrap(), "a junction");
    assert_eq!(p.merge_pairs().len(), 1, "one wire merge pair");
    assert_eq!(p.stations.len(), 4, "3 + 1: no station minted for the join");
    assert_eq!(p.imported_file_hashes, vec!["abc123".to_string()]);
    assert_eq!(p.placements.len(), 1, "one piece");
}

#[test]
fn test_execute_import_lines_from_existing_placement() {
    let (state, existing_id) = state_with_existing(vec![test_shot_data(5.0), test_shot_data(3.0)]);
    let before = project(&state);
    let target_xy = xy(&before, existing_id, 1);
    let lines = vec![config(
        0,
        "Joined",
        vec![test_shot_data(4.0)],
        ImportPlacement::FromExisting {
            import_station: 0,
            existing_station: StationRef::new(existing_id, 1),
        },
    )];
    let result = import(&state, lines);
    assert!(result.success, "import failed: {:?}", result.error);

    let p = project(&state);
    assert_eq!(p.lines().len(), 2);
    assert_eq!(p.merge_pairs().len(), 1);
    let joined = p.lines()[1].id;
    assert!(
        close(xy(&p, joined, 0), target_xy),
        "it hangs off the target"
    );
    for i in 0..=2 {
        assert!(
            close(xy(&p, existing_id, i), xy(&before, existing_id, i)),
            "nothing existing moves"
        );
    }
    assert_one_exact_entry(&state, &before);
}

/// A Manual placement puts the file's first station at the offset (0.6.0's
/// Move Line of the new line).
#[test]
fn test_execute_import_lines_manual_with_offset() {
    let state = new_state();
    let lines = vec![config(
        0,
        "Manual",
        vec![test_shot_data(5.0)],
        ImportPlacement::Manual {
            align_station_index: 0,
            offset: Some(Coordinates {
                x: 10.0,
                y: 20.0,
                z: 0.0,
            }),
        },
    )];
    let result = import(&state, lines);
    assert!(result.success, "import failed: {:?}", result.error);

    let p = project(&state);
    let line_id = p.lines()[0].id;
    assert!(close(xy(&p, line_id, 0), [10.0, 20.0]));
    assert_eq!(p.placements.len(), 1);
    let rows = state.get_undo_stack()[0]
        .preview_data
        .clone()
        .unwrap()
        .changes;
    assert_eq!(rows[1].field, "Move line \"Manual\"");
}

/// A reversed line's placement keeps the FILE's first station where the
/// un-reversed import would have put it (now the line's last station), as
/// "import, then Reverse Line" does.
#[test]
fn a_reversed_import_keeps_the_files_first_station_in_place() {
    let shots = vec![shot(90.0, 10.0), shot(0.0, 5.0)];
    let plain = new_state();
    assert!(
        import(
            &plain,
            vec![config(0, "A", shots.clone(), ImportPlacement::Origin)]
        )
        .success
    );
    let reversed = new_state();
    let mut cfg = config(0, "A", shots, ImportPlacement::Origin);
    cfg.reverse = true;
    assert!(import(&reversed, vec![cfg]).success);
    let (p, r) = (project(&plain), project(&reversed));
    let (pl, rl) = (p.lines()[0].id, r.lines()[0].id);
    for i in 0..=2 {
        let (a, b) = (xy(&p, pl, i), xy(&r, rl, 2 - i));
        assert!(
            (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9,
            "station {i}: {a:?} vs {b:?}"
        );
    }
    // The gauge (placement) is the same physical station either way.
    assert_eq!(
        r.placements[0].station,
        r.line(rl).unwrap().station(2).unwrap()
    );
}

#[test]
fn test_execute_import_lines_zero_datum_placement() {
    let state = new_state();
    let gps = GpsCoordinates::new(10.0, 20.0, 5.0).unwrap();
    let lines = vec![config(
        0,
        "GPS Line",
        vec![test_shot_data(5.0)],
        ImportPlacement::ZeroDatum {
            coordinates: gps,
            station_index: 0,
        },
    )];
    let result = import(&state, lines);
    assert!(result.success, "import failed: {:?}", result.error);

    let p = project(&state);
    assert_eq!(p.datums.len(), 1);
    assert!((p.datums[0].gps.latitude - 10.0).abs() < 1e-6);
    assert_eq!(p.datums[0].via, Some(p.lines()[0].id));
    assert!(p.placements.is_empty(), "a datum piece has no placement");
    assert_eq!(
        result.import_receipt.unwrap().zero_datum_created.as_deref(),
        Some("GPS Line")
    );
}

/// The import's datum on a station that is NOT its line's start still lands
/// that station on local (0, 0): the first datum is the frame.
#[test]
fn test_execute_import_lines_zero_datum_placement_at_non_origin_station() {
    let state = new_state();
    let gps = GpsCoordinates::new(10.0, 20.0, 5.0).unwrap();
    let imported_shot = test_shot_data(5.0);
    let lines = vec![config(
        0,
        "GPS Line",
        vec![imported_shot.clone()],
        ImportPlacement::ZeroDatum {
            coordinates: gps,
            station_index: 1,
        },
    )];
    let result = import(&state, lines);
    assert!(result.success, "import failed: {:?}", result.error);

    let p = project(&state);
    assert_eq!(p.datums.len(), 1);
    let line_id = p.lines()[0].id;
    let datum_station = xy(&p, line_id, 1);
    assert!(
        datum_station[0].abs() < 1e-9 && datum_station[1].abs() < 1e-9,
        "the import's first datum has to land its station on local (0, 0), got {datum_station:?}"
    );
    let station_zero = xy(&p, line_id, 0);
    assert!(
        (station_zero[0].hypot(station_zero[1]) - imported_shot.horizontal_length()).abs() < 1e-9,
        "the line had to move by the whole shot to put station 1 on the origin; station 0 is \
         at {station_zero:?}"
    );
}

/// Since #814 a failed placement costs the line its connection, not the
/// import: both lines import, the second a free piece, and the receipt says
/// why. The DMP wizard's all-or-nothing route still rolls the whole import
/// back, exactly, with no entry.
#[test]
fn test_execute_import_lines_keeps_a_line_whose_merge_fails() {
    let lines = || {
        vec![
            config(
                0,
                "GoodLine",
                vec![test_shot_data(5.0)],
                ImportPlacement::Origin,
            ),
            config(
                1,
                "BadLine",
                vec![test_shot_data(3.0)],
                ImportPlacement::FromExisting {
                    import_station: 0,
                    existing_station: StationRef::new(9999, 0),
                },
            ),
        ]
    };
    let state = new_state();
    let before = project(&state);
    let result = import(&state, lines());
    assert!(result.success, "{:?}", result.error);
    let p = project(&state);
    assert_eq!(p.lines().len(), 2, "both lines import");
    assert!(
        p.merge_pairs().is_empty(),
        "the failed placement is not made"
    );
    assert_eq!(
        result.import_receipt.unwrap().line_problems,
        Some(vec![
            "Line \"BadLine\" was imported, but its connection to the existing survey could \
             not be made: Station 0 of line 9999 does not exist"
                .to_string()
        ])
    );
    assert_one_exact_entry(&state, &before);

    let state = new_state();
    let before = project(&state);
    let result = state.execute_import_lines_all_or_nothing(
        lines(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(
        !result.success,
        "the DMP route keeps the whole import or nothing"
    );
    assert_eq!(
        result.error.as_deref(),
        Some(
            "The import was undone and nothing was kept, because one line could not be \
             imported whole. Line \"BadLine\" was imported, but its connection to the \
             existing survey could not be made: Station 0 of line 9999 does not exist"
        )
    );
    let p = project(&state);
    assert_eq!(
        p.lines().len(),
        0,
        "rollback should remove all lines from failed import"
    );
    assert_eq!(canonical(&p), canonical(&before));
    assert!(state.get_undo_stack().is_empty());
}

/// A same-batch placement onto a line not built yet: the line imports on its
/// own (#814); the all-or-nothing route refuses the import.
#[test]
fn a_placement_onto_a_line_not_created_yet_imports_it_on_its_own() {
    let lines = || {
        vec![config(
            0,
            "Early",
            vec![test_shot_data(3.0)],
            ImportPlacement::FromImporting {
                import_station: 0,
                target_line_index: 1,
                target_station_index: 0,
            },
        )]
    };
    let state = new_state();
    let result = import(&state, lines());
    assert!(result.success, "{:?}", result.error);
    assert_eq!(project(&state).lines().len(), 1);
    assert_eq!(
        result.import_receipt.unwrap().line_problems,
        Some(vec![
            "Line \"Early\" was imported on its own: the line it joins was not imported"
                .to_string()
        ])
    );

    let state = new_state();
    let result = state.execute_import_lines_all_or_nothing(
        lines(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert_eq!(
        result.error.as_deref(),
        Some(
            "The import was undone and nothing was kept, because one line could not be \
             imported whole. Line \"Early\" was imported on its own: the line it joins was \
             not imported"
        )
    );
    assert!(project(&state).lines().is_empty());
}

#[test]
fn test_execute_import_lines_empty_is_noop() {
    let state = new_state();
    let result = import(&state, vec![]);
    assert!(result.success);
    let receipt = result.import_receipt.unwrap();
    assert_eq!(receipt.lines_imported, 0);
    assert!(state.get_undo_stack().is_empty());
}

#[test]
fn test_execute_import_lines_no_project() {
    let state = AppState::new().unwrap();
    let lines = vec![config(
        0,
        "Test",
        vec![test_shot_data(5.0)],
        ImportPlacement::Origin,
    )];
    let result = import(&state, lines);
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("No project open"));
}

#[test]
fn test_execute_import_lines_manual_snap_placement() {
    let (state, existing_id) = state_with_existing(vec![test_shot_data(5.0)]);
    let lines = vec![config(
        0,
        "Snapped",
        vec![test_shot_data(4.0)],
        ImportPlacement::ManualSnap {
            align_station_index: 0,
            snapped_to_station: StationRef::new(existing_id, 1),
        },
    )];
    let result = import(&state, lines);
    assert!(
        result.success,
        "import with ManualSnap failed: {:?}",
        result.error
    );
    let p = project(&state);
    assert_eq!(p.merge_pairs().len(), 1);
}

/// A file imported twice brings uuids the project holds: the project keeps
/// its ids and the newcomers yield (re-minted, 0.6.0's rule), so every shot
/// uuid stays unique; a repeat inside one line keeps its first occurrence.
#[test]
fn repeated_shot_uuids_are_re_minted_for_the_newcomers() {
    let with_uuid = |uuid: &str, length: f64| ShotData {
        uuid: Some(uuid.to_string()),
        ..test_shot_data(length)
    };
    let file = || {
        vec![config(
            0,
            "Ariane",
            vec![
                with_uuid("u-1", 3.0),
                with_uuid("u-2", 4.0),
                with_uuid("u-1", 5.0),
            ],
            ImportPlacement::Origin,
        )]
    };
    let state = new_state();
    assert!(import(&state, file()).success);
    assert!(import(&state, file()).success);
    let p = project(&state);
    let first: Vec<String> = p.lines()[0].shots.iter().map(|s| s.uuid.clone()).collect();
    assert_eq!(&first[..2], &["u-1".to_string(), "u-2".to_string()]);
    assert_ne!(first[2], "u-1", "the in-line repeat gets a fresh id");
    let second: Vec<String> = p.lines()[1].shots.iter().map(|s| s.uuid.clone()).collect();
    assert!(second.iter().all(|u| !first.contains(u)), "{second:?}");
    assert_eq!(
        second[0],
        crate::models::reminted_shot_uuid(p.lines()[1].id, "u-1")
    );
    assert!(p.duplicate_shot_uuids().is_empty());
}

/// Auto-grouping (#327): a config naming a Section joins the project's
/// Section of that name, created (next palette colour) when there is none;
/// one undo takes the created Section away with the lines.
#[test]
fn imported_lines_join_sections_by_name() {
    let state = new_state();
    let mut p = project(&state);
    let existing = p.mint_section_id().unwrap();
    p.sections.push(crate::models::Section {
        id: existing,
        name: "Upper".to_string(),
        color: "#123456".to_string(),
    });
    state.set_project_for_tests(p);
    let before = project(&state);

    let mut a = config(0, "A", vec![test_shot_data(2.0)], ImportPlacement::Origin);
    a.cave_section = Some("  Upper ".to_string());
    let mut b = config(1, "B", vec![test_shot_data(2.0)], ImportPlacement::Origin);
    b.cave_section = Some("Lower".to_string());
    let mut c = config(2, "C", vec![test_shot_data(2.0)], ImportPlacement::Origin);
    c.cave_section = Some("Lower".to_string());
    assert!(import(&state, vec![a, b, c]).success);
    let p = project(&state);
    assert_eq!(p.sections.len(), 2);
    let lower = p.sections.iter().find(|s| s.name == "Lower").unwrap();
    assert_eq!(lower.color, crate::models::SECTION_PALETTE[1]);
    let sections: Vec<Option<u32>> = p.lines().iter().map(|l| l.section_id).collect();
    assert_eq!(
        sections,
        vec![Some(existing), Some(lower.id), Some(lower.id)]
    );
    assert_one_exact_entry(&state, &before);
}

/// Overlap interpolation (the DMP wizard): the updates rewrite existing
/// shots inside the import's one entry, and a free line keeps its first
/// station (a geometry edit).
#[test]
fn overlap_updates_apply_in_the_imports_entry() {
    let (state, line) = state_with_existing(vec![shot(0.0, 10.0), shot(0.0, 10.0)]);
    let before = project(&state);
    let shot_id = before.lines()[0].shots[1].id;
    let update = |length: f64| ExistingShotUpdate {
        line_id: line,
        shot_id,
        updates: ShotUpdates {
            length: Some(length),
            ..Default::default()
        },
    };
    let r = state.execute_import_lines(
        vec![config(
            0,
            "New",
            vec![shot(90.0, 3.0)],
            ImportPlacement::FromExisting {
                import_station: 0,
                existing_station: StationRef::new(line, 2),
            },
        )],
        Vec::new(),
        None,
        Vec::new(),
        vec![update(12.0)],
        VirtualShotMode::Auto,
    );
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    assert_eq!(p.line(line).unwrap().shots[1].length, 12.0);
    assert!(
        close(xy(&p, line, 0), [0.0, 0.0]),
        "the first station stays"
    );
    assert!(close(xy(&p, line, 2), [0.0, 22.0]));
    assert_eq!(state.get_undo_stack().len(), 2, "one entry for the import");
    assert_one_exact_entry(&state, &before);
}

/// 0.6.0's doubly-anchored guard: a line whose updated shot sits between
/// two anchors (here a junction with the datum line at its start and an
/// aligned snap onto that line at its end) keeps its shots, and the receipt
/// says so; the rest of the import still lands.
#[test]
fn an_overlap_update_between_two_anchors_is_kept_and_reported() {
    use crate::models::survey::{ClosureStrategy as Strategy, LineMeta, StartAt, ZeroDatum};

    let meta = |name: &str| LineMeta {
        name: Some(name.to_string()),
        color: "#ff0000".to_string(),
        ..LineMeta::default()
    };
    let mut p = Project::new("Guard", None);
    let main = p
        .add_line(
            meta("Main"),
            StartAt::New,
            vec![shot(90.0, 10.0), shot(0.0, 10.0)],
        )
        .unwrap();
    let start = p.line(main).unwrap().start;
    let side = p
        .add_line(
            meta("Side"),
            StartAt::Station(start),
            vec![shot(0.0, 10.0), shot(90.0, 10.0)],
        )
        .unwrap();
    p.push_datum(ZeroDatum::new(
        start,
        GpsCoordinates::new(20.5, -87.3, 0.0).unwrap(),
    ));
    let (main_end, side_end) = (
        p.line(main).unwrap().station(2).unwrap(),
        p.line(side).unwrap().station(2).unwrap(),
    );
    p.push_closure(side_end, Some(side), main_end, Some(main), Strategy::Move);
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    assert_eq!(p.closure_alignment(&p.raw_positions()), vec![true]);
    let state = AppState::new().unwrap();
    state.set_project_for_tests(p);
    let before = project(&state);

    let shot_id = before.line(side).unwrap().shots[1].id;
    let r = state.execute_import_lines(
        vec![config(
            0,
            "New",
            vec![shot(180.0, 3.0)],
            ImportPlacement::Origin,
        )],
        Vec::new(),
        None,
        Vec::new(),
        vec![ExistingShotUpdate {
            line_id: side,
            shot_id,
            updates: ShotUpdates {
                length: Some(12.0),
                ..Default::default()
            },
        }],
        VirtualShotMode::Auto,
    );
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    assert_eq!(p.line(side).unwrap().shots[1].length, 10.0, "kept");
    assert_eq!(p.lines().len(), 3, "the import itself landed");
    assert_eq!(
        r.import_receipt.unwrap().warnings,
        vec!["1 shot kept its position: line anchored on both sides".to_string()]
    );
}

// ============================================================================
// Import hash tracking
// ============================================================================

#[test]
fn test_import_tracking_happy_path() {
    let state = new_state();
    let hash = "abc123deadbeef".to_string();
    assert!(!state.check_duplicate_import(&hash).unwrap());
    assert!(state.track_imported_file(hash.clone()).success);
    assert!(state.check_duplicate_import(&hash).unwrap());
    // A recorded edit: one undo forgets it.
    assert_eq!(state.get_undo_stack().len(), 1);
    assert!(state.undo().success);
    assert!(!state.check_duplicate_import(&hash).unwrap());
}

#[test]
fn test_import_tracking_idempotent() {
    let state = new_state();
    let hash = "deadbeef".to_string();
    state.track_imported_file(hash.clone());
    state.track_imported_file(hash);
    let p = project(&state);
    assert_eq!(
        p.imported_file_hashes
            .iter()
            .filter(|h| h.as_str() == "deadbeef")
            .count(),
        1
    );
    assert_eq!(
        state.get_undo_stack().len(),
        1,
        "a no-op track records nothing"
    );
}

#[test]
fn test_line_hash_tracking_batch() {
    let state = new_state();
    let hashes = vec!["h1".to_string(), "h2".to_string(), "h3".to_string()];
    assert_eq!(
        state.check_line_hashes(&hashes).unwrap(),
        vec![false, false, false]
    );
    state.track_line_hashes(vec!["h1".to_string(), "h3".to_string()]);
    assert_eq!(
        state.check_line_hashes(&hashes).unwrap(),
        vec![true, false, true]
    );
    state.track_line_hashes(hashes);
    let p = project(&state);
    assert_eq!(
        p.imported_line_hashes
            .iter()
            .filter(|h| h.as_str() == "h1")
            .count(),
        1,
        "h1 must not be duplicated"
    );
}

#[test]
fn test_import_tracking_no_project_returns_false() {
    let state = AppState::new().unwrap();
    let result = state.check_duplicate_import("somehash");
    assert!(result.is_ok());
    assert!(!result.unwrap());
}

#[test]
fn test_track_imported_file_no_project_fails() {
    let state = AppState::new().unwrap();
    assert!(!state.track_imported_file("hash".to_string()).success);
}

/// Withdrawing an import forgets its dedup bookkeeping in the SAME undo
/// entry as the deletes (#394).
#[test]
fn a_withdrawal_forgets_the_import_tracking_and_undo_restores_it() {
    let state = new_state();
    let r = state.execute_import_lines(
        vec![config(
            0,
            "A",
            vec![test_shot_data(5.0)],
            ImportPlacement::Origin,
        )],
        Vec::new(),
        Some("file-hash".to_string()),
        vec!["sec-a".to_string(), "sec-b".to_string()],
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(r.success, "{:?}", r.error);
    let line_id = r.line_ids[0];
    assert!(state.check_duplicate_import("file-hash").unwrap());
    let before = project(&state);

    let withdrawn = state.execute(Command::Batch {
        commands: vec![
            Command::DeleteLine { line_id },
            Command::UntrackImport {
                file_hashes: vec!["file-hash".to_string()],
                line_hashes: vec!["sec-a".to_string()],
            },
        ],
        label: None,
    });
    assert!(withdrawn.success, "withdrawal: {:?}", withdrawn.error);
    assert!(
        !state.check_duplicate_import("file-hash").unwrap(),
        "the file must stop counting as imported once its lines are gone"
    );
    assert_eq!(
        state
            .check_line_hashes(&["sec-a".to_string(), "sec-b".to_string()])
            .unwrap(),
        vec![false, true],
        "only the named line hash is forgotten; an untouched one stays"
    );

    // ONE undo brings back the lines AND their tracking.
    assert!(state.undo().success);
    assert_eq!(canonical(&project(&state)), canonical(&before));
    assert!(state.check_duplicate_import("file-hash").unwrap());
    assert_eq!(
        state.check_line_hashes(&["sec-a".to_string()]).unwrap(),
        vec![true]
    );
}

/// Untracking a hash nobody tracked is a no-op, not a failure (the wizard
/// names what its own run sent): a failure would roll back the whole
/// withdrawal batch.
#[test]
fn untracking_an_unknown_hash_succeeds() {
    let state = new_state();
    let result = state.execute(Command::Batch {
        commands: vec![Command::UntrackImport {
            file_hashes: vec!["never-seen".to_string()],
            line_hashes: vec!["nor-this".to_string()],
        }],
        label: None,
    });
    assert!(result.success, "{:?}", result.error);
    assert!(state.get_undo_stack().is_empty(), "nothing changed");
}

/// 0.6.0's open finding (EXTENDED seeds 4, 10, 18, 28, 30): a stand-alone
/// UntrackImport changed the tracking and cleared redo but pushed no entry.
/// It is an ordinary recorded edit now.
#[test]
fn untrack_import_alone_is_undoable() {
    let state = new_state();
    let r = state.execute_import_lines(
        vec![config(
            0,
            "A",
            vec![test_shot_data(5.0)],
            ImportPlacement::Origin,
        )],
        Vec::new(),
        Some("f".to_string()),
        vec!["l".to_string()],
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(r.success);
    let before = project(&state);
    let entries = state.get_undo_stack().len();
    let untracked = state.execute(Command::UntrackImport {
        file_hashes: vec!["f".to_string()],
        line_hashes: vec!["l".to_string()],
    });
    assert!(untracked.success, "{:?}", untracked.error);
    let p = project(&state);
    assert!(p.imported_file_hashes.is_empty() && p.imported_line_hashes.is_empty());
    assert_eq!(state.get_undo_stack().len(), entries + 1);
    assert_one_exact_entry(&state, &before);
}

/// A large tree-shaped import (each line hanging off the one before it, the
/// shape the Ariane and Compass converters produce, plus a loop closure per
/// ten lines) lands as one entry and stays linear: the funnel builds every
/// line, settles the anchors and judges the closures once.
#[test]
fn a_large_tree_shaped_import_is_one_entry() {
    const LINES: u32 = 1500;
    let lines: Vec<ImportLineConfig> = (0..LINES)
        .map(|i| {
            let placement = if i == 0 {
                ImportPlacement::Origin
            } else {
                ImportPlacement::FromImporting {
                    import_station: 0,
                    target_line_index: i - 1,
                    target_station_index: 2,
                }
            };
            config(
                i,
                &format!("L{i}"),
                (0..4)
                    .map(|k| shot(f64::from((i * 37 + k * 90) % 360), 5.0))
                    .collect(),
                placement,
            )
        })
        .collect();
    let closures: Vec<ImportClosure> = (1..LINES)
        .step_by(10)
        .map(|i| ImportClosure {
            from_line_index: i,
            from_station_index: 4,
            to_line_index: i - 1,
            to_station_index: 4,
        })
        .collect();
    let requested = closures.len() as u32;
    let state = new_state();
    let started = std::time::Instant::now();
    let r = state.execute_import_lines(
        lines,
        closures,
        Some("big".to_string()),
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    let elapsed = started.elapsed();
    assert!(r.success, "{:?}", r.error);
    let receipt = r.import_receipt.unwrap();
    assert_eq!(receipt.lines_imported, LINES);
    assert_eq!(receipt.closures_requested, requested);
    assert_eq!(receipt.closures_created, requested);
    assert_eq!(state.get_undo_stack().len(), 1);
    let p = project(&state);
    assert_eq!(p.placements.len(), 1, "one piece");
    assert_eq!(p.stations.len() as u32, 1 + LINES * 4);
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "the import took {elapsed:?}"
    );
}

// ============================================================================
// The read-only gate
// ============================================================================

/// Every import door is an edit: on a SpeleoDB project opened without its
/// write lock each one refuses with the EDIT_DISABLED code and changes
/// nothing (#191).
#[test]
fn every_import_door_is_gated_on_a_read_only_remote_project() {
    let (state, line) = state_with_existing(vec![test_shot_data(5.0)]);
    state.set_speleo_link_for_tests(Some(&crate::models::SpeleoLink {
        endpoint: "http://speleodb.invalid/".to_string(),
        project_id: "11111111-1111-1111-1111-111111111111".to_string(),
        project_name: "Remote Cave".to_string(),
        base_commit_sha: Some("abc123".to_string()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }));
    let before = project(&state);
    let disabled = |label: &str, r: &CommandResult| {
        assert!(!r.success, "{label} must be refused");
        assert!(
            r.error
                .as_deref()
                .is_some_and(|e| e.starts_with(crate::core::EDIT_DISABLED_CODE)),
            "{label}: {:?}",
            r.error
        );
    };
    disabled(
        "execute_import_lines",
        &import(
            &state,
            vec![config(
                0,
                "Imp",
                vec![test_shot_data(3.0)],
                ImportPlacement::Origin,
            )],
        ),
    );
    disabled(
        "execute_tml_import_sections",
        &state.execute_tml_import_sections(
            vec![config(
                0,
                "Imp",
                vec![test_shot_data(3.0)],
                ImportPlacement::Origin,
            )],
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            VirtualShotMode::Auto,
            Some(10.0),
        ),
    );
    disabled(
        "execute_create_and_merge",
        &state.execute_create_and_merge(
            Some("Imp".to_string()),
            None,
            None,
            "#00ff00".to_string(),
            None,
            vec![test_shot_data(3.0)],
            0,
            StationRef::new(line, 1),
            ClosureStrategy::Batch,
        ),
    );
    disabled(
        "track_imported_file",
        &state.track_imported_file("hash".to_string()),
    );
    disabled(
        "untrack_import",
        &state.execute(Command::UntrackImport {
            file_hashes: vec!["hash".to_string()],
            line_hashes: Vec::new(),
        }),
    );
    assert_eq!(canonical(&project(&state)), canonical(&before));
}

// ============================================================================
// Persistence and the deferred first save
// ============================================================================

/// The import's entry is persisted with the file: after a save and a
/// reopen, one undo still removes the whole import, and the reopened
/// project equals the saved one.
#[test]
fn the_import_entry_survives_a_save_and_a_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("imported.stalx");
    let state = new_state();
    let before = project(&state);
    let lines = vec![
        config(
            0,
            "Main",
            vec![shot(0.0, 10.0), shot(90.0, 4.0)],
            ImportPlacement::ZeroDatum {
                coordinates: GpsCoordinates::new(20.5, -87.3, 0.0).unwrap(),
                station_index: 0,
            },
        ),
        config(
            1,
            "Side",
            vec![shot(180.0, 3.0)],
            ImportPlacement::FromImporting {
                import_station: 0,
                target_line_index: 0,
                target_station_index: 1,
            },
        ),
    ];
    assert!(import(&state, lines).success);
    let imported = project(&state);
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    let reopened = reopen(&path);
    assert_eq!(canonical(&project(&reopened)), canonical(&imported));
    assert_eq!(reopened.get_undo_stack().len(), 1);
    assert!(reopened.undo().success);
    assert_eq!(canonical(&project(&reopened)), canonical(&before));
    assert!(reopened.redo().success);
    assert_eq!(canonical(&project(&reopened)), canonical(&imported));
}

fn reopen(path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(r.success, "{:?}", r.error);
    state
}

// ============================================================================
// Placement suggestions
// ============================================================================

/// An import that leaves a free piece beside a datum-held cave: the orphan
/// count names it, and its leaf end is offered onto the anchored line whose
/// shots it repeats.
#[test]
fn orphans_are_counted_and_offered_onto_anchored_lines() {
    let state = new_state();
    let walk = vec![
        shot(10.0, 8.0),
        shot(80.0, 6.0),
        shot(150.0, 9.0),
        shot(200.0, 7.0),
        shot(300.0, 5.0),
    ];
    let anchored = config(
        0,
        "Anchored",
        walk.clone(),
        ImportPlacement::ZeroDatum {
            coordinates: GpsCoordinates::new(20.5, -87.3, 0.0).unwrap(),
            station_index: 0,
        },
    );
    let orphan = config(1, "Orphan", walk, ImportPlacement::Origin);
    let r = import(&state, vec![anchored, orphan]);
    assert!(r.success, "{:?}", r.error);
    let ids = r.line_ids;
    assert_eq!(state.count_unanchored_lines(&ids).unwrap(), 1);
    assert_eq!(state.count_unanchored_lines(&ids[..1]).unwrap(), 0);

    let suggestions = state.suggest_orphan_placements(&ids).unwrap();
    assert!(
        suggestions
            .iter()
            .any(|s| s.orphan_line_id == ids[1] && s.target_line_id == ids[0]),
        "{suggestions:?}"
    );
    // Nothing newly imported, nothing offered.
    assert!(state.suggest_orphan_placements(&[]).unwrap().is_empty());

    let placements = state
        .suggest_import_placement(&[shot(80.0, 6.0), shot(150.0, 9.0)], None)
        .unwrap();
    assert!(!placements.is_empty(), "an overlap with existing survey");
}

// ============================================================================
// Performance profiler (not a unit test - a measurement harness)
// ============================================================================
//
// Run with:
//   STALAGMIGHT_PROFILE_TML=/path/to/large-project.tml \
//     cargo test --release --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib \
//     import_commands_tests::profile_tml_import_full -- --nocapture --ignored
//
// The harness drives parse_tml -> convert_tml_to_import_configs ->
// execute_import_lines end-to-end on an empty in-memory project, installs a
// tracing subscriber so the import's timing markers land on stderr, and
// prints the wall clock of each phase.

#[test]
#[ignore = "profiling harness - run explicitly with STALAGMIGHT_PROFILE_TML=<path>"]
fn profile_tml_import_full() {
    use crate::parsers::tml as tml_parser;
    use std::time::Instant;

    let path = std::env::var("STALAGMIGHT_PROFILE_TML")
        .expect("set STALAGMIGHT_PROFILE_TML to the .tml file path");
    let _ = tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let t0 = Instant::now();
    let tml = tml_parser::parse_tml(Path::new(&path)).expect("parse_tml");
    let t1 = Instant::now();
    let (lines, closures, _warnings) =
        tml_parser::convert_tml_to_import_configs(&tml, false, false).expect("convert");
    let t2 = Instant::now();
    let state = AppState::new().unwrap();
    state.set_project_for_tests(Project::new("profile", None));
    let total_shots: usize = lines.iter().map(|s| s.shots.len()).sum();
    let (line_count, closure_count) = (lines.len(), closures.len());

    let t3 = Instant::now();
    let result = state.execute_import_lines(
        lines,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    let t4 = Instant::now();
    let ms = |a: Instant, b: Instant| (b - a).as_secs_f64() * 1000.0;
    eprintln!(
        "profile_tml_import_full: {line_count} lines, {total_shots} shots, {closure_count} \
         closures; parse {:.2} ms, convert {:.2} ms, execute_import_lines {:.2} ms; \
         success {}",
        ms(t0, t1),
        ms(t1, t2),
        ms(t3, t4),
        result.success
    );
    assert!(result.success, "{:?}", result.error);
}
