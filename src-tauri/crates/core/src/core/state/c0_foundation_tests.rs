//! The C0 foundation end to end: open (version 2 and legacy), the execute
//! pipeline, undo and redo in the session and after a reopen, the save with
//! its guard, the local-only durable save, the read-only gate, the wire
//! payloads, and the typed "not yet ported" refusals.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::survey::{LineMeta, Project, StartAt};
use crate::models::{ShotData, ShotUpdates};
use crate::persistence::stalx_v2::{self, Extras, Format, Mode};

fn shot(length: f64, azimuth: f64) -> ShotData {
    ShotData {
        length,
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        depth_in: 1.0,
        depth_out: 1.5,
        ..ShotData::default()
    }
}

/// A two-line survey: a main line and a branch on its station 2.
fn survey() -> Project {
    let mut p = Project::new("C0", None);
    let main = p
        .add_line(
            LineMeta {
                name: Some("Main".to_string()),
                ..LineMeta::default()
            },
            StartAt::New,
            vec![shot(10.0, 0.0), shot(12.0, 45.0), shot(8.0, 90.0)],
        )
        .unwrap();
    let start = p.line(main).unwrap().start;
    p.set_placement(start, 0.0, 0.0).unwrap();
    let junction = p.line(main).unwrap().stations().nth(2).unwrap();
    p.add_line(
        LineMeta {
            name: Some("Branch".to_string()),
            ..LineMeta::default()
        },
        StartAt::Station(junction),
        vec![shot(5.0, 180.0), shot(6.0, 200.0)],
    )
    .unwrap();
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    p
}

fn write_v2(dir: &Path, project: &Project, extras: &Extras) -> PathBuf {
    let path = dir.join("survey.stalx");
    let bytes = stalx_v2::encode(project, extras, Mode::Normal).unwrap();
    std::fs::write(&path, bytes).unwrap();
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

fn project_of(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

/// The project without the fields a step or a reopen may change by design:
/// `modified_at` (stamped by the command layer), `file_path`, session ids.
fn canonical(p: &Project) -> Vec<u8> {
    let mut p = p.clone();
    p.modified_at = p.created_at;
    p.file_path = None;
    stalx_v2::encode(&p, &Extras::default(), Mode::Normal).unwrap()
}

fn first_shot(p: &Project) -> (u32, u32) {
    let line = &p.lines()[0];
    (line.id, line.shots[0].id)
}

fn set_length(state: &AppState, length: f64) -> CommandResult {
    let (line_id, shot_id) = first_shot(&project_of(state));
    state.execute(Command::UpdateShot {
        line_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(length),
            ..ShotUpdates::default()
        },
    })
}

#[test]
fn an_edit_undoes_and_redoes_in_the_session_and_after_a_reopen() {
    let dir = TempDir::new().unwrap();
    let original = survey();
    let path = write_v2(dir.path(), &original, &Extras::default());
    let state = open(&path);
    assert_eq!(canonical(&project_of(&state)), canonical(&original));

    let r = set_length(&state, 11.25);
    assert!(r.success, "{:?}", r.error);
    let edited = project_of(&state);
    assert_eq!(edited.lines()[0].shots[0].length, 11.25);
    assert!(state.has_unsaved_changes());
    assert!(edited.modified_at > original.modified_at);
    let items = state.get_undo_stack();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].command_type, "update_shot");
    assert!(
        items[0].description.contains("Main"),
        "{}",
        items[0].description
    );
    assert!(items[0].preview_data.is_some());

    assert!(state.undo().success);
    assert_eq!(canonical(&project_of(&state)), canonical(&original));
    // Session shot ids come back exactly.
    assert_eq!(first_shot(&project_of(&state)), first_shot(&original));
    assert!(state.redo().success);
    assert_eq!(canonical(&project_of(&state)), canonical(&edited));

    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "{:?}", saved.error);
    assert!(!state.has_unsaved_changes());

    let reopened = open(&path);
    assert_eq!(canonical(&project_of(&reopened)), canonical(&edited));
    assert_eq!(reopened.get_undo_stack().len(), 1);
    assert!(!reopened.history_discarded());
    let r = reopened.undo();
    assert!(r.success, "{:?}", r.error);
    assert_eq!(canonical(&project_of(&reopened)), canonical(&original));
    assert!(reopened.redo().success);
    assert_eq!(canonical(&project_of(&reopened)), canonical(&edited));
}

