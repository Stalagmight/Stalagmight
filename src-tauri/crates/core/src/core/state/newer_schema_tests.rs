//! [#576 B6-D1] A project saved by a NEWER Stalagmight opens read-only.
//!
//! The policy: a `.stalx` whose `PRAGMA user_version` is above the schema this
//! build knows opens with editing blocked, refuses every save, and is left
//! byte-for-byte as it was - including the undo history this build cannot vet.
//! Before it, the open deleted that history (`ensure_schema_version`), the
//! project opened editable with only a log line to show for it, and the first
//! save rewrote the file at THIS build's schema, silently dropping whatever the
//! newer build had stored.
//!
//! All data is synthetic: a project built in memory, saved by this build, then
//! re-stamped one schema version above it. No fixture is read or written.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tempfile::TempDir;

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::CommandErrorCode;
use crate::models::{Project, SectionData, ShotData, ShotUpdates};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

/// Words the refusal must carry, so a user (and a log reader) learns WHY the
/// edit was refused rather than the SpeleoDB "take the write lock" story.
const NEWER_FILE_WORDS: &str = "newer version of Stalagmight";

/// One persisted undo entry, so "the history survived" is a real claim and not
/// a statement about an empty table.
fn persisted_undo_entry() -> Vec<(String, String, Option<String>)> {
    vec![(
        r#"{"type":"close_project"}"#.to_string(),
        r#"{"InverseCommand":{"type":"close_project"}}"#.to_string(),
        None,
    )]
}

/// The version this build stamps on a file it creates. Read back from a real
/// file rather than restated, so the fixture keeps meaning "one above this
/// build" across schema bumps.
fn this_builds_schema(dir: &Path) -> i64 {
    let probe = dir.join("probe.stalx");
    drop(Database::open(&probe).unwrap());
    let conn = rusqlite::Connection::open(&probe).unwrap();
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}

/// A `.stalx` a newer build wrote: this build's tables, one section of two
/// shots, one undo entry, and BOTH version claims one above this build.
fn build_newer_file(dir: &Path, name: &str) -> (PathBuf, i64) {
    let newer = this_builds_schema(dir) + 1;
    let path = dir.join(name);
    let mut project = Project::new("CaveA".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        color: "#123456".to_string(),
        shots: vec![
            ShotData {
                length: 5.0,
                ..Default::default()
            },
            ShotData {
                length: 7.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    db.save_undo_stack_with_timestamps(persisted_undo_entry())
        .unwrap();
    drop(db);
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(&format!(
        "UPDATE project SET schema_version = {newer}; PRAGMA user_version = {newer};"
    ))
    .unwrap();
    drop(conn);
    (path, newer)
}

/// Everything a write could change: the bytes, and the mtime a byte-identical
/// rewrite would still move.
fn snapshot(path: &Path) -> (Vec<u8>, SystemTime) {
    let bytes = std::fs::read(path).unwrap();
    let mtime = std::fs::metadata(path).unwrap().modified().unwrap();
    (bytes, mtime)
}

fn undo_rows(path: &Path) -> i64 {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    conn.query_row("SELECT COUNT(*) FROM undo_stack", [], |r| r.get(0))
        .unwrap()
}

fn user_version(path: &Path) -> i64 {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}

fn assert_refused_as_newer(label: &str, r: &CommandResult) {
    assert!(
        !r.success,
        "{label} must be refused on a file saved by a newer version"
    );
    let error = r.error.as_deref().unwrap_or_default();
    assert!(
        error.contains(NEWER_FILE_WORDS),
        "{label} must say the file is from a newer version, got: {error:?}"
    );
}

/// THE regression guard for the open: neither load path may write a byte of
/// a newer file - no migration, no undo purge, no load-time backfill - and its
/// undo history and stamp stay exactly as the newer build left them.
#[test]
fn opening_a_newer_file_writes_nothing_to_it() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, newer) = build_newer_file(temp.path(), "newer.stalx");
    let before = snapshot(&path);
    assert_eq!(undo_rows(&path), 1, "the fixture must stage an undo entry");

    // The production load path...
    let state = state_with_null_sink();
    let loaded = state.do_load_project_full(path.clone());
    let loaded = loaded.unwrap_or_else(|e| panic!("a newer file must still open: {e:?}"));
    assert_eq!(loaded.project.name, "CaveA");
    assert_eq!(loaded.project.sections[0].shots.len(), 2);
    assert!(state.close_project().success);

    // ...and the legacy one (the cache-replace recovery reopen).
    let legacy = AppState::new().unwrap();
    let opened = legacy.open_project(path.clone());
    assert!(opened.success, "open failed: {:?}", opened.error);
    assert!(legacy.close_project().success);

    let after = snapshot(&path);
    println!(
        "newer file (user_version {newer}): bytes identical = {}, mtime identical = {}, \
         undo rows = {}, user_version = {}",
        before.0 == after.0,
        before.1 == after.1,
        undo_rows(&path),
        user_version(&path)
    );
    assert_eq!(
        undo_rows(&path),
        1,
        "the open must leave the undo history in the file"
    );
    assert_eq!(user_version(&path), newer, "and the stamp untouched");
    assert!(
        before.0 == after.0,
        "the open must not change a byte of a newer file"
    );
    assert_eq!(before.1, after.1, "nor touch its mtime");
}

/// The history stays on disk but never reaches this build's undo stack: its
/// payloads were written by a format no migration arm here has vetted, which is
/// what the old open-time purge guarded against.
#[test]
fn a_newer_files_history_is_never_restored() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, _) = build_newer_file(temp.path(), "newer.stalx");

    let state = state_with_null_sink();
    state.do_load_project_full(path.clone()).unwrap();
    println!("after the full load: can_undo = {}", state.can_undo());
    assert!(
        !state.can_undo(),
        "no newer-format history may be replayable"
    );

    let legacy = AppState::new().unwrap();
    assert!(legacy.open_project(path).success);
    println!("after the legacy open: can_undo = {}", legacy.can_undo());
    assert!(
        !legacy.can_undo(),
        "no newer-format history may be replayable"
    );
}

