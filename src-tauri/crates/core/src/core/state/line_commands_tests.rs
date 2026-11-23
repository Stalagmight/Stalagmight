//! The line commands through the execute pipeline (ported from the 0.6.0
//! `state/tests.rs` sections "Line Operations Tests", "Line deletion cleanup
//! undo/redo tests" and "Delete line cascade tests"), plus the station-graph
//! rules the port added: 0.6.0's line naming on Create Line, the Section a
//! new line joins, the batch result fields, the UI name refusal.
//!
//! Several 0.6.0 tests in those sections exercised another family's command
//! (unmerge, move station, delete-and-split, insert shot) for what its undo
//! restores. They run that family's model operation through the same
//! pipeline (`run_recorded`, via [`run_model`]), so what they pin - the
//! recorded changeset puts every side effect back - holds whichever package
//! ports the command's `apply`.

use std::path::Path;

use tempfile::TempDir;

use super::commands::Record;
use super::AppState;
use crate::core::{Command, CommandResult, ReversedLine};
use crate::models::survey::{ClosureStrategy, Project, RangeDeleteMode, ShotPlace};
use crate::models::{
    Coordinates, CreditEdit, CreditTargets, GpsCoordinates, LineId, LineUpdates, Person, ShotData,
    StationId, StationRef, SurveyCredit, SurveyDirection,
};

/// 0.6.0's `test_shot_data`.
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

/// A level shot on a fixed heading.
fn heading_shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: (azimuth + 180.0) % 360.0,
        length,
        depth_in: 10.0,
        depth_out: 10.0,
        ..Default::default()
    }
}

fn new_state() -> AppState {
    let state = AppState::new().unwrap();
    let r = state.execute(Command::CreateProject {
        name: "Lines".to_string(),
        location: None,
    });
    assert!(r.success, "{:?}", r.error);
    state
}

fn create_line_command(name: &str, shots: Vec<ShotData>) -> Command {
    Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#111111".to_string(),
        direction: Some(SurveyDirection::In),
        shots,
        cave_section_id: None,
    }
}

fn create(state: &AppState, name: &str, shots: Vec<ShotData>) -> LineId {
    let r = state.execute(create_line_command(name, shots));
    assert!(r.success, "{:?}", r.error);
    r.line_id.unwrap()
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

/// Change the open project directly (a fixture the test does not undo).
fn edit(state: &AppState, f: impl FnOnce(&mut Project)) {
    let mut p = project(state);
    f(&mut p);
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    state.set_project_for_tests(p);
}

/// Run a model operation through the execute pipeline as one recorded edit,
/// as the command family that owns it does.
fn run_model(
    state: &AppState,
    kind: &str,
    f: impl FnOnce(&mut Project) -> Result<(), String>,
) -> CommandResult {
    let recorded = state.run_recorded(
        |_| Record {
            kind: kind.to_string(),
            label: kind.to_string(),
            ..Record::default()
        },
        |p, _| f(p),
    );
    match recorded {
        Ok(()) => CommandResult::success(),
        Err(failure) => failure,
    }
}

fn gps(latitude: f64, longitude: f64) -> GpsCoordinates {
    GpsCoordinates::new(latitude, longitude, 0.0).unwrap()
}

fn set_datum(state: &AppState, slot: StationRef, at: GpsCoordinates) {
    edit(state, |p| {
        p.set_datum_at(slot, at, None).unwrap();
    });
}

fn station(p: &Project, line: LineId, index: u32) -> StationId {
    p.station_at(StationRef::new(line, index)).unwrap()
}

fn xyz(p: &Project, line: LineId, index: u32) -> [f64; 3] {
    p.raw_positions()
        .slot(p, StationRef::new(line, index))
        .unwrap()
}

fn same_place(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9)
}

/// The project as a save would write it, without the fields an edit, an
/// undo or a reopen may change by design: the edit time, the path, and the
/// id counters (an undo leaves them standing, so ids are never reused).
fn canonical(p: &Project) -> String {
    let mut p = p.clone();
    p.modified_at = p.created_at;
    p.file_path = None;
    p.counters.next_station_id = 1 << 30;
    p.counters.next_closure_id = 1 << 40;
    p.counters.next_section_id = 1 << 30;
    let bytes = crate::persistence::stalx_v2::encode(
        &p,
        &crate::persistence::stalx_v2::Extras::default(),
        crate::persistence::stalx_v2::Mode::Normal,
    )
    .unwrap();
    String::from_utf8(bytes).unwrap()
}

fn save_to(state: &AppState, path: &Path) {
    let r = state.execute(Command::SaveProject {
        path: Some(path.to_path_buf()),
    });
    assert!(r.success, "{:?}", r.error);
}

fn open(path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    let r = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(r.success, "{:?}", r.error);
    state
}

// === Create Line ===

#[test]
fn test_create_line() {
    let state = new_state();
    let result = state.execute(Command::CreateLine {
        name: Some("Line A".to_string()),
        surveyor: Some("John".to_string()),
        date: None,
        color: "#4ecdc4".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![shot(5.0), shot(7.0)],
        cave_section_id: None,
    });
    assert!(result.success, "{:?}", result.error);
    let line_id = result.line_id.unwrap();

    let p = project(&state);
    assert_eq!(p.lines().len(), 1);
    let line = p.line(line_id).unwrap();
    assert_eq!(line.name.as_deref(), Some("Line A"));
    assert_eq!(line.shots.len(), 2);
    assert_eq!(line.color, "#4ecdc4");
    // The legacy surveyor is the shots' credit (0.6.0's `Line::new`), not a
    // line field, once the line has shots.
    assert_eq!(line.legacy_surveyor, None);
    for s in &line.shots {
        assert_eq!(
            s.credit.as_deref(),
            Some(&SurveyCredit {
                surveyors: vec![Person::named("John").unwrap()],
                ..SurveyCredit::default()
            })
        );
    }
    // Station 0 of a new free line sits at the local origin.
    let [x, y, _] = xyz(&p, line_id, 0);
    assert_eq!((x, y), (0.0, 0.0));
    assert_eq!(state.get_undo_stack().len(), 1);
}