#[test]
fn a_no_op_edit_records_nothing_and_keeps_the_redo_branch() {
    let dir = TempDir::new().unwrap();
    let path = write_v2(dir.path(), &survey(), &Extras::default());
    let state = open(&path);
    assert!(set_length(&state, 11.25).success);
    assert!(state.undo().success);
    assert!(state.can_redo());
    // The first shot's length is 10.0 again: setting it is a no-op.
    assert!(set_length(&state, 10.0).success);
    assert!(state.can_redo(), "a provable no-op keeps the redo branch");
    assert!(!state.can_undo());
}

#[test]
fn a_refused_edit_changes_nothing() {
    let dir = TempDir::new().unwrap();
    let original = survey();
    let path = write_v2(dir.path(), &original, &Extras::default());
    let state = open(&path);
    let r = set_length(&state, -3.0);
    assert!(!r.success);
    assert_eq!(canonical(&project_of(&state)), canonical(&original));
    assert!(!state.can_undo());
    assert!(!state.has_unsaved_changes());
}

#[test]
fn a_merge_resolves_slots_and_undoes() {
    let dir = TempDir::new().unwrap();
    let mut original = survey();
    let free = original
        .add_line(
            LineMeta {
                name: Some("Free".to_string()),
                ..LineMeta::default()
            },
            StartAt::New,
            vec![shot(4.0, 10.0)],
        )
        .unwrap();
    let free_start = original.line(free).unwrap().start;
    original.set_placement(free_start, 30.0, 0.0).unwrap();
    assert!(original.validate().is_empty());
    let path = write_v2(dir.path(), &original, &Extras::default());
    let state = open(&path);
    let main = original.lines()[0].id;
    let r = state.execute(Command::MergeStations {
        station_a: crate::models::StationRef::new(free, 0),
        station_b: crate::models::StationRef::new(main, 3),
    });
    assert!(r.success, "{:?}", r.error);
    let merged = project_of(&state);
    assert!(merged.validate().is_empty());
    assert!(merged.stations.len() < original.stations.len());
    assert!(state.undo().success);
    assert_eq!(canonical(&project_of(&state)), canonical(&original));
    // An out-of-range slot is refused, nothing changes.
    let r = state.execute(Command::MergeStations {
        station_a: crate::models::StationRef::new(free, 7),
        station_b: crate::models::StationRef::new(main, 3),
    });
    assert!(!r.success);
    assert_eq!(canonical(&project_of(&state)), canonical(&original));
}