/// Every edit is refused server-side - not only in the UI - with the
/// read-only gate's code and a reason that names the newer version.
#[test]
fn edits_to_a_newer_file_are_refused() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, _) = build_newer_file(temp.path(), "newer.stalx");
    let before = snapshot(&path);

    let state = state_with_null_sink();
    let loaded = state.do_load_project_full(path.clone()).unwrap();
    let section = &loaded.project.sections[0];

    let edit = state.execute(Command::UpdateShot {
        section_id: section.id,
        shot_id: section.shots[0].id,
        updates: ShotUpdates {
            length: Some(9.0),
            ..Default::default()
        },
    });
    println!(
        "execute(UpdateShot) on a newer file: success = {}, error = {:?}",
        edit.success, edit.error
    );
    assert_refused_as_newer("execute", &edit);
    // [#576 C4] The edit gate's newer-file code (what used to read
    // "EDIT_DISABLED: NEWER_FILE_READ_ONLY: ..."), and display text with no marker.
    assert_eq!(
        edit.code,
        Some(CommandErrorCode::NewerFileEditDisabled),
        "the refusal must carry the edit gate's newer-file code, got: {:?}",
        edit.error
    );
    assert!(
        edit.code.is_some_and(CommandErrorCode::is_edit_gate),
        "a newer-file edit refusal is an edit-gate refusal"
    );
    assert!(
        edit.error.as_deref().is_some_and(|e| {
            e.contains(" is disabled: this project was saved by a newer version")
                && !e.contains("EDIT_DISABLED")
                && !e.contains("NEWER_FILE_READ_ONLY")
        }),
        "the text is the plain sentence, got: {:?}",
        edit.error
    );
    assert_refused_as_newer(
        "update_project_settings",
        &state.update_project_settings("Renamed".to_string(), state.get_config().validation, None),
    );
    assert_refused_as_newer(
        "set_project_name_for_path",
        &state.set_project_name_for_path(path.clone(), "Renamed".to_string()),
    );
    // The in-memory project is untouched too.
    let live = state.get_project_state().unwrap();
    assert_eq!(live.name, "CaveA");
    assert!((live.sections[0].shots[0].length - 5.0).abs() < 1e-9);
    assert!(state.close_project().success);
    assert!(
        before.0 == snapshot(&path).0,
        "a refused edit wrote nothing"
    );
}

/// Every save is refused: in place (Ctrl+S, autosave, the pre-push flush) and
/// to another path (Save As would write this build's schema of a newer
/// project, the same downgrade by a different name).
#[test]
fn saving_a_newer_file_is_refused() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, newer) = build_newer_file(temp.path(), "newer.stalx");
    let before = snapshot(&path);
    let elsewhere = temp.path().join("copy.stalx");

    let state = state_with_null_sink();
    state.do_load_project_full(path.clone()).unwrap();

    let in_place = state.save_project(None);
    let explicit = state.save_project(Some(path.clone()));
    let save_as = state.save_project(Some(elsewhere.clone()));
    println!(
        "save_project on a newer file: in place = {}, explicit path = {}, save as = {}; \
         user_version after = {}",
        in_place.success,
        explicit.success,
        save_as.success,
        user_version(&path)
    );
    assert_refused_as_newer("save_project(None)", &in_place);
    assert_refused_as_newer("save_project(Some(own path))", &explicit);
    assert_refused_as_newer("save_project(Some(other path))", &save_as);
    for refused in [&in_place, &explicit, &save_as] {
        assert_eq!(
            refused.code,
            Some(CommandErrorCode::NewerFileReadOnly),
            "a refused save carries the code the frontend routes on, got: {:?}",
            refused.error
        );
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("Saving is disabled: ")),
            "and its text is the plain sentence, got: {:?}",
            refused.error
        );
    }
    assert!(
        !elsewhere.exists(),
        "a refused Save As must not create the copy"
    );
    assert!(state.close_project().success);
    let after = snapshot(&path);
    assert_eq!(user_version(&path), newer, "the file was not re-stamped");
    assert!(before.0 == after.0, "a refused save wrote nothing");
    assert_eq!(before.1, after.1);
}