#[test]
fn test_create_line_returns_id() {
    let state = new_state();
    let first = state.execute(Command::CreateLine {
        name: Some("First".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
        cave_section_id: None,
    });
    assert!(first.success, "{:?}", first.error);
    assert_eq!(first.line_id, Some(0));

    let second = state.execute(Command::CreateLine {
        name: Some("Second".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![],
        cave_section_id: None,
    });
    assert!(second.success, "{:?}", second.error);
    assert_eq!(second.line_id, Some(1));
}

#[test]
fn test_create_line_no_project() {
    let state = AppState::new().unwrap();
    let result = state.execute(create_line_command("Orphan", vec![]));
    assert!(!result.success);
    assert!(result.error.is_some());
}

#[test]
fn create_line_rejects_non_finite_geometry() {
    let state = new_state();
    let mut bad = shot(5.0);
    bad.length = f64::NAN;
    let r = state.execute(Command::CreateLine {
        name: None,
        surveyor: None,
        date: None,
        color: "#ffffff".to_string(),
        direction: None,
        shots: vec![bad],
        cave_section_id: None,
    });
    assert!(!r.success, "create_line must reject non-finite geometry");
    assert_eq!(
        r.error.as_deref(),
        Some("Line has a shot with a non-finite (NaN/inf) value")
    );
    assert!(project(&state).lines().is_empty(), "nothing committed");
    assert!(state.get_undo_stack().is_empty());
}

#[test]
fn create_line_refuses_zero_and_negative_lengths_naming_the_shot() {
    let state = new_state();
    let r = state.execute(create_line_command("Main", vec![shot(5.0), shot(0.0)]));
    assert_eq!(
        r.error.as_deref(),
        Some("Shot Main: 1 \u{2192} 2 has zero length")
    );
    let r = state.execute(Command::CreateLine {
        name: None,
        surveyor: None,
        date: None,
        color: "#ffffff".to_string(),
        direction: None,
        shots: vec![shot(-2.0)],
        cave_section_id: None,
    });
    assert_eq!(
        r.error.as_deref(),
        Some("Shot New line: 0 \u{2192} 1 has negative length: -2.0m")
    );
    assert!(project(&state).lines().is_empty());
}

/// 0.6.0's normalization named every unnamed line `Line n` and made a
/// repeated name unique with ` (k)`; the IPC refuses a user's duplicate
/// before the command runs, so only the automatic paths (a batch, the
/// harness) reach this.
#[test]
fn a_created_line_is_named_and_made_unique_as_0_6_0_named_it() {
    let state = new_state();
    let unnamed = |state: &AppState| {
        let r = state.execute(Command::CreateLine {
            name: Some("   ".to_string()),
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: None,
            shots: vec![shot(1.0)],
            cave_section_id: None,
        });
        assert!(r.success, "{:?}", r.error);
        r.line_id.unwrap()
    };
    let first = unnamed(&state);
    let second = unnamed(&state);
    let main = create(&state, "  Main  ", vec![shot(1.0)]);
    let main_again = create(&state, "Main", vec![shot(1.0)]);
    let main_third = create(&state, "Main", vec![shot(1.0)]);
    let p = project(&state);
    let name = |id: LineId| p.line(id).unwrap().name.clone().unwrap();
    assert_eq!(name(first), "Line 1");
    assert_eq!(name(second), "Line 2");
    assert_eq!(name(main), "Main");
    assert_eq!(name(main_again), "Main (2)");
    assert_eq!(name(main_third), "Main (3)");
}

#[test]
fn the_ui_refuses_a_new_line_name_that_is_empty_or_taken() {
    let state = new_state();
    assert!(state.new_line_name_refusal(Some("Main")).is_none());
    create(&state, "Main", vec![shot(1.0)]);
    let taken = state.new_line_name_refusal(Some(" Main ")).unwrap();
    assert_eq!(
        taken.error.as_deref(),
        Some("A line named \"Main\" already exists")
    );
    let empty = state.new_line_name_refusal(None).unwrap();
    assert_eq!(empty.error.as_deref(), Some("A line name cannot be empty"));
    assert!(AppState::new()
        .unwrap()
        .new_line_name_refusal(Some("Main"))
        .is_none());
}

#[test]
fn create_line_reports_validation_warnings_with_the_shots_labels() {
    let state = new_state();
    // A depth change longer than the shot itself trips the length check.
    let steep = ShotData {
        depth_in: 0.0,
        depth_out: 9.0,
        ..heading_shot(0.0, 2.0)
    };
    let r = state.execute(create_line_command(
        "Main",
        vec![
            ShotData {
                azimuth_out: 0.0,
                ..heading_shot(0.0, 5.0)
            },
            ShotData {
                azimuth_out: 0.0,
                ..steep
            },
        ],
    ));
    assert!(r.success, "{:?}", r.error);
    let codes: Vec<(&str, &str)> = r
        .warnings
        .iter()
        .map(|w| (w.code.as_str(), w.message.as_str()))
        .collect();
    assert_eq!(codes.len(), 1, "{codes:?}");
    assert_eq!(codes[0].0, "LENGTH_BELOW_DEPTH_CHANGE");
    assert!(
        codes[0].1.starts_with("Shot Main: 1 \u{2192} 2 "),
        "{codes:?}"
    );
}

/// Reverse Line's warnings name each station as the line shows it (RS 4.1):
/// Side's station 2 is kept by a merge with Main's station 1, so Main shows
/// the junction as "2" and its own station 2 took the letter (rule L).
/// Injection: naming the junction by Main's archived record fails it with
/// "Main: 2a \u{2192} 1".
#[test]
fn reverse_line_warnings_name_a_junction_as_the_line_shows_it() {
    use super::topology_kit::{deep, empty, line_at, open, st};
    let mut p = empty("Reverse");
    let main = line_at(
        &mut p,
        "Main",
        [0.0, 0.0],
        vec![heading_shot(0.0, 5.0), deep(0.0, 2.0, 0.0, 9.0)],
    );
    let side = line_at(
        &mut p,
        "Side",
        [40.0, 40.0],
        vec![heading_shot(90.0, 4.0), heading_shot(90.0, 4.0)],
    );
    let j = st(&p, side, 2);
    p.merge_keeping_stationary(st(&p, main, 1), j).unwrap();
    assert_eq!(p.shown_name(j), Some("2"));
    assert_eq!(p.shown_name(st(&p, main, 2)), Some("2a"));
    let opened = open(&p);
    let r = opened.state.execute(Command::ReverseLine {
        line_id: main,
        swap_left_right: false,
    });
    assert!(r.success, "{:?}", r.error);
    let messages: Vec<&str> = r
        .warnings
        .iter()
        .filter(|w| w.code == "LENGTH_BELOW_DEPTH_CHANGE")
        .map(|w| w.message.as_str())
        .collect();
    assert_eq!(messages.len(), 1, "{:?}", r.warnings);
    assert!(
        messages[0].starts_with("Shot Main: 2a \u{2192} 2 "),
        "{messages:?}"
    );
}

#[test]
fn the_shots_uuids_are_minted_by_the_app() {
    let state = new_state();
    let foreign = ShotData {
        uuid: Some("caller-chosen".to_string()),
        ..shot(3.0)
    };
    let id = create(&state, "Main", vec![foreign]);
    let p = project(&state);
    let uuid = &p.line(id).unwrap().shots[0].uuid;
    assert_ne!(uuid, "caller-chosen", "#423: identity is not the caller's");
    assert!(!uuid.is_empty());
}

// === Delete Line ===

#[test]
fn test_delete_line() {
    let state = new_state();
    let line_id = create(&state, "To Delete", vec![shot(3.0)]);
    let result = state.execute(Command::DeleteLine { line_id });
    assert!(result.success, "{:?}", result.error);
    let p = project(&state);
    assert!(p.lines().is_empty());
    assert!(p.stations.is_empty(), "its stations went with it");
    assert!(p.placements.is_empty());
}

#[test]
fn test_delete_nonexistent_line() {
    let state = new_state();
    let result = state.execute(Command::DeleteLine { line_id: 999 });
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some(crate::models::LINE_GONE));
    assert!(state.get_undo_stack().is_empty());
}

/// Two lines joined at A1 = B0 (a junction: one station on both lines).
fn joined_pair(state: &AppState) -> (LineId, LineId) {
    let a = create(state, "A", vec![shot(5.0)]);
    let b = create(state, "B", vec![shot(3.0)]);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "{:?}", merged.error);
    (a, b)
}