#[test]
fn undo_to_steps_several_entries_and_reports_reversed_lines() {
    let dir = TempDir::new().unwrap();
    let original = survey();
    let path = write_v2(dir.path(), &original, &Extras::default());
    let state = open(&path);
    let line_id = original.lines()[0].id;
    assert!(set_length(&state, 11.0).success);
    let r = state.execute(Command::ReverseLine {
        line_id,
        swap_left_right: false,
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(r.reversed_lines.as_ref().map(Vec::len), Some(1));
    assert_eq!(state.get_undo_stack().len(), 2);
    let r = state.undo_to(1);
    assert!(r.success, "{:?}", r.error);
    assert_eq!(r.reversed_lines.as_ref().map(Vec::len), Some(1));
    assert_eq!(canonical(&project_of(&state)), canonical(&original));
    assert_eq!(state.get_redo_stack().len(), 2);
    assert!(state.redo_to(1).success);
    assert!(!state.can_redo());
}

#[test]
fn a_save_refuses_a_file_changed_on_disk_until_told_to_overwrite() {
    let dir = TempDir::new().unwrap();
    let path = write_v2(dir.path(), &survey(), &Extras::default());
    let state = open(&path);
    assert!(set_length(&state, 11.25).success);
    let mut other = std::fs::read(&path).unwrap();
    other.extend_from_slice(b"\n");
    std::fs::write(&path, &other).unwrap();
    let r = state.execute(Command::SaveProject { path: None });
    assert!(!r.success);
    assert!(
        r.error
            .as_deref()
            .unwrap_or_default()
            .starts_with(super::EXTERNALLY_MODIFIED_CODE),
        "{:?}",
        r.error
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        other,
        "the file is untouched"
    );
    let r = state.save_project_overwriting(None);
    assert!(r.success, "{:?}", r.error);
    let reopened = open(&path);
    assert_eq!(project_of(&reopened).lines()[0].shots[0].length, 11.25);
}

#[test]
fn a_display_setting_is_durable_at_once_and_leaves_the_survey_bytes_alone() {
    let dir = TempDir::new().unwrap();
    let path = write_v2(dir.path(), &survey(), &Extras::default());
    let state = open(&path);
    state.save_display_setting("showGrid", "true").unwrap();
    let decoded = stalx_v2::decode(&std::fs::read(&path).unwrap(), &Default::default()).unwrap();
    assert_eq!(
        decoded
            .extras
            .local
            .display
            .get("showGrid")
            .map(String::as_str),
        Some("true")
    );
    // And the next normal save still passes the guard.
    assert!(set_length(&state, 11.25).success);
    let r = state.execute(Command::SaveProject { path: None });
    assert!(r.success, "{:?}", r.error);
}

#[test]
fn a_file_with_an_unknown_required_feature_opens_read_only() {
    let dir = TempDir::new().unwrap();
    let path = write_v2(dir.path(), &survey(), &Extras::default());
    let text = std::fs::read_to_string(&path).unwrap();
    let patched = text.replacen("\"requires\":[]", "\"requires\":[\"x-from-the-future\"]", 1);
    assert_ne!(patched, text);
    std::fs::write(&path, patched.as_bytes()).unwrap();
    let state = open(&path);
    let r = set_length(&state, 11.25);
    assert!(!r.success);
    assert!(r
        .error
        .as_deref()
        .unwrap_or_default()
        .starts_with(super::READ_ONLY_FILE_CODE));
    let r = state.execute(Command::SaveProject { path: None });
    assert!(!r.success);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), patched);
}

/// A file that requires an unknown feature and breaks an invariant without it
/// gets the "saved by a newer Stalagmight" refusal (`NEWER_SCHEMA`, no
/// repair offer), never the corrupt route (review finding C-file-format-2).
/// The same file without the feature is still corrupt. Injection: mapping
/// `DecodeError::NewerFeatures` to `LoadError::corrupt` fails the first
/// assertion.
#[test]
fn a_rule_broken_under_an_unknown_feature_is_refused_as_newer_not_corrupt() {
    let dir = TempDir::new().unwrap();
    let path = write_v2(dir.path(), &survey(), &Extras::default());
    let text = std::fs::read_to_string(&path).unwrap();
    // A station on no line: an invariant a future feature could relax.
    let station_counter = "\"counters\":{\"station\":";
    let at = text.find(station_counter).unwrap() + station_counter.len();
    let end = at + text[at..].find(',').unwrap();
    let next: u32 = text[at..end].parse().unwrap();
    let broken = format!(
        "{}{}{}",
        &text[..at],
        next + 1,
        &text[end..].replacen(
            "\n],\n\"lines\":",
            &format!(",\n[{next},\"Off line\"]\n],\n\"lines\":"),
            1
        )
    );
    assert!(broken.contains("\"Off line\""));
    let newer = broken.replacen("\"requires\":[]", "\"requires\":[\"x-from-the-future\"]", 1);

    std::fs::write(&path, newer.as_bytes()).unwrap();
    let state = AppState::new().unwrap();
    let error = state.read_project_file(&path).err().unwrap();
    assert_eq!(error.code, "NEWER_SCHEMA", "{}", error.message);
    assert!(
        error.message.contains("newer version of Stalagmight")
            && error.message.contains("x-from-the-future"),
        "{}",
        error.message
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);

    std::fs::write(&path, broken.as_bytes()).unwrap();
    let error = state.read_project_file(&path).err().unwrap();
    assert_eq!(error.code, "CORRUPT", "{}", error.message);
}