/// The block is per project: closing the newer file and opening an ordinary
/// one gives an editable, saveable project again.
#[test]
fn the_read_only_block_ends_with_the_newer_project() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (newer_path, _) = build_newer_file(temp.path(), "newer.stalx");

    let ordinary = temp.path().join("ordinary.stalx");
    let mut project = Project::new("CaveB".to_string(), None);
    project.add_section(SectionData {
        color: "#654321".to_string(),
        shots: vec![ShotData {
            length: 3.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let mut db = Database::open(&ordinary).unwrap();
    db.save_project(&project).unwrap();
    drop(db);

    let state = state_with_null_sink();
    state.do_load_project_full(newer_path).unwrap();
    assert!(state.close_project().success);

    let loaded = state.do_load_project_full(ordinary).unwrap();
    let section = &loaded.project.sections[0];
    let edit = state.execute(Command::UpdateShot {
        section_id: section.id,
        shot_id: section.shots[0].id,
        updates: ShotUpdates {
            length: Some(4.0),
            ..Default::default()
        },
    });
    assert!(
        edit.success,
        "an ordinary project is editable: {:?}",
        edit.error
    );
    let saved = state.save_project(None);
    assert!(saved.success, "and saveable: {:?}", saved.error);
}

/// No SpeleoDB edit session for a project a newer build saved: taking the write
/// lock (or forcing an edit without one) would keep every collaborator out of a
/// project this session can neither change nor push. Refused before any network.
#[test]
fn a_newer_file_cannot_start_a_speleodb_edit_session() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, _) = build_newer_file(temp.path(), "newer.stalx");

    let state = state_with_null_sink();
    state.do_load_project_full(path).unwrap();

    let start = state.speleodb_start_editing().map(|_| ()).unwrap_err();
    assert!(
        start.message.contains(NEWER_FILE_WORDS),
        "start editing must be refused for the newer file, got: {}",
        start.message
    );
    let force = state.speleodb_force_edit().unwrap_err();
    assert!(
        force.message.contains(NEWER_FILE_WORDS),
        "force edit must be refused for the newer file, got: {}",
        force.message
    );
}

/// The recent-projects panel renames a CLOSED file by path. A newer file is
/// refused there too, before anything is written.
#[test]
fn renaming_a_closed_newer_file_is_refused() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, _) = build_newer_file(temp.path(), "newer.stalx");
    let before = snapshot(&path);

    let state = AppState::new().unwrap();
    assert_refused_as_newer(
        "set_project_name_for_path (closed file)",
        &state.set_project_name_for_path(path.clone(), "Renamed".to_string()),
    );
    let after = snapshot(&path);
    assert!(before.0 == after.0, "a refused rename wrote nothing");
    assert_eq!(undo_rows(&path), 1);
}

/// A merge reads its sources and writes a NEW file at this build's schema, so
/// a newer source would come out of it downgraded - the same loss a Save As of
/// that project is refused for. The merge is refused before anything is
/// written: no output file, no archived source, the newer file untouched.
#[test]
fn merging_a_newer_file_as_a_source_is_refused() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (newer_path, _) = build_newer_file(temp.path(), "newer.stalx");
    let current_path = temp.path().join("current.stalx");
    let mut current = Project::new("CaveB".to_string(), None);
    current.add_section(SectionData {
        name: Some("SectionB".to_string()),
        shots: vec![ShotData {
            length: 3.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    Database::open(&current_path)
        .unwrap()
        .save_project(&current)
        .unwrap();
    let before = snapshot(&newer_path);
    let output = temp.path().join("merged.stalx");

    let state = AppState::new().unwrap();
    let merged = state.merge_projects(
        vec![current_path.clone(), newer_path.clone()],
        output.clone(),
        "Merged".to_string(),
        true,
    );
    println!(
        "merge_projects with a newer source: success = {}, output written = {}, error = {:?}",
        merged.success,
        output.exists(),
        merged.error
    );
    assert_refused_as_newer("merge_projects (newer source)", &merged);
    assert!(
        !output.exists(),
        "a refused merge must not write the output"
    );
    assert!(
        current_path.exists() && newer_path.exists(),
        "a refused merge must not archive its sources"
    );
    let after = snapshot(&newer_path);
    assert!(
        before.0 == after.0,
        "a refused merge wrote nothing to the source"
    );
    assert_eq!(before.1, after.1);
    assert_eq!(undo_rows(&newer_path), 1);
}