#[test]
fn test_delete_line_cascades_merges() {
    let state = new_state();
    let (a, b) = joined_pair(&state);
    let junction = station(&project(&state), b, 0);
    assert_eq!(project(&state).merge_pairs().len(), 1);

    let r = state.execute(Command::DeleteLine { line_id: a });
    assert!(r.success, "{:?}", r.error);

    let p = project(&state);
    assert_eq!(p.lines().len(), 1);
    assert!(p.merge_pairs().is_empty(), "the junction left with line A");
    // The junction station stays for B, where it is B's station 0.
    assert_eq!(station(&p, b, 0), junction);
}

#[test]
fn test_delete_line_cascades_closures_and_offsets() {
    let state = new_state();
    let a = create(&state, "A", vec![shot(5.0), shot(3.0)]);
    let b = create(&state, "B", vec![shot(4.0)]);
    // A datum on A1 (not station 0, so the datum actually displaces A).
    set_datum(&state, StationRef::new(a, 1), gps(45.0, 10.0));
    let p = project(&state);
    assert_eq!(p.datums.len(), 1);
    let a_stations: Vec<StationId> = p.line(a).unwrap().stations().collect();

    let r = state.execute(Command::DeleteLine { line_id: a });
    assert!(r.success, "{:?}", r.error);

    let p = project(&state);
    assert_eq!(p.lines().len(), 1);
    assert_eq!(p.lines()[0].id, b);
    assert!(p.datums.is_empty(), "the datum went with its station");
    assert!(
        p.placements
            .iter()
            .all(|pl| !a_stations.contains(&pl.station)),
        "no placement names a deleted station: {:?}",
        p.placements
    );
    assert!(a_stations.iter().all(|s| !p.stations.contains_key(s)));
}

#[test]
fn test_delete_line_undo_restores_merges_and_datums() {
    let state = new_state();
    let (a, _b) = joined_pair(&state);
    set_datum(&state, StationRef::new(a, 0), gps(45.0, 10.0));
    let before = project(&state);

    assert!(state.execute(Command::DeleteLine { line_id: a }).success);
    let p = project(&state);
    assert_eq!(p.lines().len(), 1);
    assert!(p.merge_pairs().is_empty());
    assert!(p.datums.is_empty());

    let undo = state.undo();
    assert!(undo.success, "{:?}", undo.error);
    let p = project(&state);
    assert_eq!(p.lines().len(), 2, "undo restores the deleted line");
    assert_eq!(p.merge_pairs().len(), 1, "undo restores the junction");
    assert_eq!(p.datums.len(), 1, "undo restores the datum");
    assert_eq!(canonical(&p), canonical(&before));
}