/// A file whose closure counter is at the format's integer bound opens (its
/// closure ids are used up), and a new loop closure is refused instead of
/// leaving a project no save can write (review finding C-file-format-6).
/// Injection: without the closure bounds in `validate` (the id and the
/// counter), the closure is created and every later save fails with "above
/// the largest integer".
#[test]
fn a_closure_past_the_id_space_is_refused_and_saves_keep_working() {
    let dir = TempDir::new().unwrap();
    let mut p = survey();
    p.counters.next_closure_id = crate::models::survey::MAX_CLOSURE_ID;
    let path = write_v2(dir.path(), &p, &Extras::default());
    let state = open(&path);
    let main = project_of(&state).lines()[0].id;
    let branch = project_of(&state).lines()[1].id;
    let r = state.execute(Command::CreateLoopClosure {
        station_a: crate::models::StationRef::new(main, 3),
        station_b: crate::models::StationRef::new(branch, 2),
        closure_strategy: crate::models::ClosureStrategy::Batch,
    });
    assert!(!r.success, "a closure id past the bound was minted");
    assert!(project_of(&state).closures.is_empty());
    assert!(set_length(&state, 11.25).success);
    let r = state.execute(Command::SaveProject { path: None });
    assert!(r.success, "{:?}", r.error);
}

#[test]
fn the_project_state_and_the_binary_envelope_are_the_wire_projection() {
    let dir = TempDir::new().unwrap();
    let original = survey();
    let path = write_v2(dir.path(), &original, &Extras::default());
    let state = open(&path);
    let wire = state.project_state_wire().unwrap();
    let expected = project_of(&state).project_state_wire(false);
    assert_eq!(
        serde_json::to_value(&wire).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    let input = project_of(&state).load_envelope_input(false);
    let result = crate::core::loading::LoadProjectFullResult::from_envelope(
        1,
        input,
        Vec::new(),
        crate::core::loading::DisplaySettings::default(),
        false,
    );
    let bytes = crate::core::load_binary::encode(result).unwrap();
    assert!(!bytes.is_empty());
}

#[test]
fn a_new_project_is_written_as_version_2() {
    let dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Fresh".to_string(),
                location: None,
            })
            .success
    );
    let path = dir.path().join("fresh.stalx");
    let r = state.execute(Command::SaveProject {
        path: Some(path.clone()),
    });
    assert!(r.success, "{:?}", r.error);
    assert_eq!(stalx_v2::sniff(&std::fs::read(&path).unwrap()), Format::V2);
}

/// The tracked legacy fixture (e2e/fixtures), copied into a temp dir.
fn legacy_copy(dir: &Path) -> PathBuf {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../e2e/fixtures/fractal_plant_iter3.stalx");
    let path = dir.join("legacy.stalx");
    std::fs::copy(&fixture, &path).unwrap();
    path
}

#[test]
fn a_legacy_file_opens_in_memory_and_its_first_save_upgrades_it_with_a_backup() {
    crate::app_dirs::init_for_test();
    let dir = TempDir::new().unwrap();
    let path = legacy_copy(dir.path());
    let original_bytes = std::fs::read(&path).unwrap();
    assert_eq!(stalx_v2::sniff(&original_bytes), Format::Legacy);
    let state = open(&path);
    let opened = project_of(&state);
    assert!(opened.validate().is_empty());
    assert!(!opened.lines().is_empty());
    // Nothing is written by the open, nor by a display toggle.
    state.save_display_setting("showGrid", "true").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original_bytes);

    let r = state.execute(Command::SaveProject { path: None });
    assert!(r.success, "{:?}", r.error);
    let upgraded = std::fs::read(&path).unwrap();
    assert_eq!(stalx_v2::sniff(&upgraded), Format::V2);
    let decoded = stalx_v2::decode(&upgraded, &Default::default()).unwrap();
    assert_eq!(
        decoded
            .extras
            .local
            .display
            .get("showGrid")
            .map(String::as_str),
        Some("true"),
        "the in-memory display toggle rides the upgrading save"
    );
    let backups = super::project_io::upgrade_backup_dir().unwrap();
    let kept = std::fs::read_dir(&backups)
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| std::fs::read(e.path()).ok().as_deref() == Some(original_bytes.as_slice()));
    assert!(kept, "the SQLite original is kept in {}", backups.display());
    // The upgraded file reopens to the same survey.
    let reopened = open(&path);
    assert_eq!(canonical(&project_of(&reopened)), canonical(&opened));
}