#[test]
fn test_delete_line_undo_restores_relationships() {
    let state = new_state();
    let (a, _b) = joined_pair(&state);
    set_datum(&state, StationRef::new(a, 0), gps(45.0, -73.0));
    // A legacy hand move on A0 (only migrated files carry them).
    edit(&state, |p| {
        let s = station(p, a, 0);
        p.line_mut(a).unwrap().hand_moves.insert(s, [1.0, 2.0, 0.0]);
    });
    let before = project(&state);
    assert_eq!(before.merge_pairs().len(), 1);
    assert_eq!(before.datums.len(), 1);
    assert_eq!(before.line(a).unwrap().hand_moves.len(), 1);

    let r = state.execute(Command::DeleteLine { line_id: a });
    assert!(r.success, "{:?}", r.error);
    let gone = project(&state);
    assert!(gone.line(a).is_none());
    assert!(gone.merge_pairs().is_empty());
    assert!(gone.datums.is_empty());
    assert!(gone.lines().iter().all(|l| l.hand_moves.is_empty()));

    let undo = state.undo();
    assert!(undo.success, "{:?}", undo.error);
    assert_eq!(canonical(&project(&state)), canonical(&before));

    let redo = state.redo();
    assert!(redo.success, "{:?}", redo.error);
    assert_eq!(canonical(&project(&state)), canonical(&gone));
}

/// 0.6.0's "a snapshot restore is bit exact" (#413 finding 25), for the
/// station graph: a deleted line's shots come back to the bit, in the
/// session and through the history a save persisted.
#[test]
fn a_deleted_line_comes_back_bit_exact() {
    // Doubles whose shortest round-trip decimal a best-effort parser misreads.
    let pitch = -26.759_999_999_999_998_f64;
    let length = 26.759_999_999_999_998_f64;
    let state = new_state();
    let a = create(
        &state,
        "A",
        vec![ShotData {
            azimuth_in: 12.0,
            azimuth_out: 12.0,
            length,
            depth_in: 1.0,
            depth_out: 2.0,
            pitch_out: Some(pitch),
            ..Default::default()
        }],
    );
    let bits = |p: &Project| {
        let s = &p.line(a).unwrap().shots[0];
        (s.length.to_bits(), s.pitch_out.map(f64::to_bits))
    };
    let expected = (length.to_bits(), Some(pitch.to_bits()));
    assert_eq!(bits(&project(&state)), expected, "fixture");

    assert!(state.execute(Command::DeleteLine { line_id: a }).success);
    assert!(state.undo().success);
    assert_eq!(bits(&project(&state)), expected, "undo in the session");

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("bits.stalx");
    save_to(&state, &path);
    let reopened = open(&path);
    assert!(reopened.redo().success, "the delete is on the redo stack");
    assert!(project(&reopened).line(a).is_none());
    assert!(reopened.undo().success);
    assert_eq!(
        bits(&project(&reopened)),
        expected,
        "undo through the persisted entry"
    );
}

// === Update Line Metadata ===

#[test]
fn test_update_line_metadata() {
    let state = new_state();
    let line_id = create(&state, "Original", vec![]);
    let result = state.execute(Command::UpdateLineMetadata {
        line_id,
        updates: LineUpdates {
            name: Some(Some("Updated".to_string())),
            color: Some("#00ff00".to_string()),
            ..Default::default()
        },
    });
    assert!(result.success, "{:?}", result.error);
    let p = project(&state);
    assert_eq!(p.lines()[0].name.as_deref(), Some("Updated"));
    assert_eq!(p.lines()[0].color, "#00ff00");

    assert!(state.undo().success);
    let p = project(&state);
    assert_eq!(p.lines()[0].name.as_deref(), Some("Original"));
    assert_eq!(p.lines()[0].color, "#111111");
}

#[test]
fn a_rename_is_refused_in_0_6_0_words_and_changes_nothing() {
    let state = new_state();
    let a = create(&state, "A", vec![shot(1.0)]);
    create(&state, "B", vec![shot(1.0)]);
    let steps = state.get_undo_stack().len();
    let rename = |name: Option<String>| Command::UpdateLineMetadata {
        line_id: a,
        updates: LineUpdates {
            name: Some(name),
            color: Some("#abcdef".to_string()),
            ..Default::default()
        },
    };
    let taken = state.execute(rename(Some("B".to_string())));
    assert_eq!(
        taken.error.as_deref(),
        Some("A line named \"B\" already exists")
    );
    let empty = state.execute(rename(Some("  ".to_string())));
    assert_eq!(empty.error.as_deref(), Some("A line name cannot be empty"));
    let cleared = state.execute(rename(None));
    assert_eq!(
        cleared.error.as_deref(),
        Some("A line name cannot be empty")
    );
    assert_eq!(project(&state).line(a).unwrap().color, "#111111");
    assert_eq!(state.get_undo_stack().len(), steps);

    // Keeping its own name (trimmed) is no collision.
    let same = state.execute(rename(Some(" A ".to_string())));
    assert!(same.success, "{:?}", same.error);
    let missing = state.execute(Command::UpdateLineMetadata {
        line_id: 77,
        updates: LineUpdates::default(),
    });
    assert_eq!(missing.error.as_deref(), Some(crate::models::LINE_GONE));
}

#[test]
fn a_metadata_edit_that_changes_nothing_records_nothing_and_keeps_redo() {
    let state = new_state();
    let a = create(&state, "A", vec![shot(1.0)]);
    assert!(
        state
            .execute(Command::UpdateLineMetadata {
                line_id: a,
                updates: LineUpdates {
                    color: Some("#222222".to_string()),
                    ..Default::default()
                },
            })
            .success
    );
    assert!(state.undo().success);
    let redo_before = state.get_redo_stack().len();
    let same = state.execute(Command::UpdateLineMetadata {
        line_id: a,
        updates: LineUpdates {
            color: Some("#111111".to_string()),
            ..Default::default()
        },
    });
    assert!(same.success, "{:?}", same.error);
    assert_eq!(state.get_redo_stack().len(), redo_before);
}

// === Batches ===

/// 0.6.0's `execute_batch` folded each sub-command's `line_id` into the
/// batch's `line_ids`; the batch itself names no `line_id`.
#[test]
fn a_batch_reports_every_created_line_in_line_ids() {
    let state = new_state();
    let r = state.execute(Command::Batch {
        commands: vec![
            create_line_command("A", vec![shot(1.0)]),
            create_line_command("B", vec![shot(2.0)]),
        ],
        label: None,
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(r.line_id, None);
    let p = project(&state);
    let ids: Vec<LineId> = p.lines().iter().map(|l| l.id).collect();
    assert_eq!(r.line_ids, ids);
    assert_eq!(state.get_undo_stack().len(), 1, "one step");
    assert!(state.undo().success);
    assert!(project(&state).lines().is_empty());
}

#[test]
fn a_failing_batch_member_rolls_the_whole_batch_back() {
    let state = new_state();
    let a = create(&state, "A", vec![shot(1.0)]);
    let before = project(&state);
    let r = state.execute(Command::Batch {
        commands: vec![
            Command::UpdateLineMetadata {
                line_id: a,
                updates: LineUpdates {
                    color: Some("#999999".to_string()),
                    ..Default::default()
                },
            },
            Command::DeleteLine { line_id: 404 },
        ],
        label: None,
    });
    assert!(!r.success);
    assert_eq!(canonical(&project(&state)), canonical(&before));
    assert_eq!(state.get_undo_stack().len(), 1);
}

#[test]
fn a_batch_reports_the_reverses_parity_reduced() {
    let state = new_state();
    let a = create(&state, "A", vec![shot(1.0), shot(2.0)]);
    let b = create(&state, "B", vec![shot(1.0)]);
    let reverse = |line_id| Command::ReverseLine {
        line_id,
        swap_left_right: false,
    };
    let r = state.execute(Command::Batch {
        commands: vec![reverse(a), reverse(b), reverse(a)],
        label: None,
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(
        r.reversed_lines,
        Some(vec![ReversedLine {
            line_id: b,
            shot_count: 1
        }])
    );
}

#[test]
fn credit_and_metadata_batch_into_one_step_with_their_lines() {
    let state = new_state();
    let a = create(&state, "A", vec![shot(1.0), shot(2.0)]);
    let r = state.execute(Command::Batch {
        commands: vec![
            Command::UpdateLineMetadata {
                line_id: a,
                updates: LineUpdates {
                    name: Some(Some("Renamed".to_string())),
                    ..Default::default()
                },
            },
            Command::EditShotCredit {
                targets: CreditTargets::Line { line_id: a },
                edit: CreditEdit {
                    add_surveyors: Some(vec![Person::named("Ann").unwrap()]),
                    ..Default::default()
                },
            },
        ],
        label: None,
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(r.line_ids, vec![a], "the credit edit's line");
    assert_eq!(state.get_undo_stack().len(), 2);
}

// === Derived views over the pipeline ===

#[test]
fn get_adjusted_stations_none_without_project() {
    let state = AppState::new().unwrap();
    assert!(state.get_adjusted_stations().is_none());
}

#[test]
fn get_adjusted_stations_follow_every_edit() {
    let state = new_state();
    create(&state, "A", vec![shot(5.0), shot(5.0)]);
    let adj1 = state.get_adjusted_stations().unwrap();
    let again = state.get_adjusted_stations().unwrap();
    assert_eq!(adj1.len(), again.len());
    assert_eq!(adj1.len(), 3, "two shots, three stations");

    create(&state, "B", vec![shot(5.0)]);
    let adj2 = state.get_adjusted_stations().unwrap();
    assert_eq!(adj2.len(), 5, "the adjusted view reflects the added line");
}

#[test]
fn distance_field_none_without_project() {
    let state = AppState::new().unwrap();
    assert!(state.distance_field(None).is_none());
}

#[test]
fn distance_field_from_origin_over_appstate() {
    let state = new_state();
    let line_id = create(&state, "A", vec![shot(5.0), shot(5.0)]);
    let origin = StationRef::new(line_id, 0);
    let field = state.distance_field(Some(&origin)).unwrap();
    assert_eq!(field.distances.len(), 3, "two shots, three stations");
    assert_eq!(field.station_refs.len(), 3);
    let oi = field
        .station_refs
        .iter()
        .position(|r| *r == origin)
        .unwrap();
    assert_eq!(field.distances[oi], Some(0.0));
    assert!(field.distances.iter().all(Option::is_some));
    assert!(field.max_reachable > 0.0);
}

#[test]
fn coordinates_new_checked_rejects_non_finite() {
    assert!(Coordinates::new_checked(1.0, 2.0, 3.0).is_ok());
    assert!(Coordinates::new_checked(f64::NAN, 0.0, 0.0).is_err());
    assert!(Coordinates::new_checked(0.0, f64::INFINITY, 0.0).is_err());
    assert!(Coordinates::new_checked(0.0, 0.0, f64::NEG_INFINITY).is_err());
}

// === Merges across closures ===

/// 0.6.0: a merge across an inert (unaligned) closure is the first rigid
/// link, so it is allowed; across stations already connected it is refused
/// (use a loop closure). On the station graph a closure between two separate
/// pieces needs both placed (invariant 7), so the inert case joins two
/// datum-held pieces (#741: the later datum's piece moves).
#[test]
fn merge_allowed_across_inert_closure_but_rejected_when_connected() {
    let state = new_state();
    let a = create(&state, "A", vec![shot(5.0)]);
    let b = create(&state, "B", vec![shot(5.0)]);
    set_datum(&state, StationRef::new(a, 0), gps(10.0, 20.0));
    set_datum(&state, StationRef::new(b, 0), gps(10.001, 20.0));
    edit(&state, |p| {
        p.create_closure_at(
            StationRef::new(a, 0),
            StationRef::new(b, 1),
            ClosureStrategy::Batch,
        )
        .unwrap();
    });
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    assert!(r.success, "merge across an inert closure: {:?}", r.error);
    assert!(project(&state).validate().is_empty());

    // A and B are now one piece: merging two of its stations again would
    // close a loop with a merge.
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
    });
    assert!(!r.success, "stations already connected must be refused");
}

// === What undo restores (other families' model operations) ===

/// A (A0..A2) with a datum on A0, B (B0, B1) hanging off A2, and a closure
/// A0 = B1 closing the loop.
fn loop_through_a_closure(state: &AppState) -> (LineId, LineId) {
    let a = create(state, "A", vec![shot(5.0), shot(5.0)]);
    let b = create(state, "B", vec![shot(5.0)]);
    set_datum(state, StationRef::new(a, 0), gps(10.0, 20.0));
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "{:?}", merged.error);
    edit(state, |p| {
        p.create_closure_at(
            StationRef::new(a, 0),
            StationRef::new(b, 1),
            ClosureStrategy::default(),
        )
        .unwrap();
    });
    (a, b)
}

/// 0.6.0's `unmerge_converts_orphaned_bridge_closure` and
/// `undo_unmerge_restores_converted_closure_and_position`: detaching B
/// strands it on the closure, which the sweep promotes (B moves onto A0);
/// undo puts the junction, the closure and B's place back exactly; redo
/// re-applies the promotion.
#[test]
fn undo_unmerge_restores_converted_closure_and_position() {
    let state = new_state();
    let (a, b) = loop_through_a_closure(&state);
    let before = project(&state);
    assert_eq!(before.closures.len(), 1);

    let r = run_model(&state, "unmerge_station_pair", |p| {
        p.unmerge_station_pair(StationRef::new(a, 2), StationRef::new(b, 0), crate::models::survey::NameAnswers::RuleL, None)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    assert!(r.success, "{:?}", r.error);
    let converted = project(&state);
    assert!(
        converted.closures.is_empty(),
        "the sole-bridge closure became a merge: {:?}",
        converted.closures
    );
    assert!(
        !same_place(xyz(&converted, b, 0), xyz(&before, b, 0)),
        "precondition: the promotion moves B"
    );

    let u = state.undo();
    assert!(u.success, "{:?}", u.error);
    let restored = project(&state);
    assert_eq!(canonical(&restored), canonical(&before));
    assert_eq!(restored.closures.len(), 1);
    assert_eq!(restored.merge_pairs().len(), 1, "no leftover cycle");
    assert!(same_place(xyz(&restored, b, 1), xyz(&before, b, 1)));

    let re = state.redo();
    assert!(re.success, "{:?}", re.error);
    assert_eq!(canonical(&project(&state)), canonical(&converted));
}

/// The singular form: detach A2's other line at the station.
#[test]
fn undo_unmerge_singular_restores_converted_closure_and_position() {
    let state = new_state();
    let (a, b) = loop_through_a_closure(&state);
    let before = project(&state);
    let r = run_model(&state, "unmerge_stations", |p| {
        p.unmerge_station(StationRef::new(b, 0), crate::models::survey::NameAnswers::RuleL, None)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    assert!(r.success, "{:?}", r.error);
    assert!(project(&state).closures.is_empty());
    let u = state.undo();
    assert!(u.success, "{:?}", u.error);
    let restored = project(&state);
    assert_eq!(canonical(&restored), canonical(&before));
    assert!(restored.validate().is_empty());
    assert!(same_place(xyz(&restored, a, 2), xyz(&before, a, 2)));
}

#[test]
fn test_unmerge_removes_only_specific_merge() {
    let state = new_state();
    let a = create(&state, "A", vec![shot(5.0)]);
    let b = create(&state, "B", vec![shot(3.0)]);
    let c = create(&state, "C", vec![shot(4.0)]);
    for (x, y) in [
        (StationRef::new(a, 1), StationRef::new(b, 0)),
        (StationRef::new(b, 1), StationRef::new(c, 0)),
    ] {
        let r = state.execute(Command::MergeStations {
            station_a: x,
            station_b: y,
        });
        assert!(r.success, "{:?}", r.error);
    }
    assert_eq!(project(&state).merge_pairs().len(), 2);

    let r = run_model(&state, "unmerge_stations", |p| {
        p.unmerge_station(StationRef::new(a, 1), crate::models::survey::NameAnswers::RuleL, None)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    assert!(r.success, "{:?}", r.error);
    let pairs = project(&state).merge_pairs();
    assert_eq!(pairs.len(), 1, "only A and B came apart: {pairs:?}");
    assert_eq!((pairs[0].0.line_id, pairs[0].1.line_id), (b, c));
}

#[test]
fn test_unmerge_undo_restores_merge() {
    let state = new_state();
    let (a, _b) = joined_pair(&state);
    let r = run_model(&state, "unmerge_stations", |p| {
        p.unmerge_station(StationRef::new(a, 1), crate::models::survey::NameAnswers::RuleL, None)
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    assert!(r.success, "{:?}", r.error);
    assert!(project(&state).merge_pairs().is_empty());
    assert!(state.undo().success);
    assert_eq!(project(&state).merge_pairs().len(), 1);
}

#[test]
fn undo_move_station_restores_pitch_and_adjacent_shots() {
    let state = new_state();
    let a = create(&state, "A", vec![shot(5.0), shot(5.0)]);
    let before = project(&state);
    let line_before = before.line(a).unwrap().clone();
    assert_eq!(line_before.shots[0].pitch_in, None);

    let moved_station = station(&before, a, 1);
    let r = run_model(&state, "move_station", |p| {
        p.move_station(moved_station, [2.0, 3.0, 1.5])
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    assert!(r.success, "{:?}", r.error);
    let moved = project(&state);
    assert_ne!(
        moved.line(a).unwrap().shots,
        line_before.shots,
        "precondition: the move rewrote the adjacent shots"
    );

    let u = state.undo();
    assert!(u.success, "{:?}", u.error);
    let restored = project(&state);
    // Every field of both adjacent shots, pitch included, to the bit.
    assert_eq!(restored.line(a).unwrap(), &line_before);
}

/// 0.6.0's `split_converts_orphaned_bridge_closure_immediately`: a
/// delete-and-split that strands a closure as a piece's only bridge promotes
/// it at once.
#[test]
fn split_converts_orphaned_bridge_closure_immediately() {
    let state = new_state();
    let a = create(&state, "A", vec![shot(5.0), shot(5.0), shot(5.0)]);
    let b = create(&state, "B", vec![shot(5.0)]);
    set_datum(&state, StationRef::new(a, 0), gps(10.0, 20.0));
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 3),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "{:?}", merged.error);
    edit(&state, |p| {
        p.create_closure_at(
            StationRef::new(a, 0),
            StationRef::new(b, 1),
            ClosureStrategy::default(),
        )
        .unwrap();
    });
    let mid = project(&state).line(a).unwrap().shots[1].id;
    let r = run_model(&state, "delete_shot_and_split", |p| {
        p.delete_shot_range(
            a,
            mid,
            mid,
            &RangeDeleteMode::Split {
                name_a: "A-1".to_string(),
                name_b: "A-2".to_string(),
            },
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    });
    assert!(r.success, "{:?}", r.error);
    let p = project(&state);
    assert!(
        p.closures.is_empty(),
        "the split promotes the orphaned closure at once: {:?}",
        p.closures
    );
    assert!(p.validate().is_empty());
}

#[test]
fn split_line_does_not_promote_a_bridge_closure_across_two_anchored_components() {
    let state = new_state();
    let mk = |name: &str, azimuth: f64| {
        create(
            &state,
            name,
            vec![
                heading_shot(azimuth, 5.0),
                heading_shot(azimuth, 5.0),
                heading_shot(azimuth, 5.0),
            ],
        )
    };
    let a = mk("A", 0.0);
    let b = mk("B", 90.0);
    let c = mk("C", 0.0);
    set_datum(&state, StationRef::new(a, 0), gps(10.0, 20.0));
    // B hangs off A by its LAST station.
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 3),
        station_b: StationRef::new(b, 3),
    });
    assert!(merged.success, "{:?}", merged.error);
    // C is a second, independently anchored survey, tied by a closure only.
    set_datum(&state, StationRef::new(c, 0), gps(10.001, 20.0));
    edit(&state, |p| {
        p.create_closure_at(
            StationRef::new(b, 2),
            StationRef::new(c, 0),
            ClosureStrategy::Batch,
        )
        .unwrap();
    });

    let mid = project(&state).line(b).unwrap().shots[1].id;
    let r = run_model(&state, "delete_shot_and_split", |p| {
        p.delete_shot_range(
            b,
            mid,
            mid,
            &RangeDeleteMode::Split {
                name_a: "B-1".to_string(),
                name_b: "B-2".to_string(),
            },
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    });
    assert!(r.success, "{:?}", r.error);

    let p = project(&state);
    assert_eq!(p.closures.len(), 1, "the bridge survives as a closure");
    assert_eq!(
        p.merge_pairs().len(),
        1,
        "only the A-B junction: {:?}",
        p.merge_pairs()
    );
    assert!(p
        .merge_pairs()
        .iter()
        .all(|(x, y)| x.line_id != c && y.line_id != c));
    assert_eq!(
        p.placement_datums().iter().flatten().count(),
        2,
        "each datum places its own piece"
    );
}

#[test]
fn redo_insert_shot_keeps_merged_stations_aligned() {
    let state = new_state();
    let a = create(&state, "A", vec![shot(5.0), shot(7.0)]);
    let b = create(&state, "B", vec![shot(5.0)]);
    set_datum(&state, StationRef::new(a, 0), gps(10.0, 20.0));
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "{:?}", merged.error);
    let original = project(&state);

    let after_id = original.line(a).unwrap().shots[0].id;
    let r = run_model(&state, "create_shot", |p| {
        p.create_shot(a, shot(6.0), ShotPlace::After(after_id))
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    assert!(r.success, "{:?}", r.error);
    let inserted = project(&state);
    // B stays on the junction, now A's station 3.
    assert_eq!(station(&inserted, a, 3), station(&inserted, b, 0));

    assert!(state.undo().success);
    assert_eq!(canonical(&project(&state)), canonical(&original));
    assert!(state.redo().success);
    let redone = project(&state);
    assert_eq!(canonical(&redone), canonical(&inserted));
    assert!(same_place(xyz(&redone, b, 1), xyz(&inserted, b, 1)));
}

// === Other families' refusals (their commands port elsewhere) ===

#[test]
fn move_line_and_segment_reject_non_finite_offset() {
    let state = new_state();
    let a = create(&state, "A", vec![shot(5.0)]);
    let nan = Coordinates::new(f64::NAN, 0.0, 0.0);
    assert!(
        !state
            .execute(Command::MoveLine {
                line_id: a,
                offset: nan
            })
            .success
    );
    assert!(
        !state
            .execute(Command::MoveSegment {
                line_id: a,
                offset: nan
            })
            .success
    );
    assert!(
        state
            .execute(Command::MoveSegment {
                line_id: a,
                offset: Coordinates::new(1.0, 2.0, 0.0),
            })
            .success
    );
}

#[test]
fn move_station_rejects_non_finite_offset() {
    let state = new_state();
    let a = create(&state, "A", vec![shot(5.0)]);
    let before = project(&state);
    let r = state.execute(Command::MoveStation {
        station_ref: StationRef::new(a, 1),
        offset: Coordinates::new(f64::INFINITY, 0.0, 0.0),
    });
    assert!(!r.success, "move_station must reject a non-finite offset");
    assert_eq!(canonical(&project(&state)), canonical(&before));
}

// === Import overlap (the import package's funnel; fixture-gated) ===

/// Import kb5.dmp lines with overlap treatment and verify the interpolated
/// shot values in the project state after import: line 002 at the origin,
/// then line 001 (overlap shots removed) with `existing_shot_updates` that
/// interpolate 002's overlap shots, all in one import call.
#[test]
#[ignore = "fixture-gated: needs e2e/fixtures/kb5.dmp, which the repo does not carry"]
fn kb5_import_overlap_interpolation_integration() {
    use crate::models::{
        ExistingShotUpdate, ImportLineConfig, ImportPlacement, ShotId, ShotUpdates,
    };
    use crate::parsers::dmp::{parse_dmp, DmpShot, DmpShotType};

    fn circular_mean_deg(a: f64, b: f64) -> f64 {
        let (a_r, b_r) = (a.to_radians(), b.to_radians());
        let mean = f64::atan2(
            f64::midpoint(a_r.sin(), b_r.sin()),
            f64::midpoint(a_r.cos(), b_r.cos()),
        )
        .to_degrees();
        if mean < 0.0 {
            mean + 360.0
        } else {
            mean
        }
    }

    fn interp_update(
        line_id: LineId,
        shot_id: ShotId,
        imp: &ShotData,
        ext: &ShotData,
    ) -> ExistingShotUpdate {
        // `ShotUpdates`' clearable fields: absent, or set to the mean.
        let avg_opt = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (None, None) => None,
            (Some(v), None) | (None, Some(v)) => Some(Some(v)),
            (Some(a), Some(b)) => Some(Some(f64::midpoint(a, b))),
        };
        ExistingShotUpdate {
            line_id,
            shot_id,
            updates: ShotUpdates {
                azimuth_in: Some(circular_mean_deg(imp.azimuth_in, ext.azimuth_in)),
                azimuth_out: Some(circular_mean_deg(imp.azimuth_out, ext.azimuth_out)),
                length: Some(f64::midpoint(imp.length, ext.length)),
                depth_in: Some(f64::midpoint(imp.depth_in, ext.depth_in)),
                depth_out: Some(f64::midpoint(imp.depth_out, ext.depth_out)),
                pitch_in: avg_opt(imp.pitch_in, ext.pitch_in),
                pitch_out: avg_opt(imp.pitch_out, ext.pitch_out),
                left: avg_opt(imp.left, ext.left),
                right: avg_opt(imp.right, ext.right),
                up: avg_opt(imp.up, ext.up),
                down: avg_opt(imp.down, ext.down),
                ..Default::default()
            },
        }
    }

    let config = |name: &str, color: &str, shots: Vec<ShotData>, placement| ImportLineConfig {
        cave_section: None,
        reverse: false,
        line_index: 0,
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: color.to_string(),
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
    };

    let dmp_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../e2e/fixtures/kb5.dmp");
    let dmp = parse_dmp(&dmp_path).unwrap();
    let standard = |index: usize| -> Vec<ShotData> {
        dmp.lines[index]
            .shots
            .iter()
            .filter(|s| s.shot_type == DmpShotType::Standard)
            .map(DmpShot::to_shot_data)
            .collect()
    };
    let shots_001 = standard(0);
    let shots_002 = standard(1);
    assert_eq!(shots_001.len(), 107);
    assert_eq!(shots_002.len(), 364);

    // Step 1: import line 002 at the origin.
    let state = new_state();
    let result_002 = state.execute_import_lines(
        vec![config(
            "002",
            "#4ecdc4",
            shots_002.clone(),
            ImportPlacement::Origin,
        )],
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        super::VirtualShotMode::Auto,
    );
    assert!(
        result_002.success,
        "import 002 failed: {:?}",
        result_002.error
    );
    let p = project(&state);
    let line_002_id = p.lines()[0].id;
    assert_eq!(p.lines()[0].shots.len(), 364);

    // Step 2: 001 -> 002 overlap: Back (impStn=107), Reverse, matched=2,
    // exStn=182. Import shots 105, 106 are deleted; 002 shots 180, 181 get
    // interpolated values.
    let shot_180_id = p.lines()[0].shots[180].id;
    let shot_181_id = p.lines()[0].shots[181].id;
    let existing_updates = vec![
        interp_update(line_002_id, shot_180_id, &shots_001[105], &shots_002[180]),
        interp_update(line_002_id, shot_181_id, &shots_001[106], &shots_002[181]),
    ];
    let mut treated = shots_001;
    treated.remove(106);
    treated.remove(105);
    assert_eq!(treated.len(), 105);
    let result_001 = state.execute_import_lines(
        vec![config(
            "001",
            "#ff6b6b",
            treated,
            ImportPlacement::FromExisting {
                import_station: 105,
                existing_station: StationRef::new(line_002_id, 182),
            },
        )],
        Vec::new(),
        None,
        Vec::new(),
        existing_updates,
        super::VirtualShotMode::Auto,
    );
    assert!(
        result_001.success,
        "import 001 failed: {:?}",
        result_001.error
    );

    // Step 3: read back.
    let p = project(&state);
    assert_eq!(p.lines().len(), 2);
    let sec_002 = p.line(line_002_id).unwrap();
    let sec_001 = p.lines().iter().find(|s| s.id != line_002_id).unwrap();
    assert_eq!(sec_001.shots.len(), 105);
    assert_eq!(sec_002.shots.len(), 364);
    let (actual_180, actual_181) = (&sec_002.shots[180], &sec_002.shots[181]);
    let eps = 0.01;
    assert!(
        (actual_180.length - 5.665).abs() < eps,
        "{}",
        actual_180.length
    );
    assert!(
        (actual_180.azimuth_in - 322.4).abs() < 0.5,
        "{}",
        actual_180.azimuth_in
    );
    assert!(
        (actual_181.length - 12.375).abs() < eps,
        "{}",
        actual_181.length
    );
    assert!(
        (actual_181.azimuth_in - 349.75).abs() < 0.5,
        "{}",
        actual_181.azimuth_in
    );
}
