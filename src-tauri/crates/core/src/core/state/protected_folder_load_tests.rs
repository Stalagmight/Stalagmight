//! [#576 RV4-R-M3-OPEN] What a load does with a location's upgrade refusal.
//!
//! The load commands work out whether a project sits where nothing may be written
//! (the app's own folders, a system folder: `commands::write_path`) and hand the
//! refusal to the load. A file this build reads as it is opens exactly as it would
//! anywhere and is not written; an older-format file - one the open would upgrade
//! in place, keeping a backup copy beside it first - is refused before the first
//! write, with its own code and the refusal as the message, and left as it was.
//! The load-time repairs of a current file (#576 RV4-R-M3-D2) are made in memory
//! only there, as for a file a newer build saved.
//!
//! All data is synthetic: a project built in memory, saved by this build, then
//! re-stamped as an older file. No fixture is read or written.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::models::{CommandErrorCode, Project, SectionData, ShotData};
use crate::persistence::{Database, UpgradeRefusal};
use crate::test_fixtures::state_with_null_sink;

const REFUSAL: &str = "CaveL.stalx was saved by an older version of Stalagmight (test refusal). \
                       Copy the project to a folder of your own and open the copy.";
/// [#576 RV6b-R3] The message for an unstamped file the open would complete.
const UNSTAMPED_REFUSAL: &str = "CaveL.stalx has no file-format version stamp (test refusal). \
                                 Copy the project to a folder of your own and open the copy.";

/// What the load commands hand the load for a protected folder.
fn refusal() -> UpgradeRefusal {
    UpgradeRefusal {
        older_format: REFUSAL.to_owned(),
        unstamped: UNSTAMPED_REFUSAL.to_owned(),
    }
}

const OLDER_FORMAT: &str = "UPDATE project SET schema_version = 16; PRAGMA user_version = 16;";

/// A current project file with one section of two shots, then re-stamped by
/// `surgery` (empty: left current).
fn write_project(dir: &Path, surgery: &str) -> PathBuf {
    let path = dir.join("CaveL.stalx");
    let mut project = Project::new("CaveL".to_owned(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_owned()),
        color: "#123456".to_owned(),
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
    Database::open(&path)
        .expect("create")
        .save_project(&project)
        .expect("write");
    if !surgery.is_empty() {
        rusqlite::Connection::open(&path)
            .expect("reopen")
            .execute_batch(surgery)
            .expect("re-stamp");
    }
    path
}

/// The files in `dir`, by name (a backup copy or a journal would show up here).
fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("list")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn a_current_project_where_nothing_may_be_written_opens_and_is_not_written() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("dir");
    let path = write_project(dir.path(), "");
    let before = std::fs::read(&path).expect("read");

    let state = state_with_null_sink();
    let loaded = state.do_load_project_full_with(path.clone(), Some(&refusal()), None);
    let after = std::fs::read(&path).expect("read");
    println!(
        "current-format project, upgrade refused here: opened={} shots={:?}; file unchanged: {}; folder: {:?}",
        loaded.is_ok(),
        loaded
            .as_ref()
            .ok()
            .and_then(|l| l.project.sections.first())
            .map(|s| s.shots.len()),
        after == before,
        files_in(dir.path())
    );
    let loaded = loaded.unwrap_or_else(|e| panic!("it must open: {} {}", e.code, e.message));
    assert_eq!(loaded.project.sections[0].shots.len(), 2);
    assert!(state.get_project_state().is_some(), "the project is open");
    assert_eq!(after, before, "opening it wrote nothing");
    assert_eq!(files_in(dir.path()), ["CaveL.stalx"]);
}

#[test]
fn an_older_project_where_nothing_may_be_written_is_refused_untouched() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("dir");
    let path = write_project(dir.path(), OLDER_FORMAT);
    let before = std::fs::read(&path).expect("read");

    let state = state_with_null_sink();
    let Err(error) = state.do_load_project_full_with(path.clone(), Some(&refusal()), None) else {
        panic!("an older file must not be upgraded there");
    };
    let after = std::fs::read(&path).expect("read");
    println!(
        "older-format project, upgrade refused here: code={} message={:?}; file unchanged: {}; folder: {:?}",
        error.code,
        error.message,
        after == before,
        files_in(dir.path())
    );
    assert_eq!(error.code, CommandErrorCode::UpgradeRefused);
    assert_eq!(
        error.message, REFUSAL,
        "shown as-is, so the message carries no code prefix"
    );
    assert!(
        state.get_project_state().is_none(),
        "a refused open leaves no project"
    );
    assert_eq!(after, before, "the file is left as it was");
    assert_eq!(
        files_in(dir.path()),
        ["CaveL.stalx"],
        "and no backup copy beside it"
    );
}

#[test]
fn without_a_refusal_the_same_older_project_is_upgraded_as_before() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("dir");
    let path = write_project(dir.path(), OLDER_FORMAT);

    let state = state_with_null_sink();
    let loaded = state.do_load_project_full_with(path, None, None);
    println!(
        "older-format project in a user folder: opened={}; folder: {:?}",
        loaded.is_ok(),
        files_in(dir.path())
    );
    assert!(loaded.is_ok(), "it opens, upgraded");
    assert!(
        files_in(dir.path()).contains(&"CaveL.stalx.pre-v16.backup".to_owned()),
        "with its pre-upgrade copy"
    );
}

/// [#576 RV6b-R3] No file-format stamp, but a project row claiming this build's
/// version: the open would only complete the tables in place (no backup copy, no
/// stamp), so the user is not told the file is an older one.
const UNSTAMPED: &str = "PRAGMA user_version = 0;";

#[test]
fn an_unstamped_project_where_nothing_may_be_written_is_not_called_older() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("dir");
    let path = write_project(dir.path(), UNSTAMPED);
    let before = std::fs::read(&path).expect("read");

    let state = state_with_null_sink();
    let Err(error) = state.do_load_project_full_with(path.clone(), Some(&refusal()), None) else {
        panic!("an unstamped file is not completed in place there either");
    };
    let after = std::fs::read(&path).expect("read");
    println!(
        "unstamped project claiming the current version, upgrade refused here: code={} \
         message={:?}; file unchanged: {}; folder: {:?}",
        error.code,
        error.message,
        after == before,
        files_in(dir.path())
    );
    assert_eq!(error.code, CommandErrorCode::UpgradeRefused);
    assert!(
        !error.message.contains("older version") && !error.message.contains("backup"),
        "the message must not call the file older or promise a backup: {:?}",
        error.message
    );
    assert_eq!(error.message, UNSTAMPED_REFUSAL, "shown as-is");
    assert_eq!(after, before, "the file is left as it was");
    assert_eq!(files_in(dir.path()), ["CaveL.stalx"]);
}

/// [#576 RV4-R-M3-D2, RV6b-R2] The repairs a load makes to a CURRENT file, one
/// file each: `(what, surgery)`. `{sid}` is the section's id, `{shot0}` the first
/// shot's in-memory id, `{ignores}` the table the "Ignore" choices live in.
const REPAIRS: &[(&str, &str)] = &[
    (
        "a legacy ignore keyed by position (the UUID backfill)",
        "INSERT INTO {ignores} (section_id, shot_id, warning_code, resolved_at) \
         VALUES ({sid}, {shot0}, 'SHORT_SHOT', '2026-01-01T00:00:00+00:00');",
    ),
    (
        "a shot kind this build cannot read (the Reference repair)",
        "UPDATE shots SET kind = 9 WHERE sequence_order = (SELECT MAX(sequence_order) FROM shots);",
    ),
    (
        "a pending Jump/Reference classification (the kind backfill)",
        "UPDATE shots SET kind = 1 WHERE sequence_order = (SELECT MAX(sequence_order) FROM shots); \
         INSERT INTO project_settings (key, value) VALUES ('pending_kind_classification', '1');",
    ),
    (
        "a SpeleoDB link with no undo mode recorded (the undo-mode materialization)",
        "INSERT INTO speleo_link (id, endpoint, project_id, project_name, has_unpushed_edits) \
         VALUES (1, 'https://example.invalid', '11111111-1111-1111-1111-111111111111', 'CaveL', 0);",
    ),
];

/// A current file whose first shot is short (it raises `SHORT_SHOT`) and whose
/// shots carry persisted UUIDs, with `surgery` applied (see [`REPAIRS`]).
fn file_needing_a_repair(dir: &Path, surgery: &str) -> PathBuf {
    let path = dir.join("CaveL.stalx");
    let mut project = Project::new("CaveL".to_owned(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_owned()),
        color: "#123456".to_owned(),
        shots: vec![
            ShotData {
                length: 0.1,
                ..Default::default()
            },
            ShotData {
                length: 7.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    project.ensure_shot_uuids();
    let mut db = Database::open(&path).expect("create");
    db.save_project(&project).expect("write");
    let saved = db.load_project().expect("read back");
    drop(db);
    let conn = rusqlite::Connection::open(&path).expect("reopen");
    let ignores: String = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'resolved%'",
            [],
            |r| r.get(0),
        )
        .expect("the ignore table");
    conn.execute_batch(
        &surgery
            .replace("{ignores}", &ignores)
            .replace("{sid}", &saved.sections[0].id.to_string())
            .replace("{shot0}", &saved.sections[0].shots[0].id.to_string()),
    )
    .expect("surgery");
    path
}

#[test]
fn a_load_where_nothing_may_be_written_keeps_its_repairs_in_memory() {
    let mut wrong = Vec::new();
    for (what, surgery) in REPAIRS {
        let _cfg = crate::test_helpers::isolated_config();
        // The control: the same file where a user keeps projects is repaired on
        // disk, so each case really is a write the load makes.
        let control_dir = TempDir::new().expect("dir");
        let control = file_needing_a_repair(control_dir.path(), surgery);
        let control_before = std::fs::read(&control).expect("read");
        let opened = state_with_null_sink().do_load_project_full_with(control.clone(), None, None);
        assert!(opened.is_ok(), "{what}: the control opens");
        let control_written = std::fs::read(&control).expect("read") != control_before;

        let dir = TempDir::new().expect("dir");
        let path = file_needing_a_repair(dir.path(), surgery);
        let before = std::fs::read(&path).expect("read");
        let state = state_with_null_sink();
        let loaded = state
            .do_load_project_full_with(path.clone(), Some(&refusal()), None)
            .unwrap_or_else(|e| panic!("{what}: it must open: {} {}", e.code, e.message));
        let unchanged = std::fs::read(&path).expect("read") == before;
        let shots = &loaded.project.sections[0].shots;
        let in_memory = match *what {
            w if w.starts_with("a legacy ignore") => loaded
                .warnings
                .iter()
                .find(|w| w.code == "SHORT_SHOT")
                .map(|w| format!("SHORT_SHOT ignored: {}", w.is_resolved)),
            w if w.starts_with("a shot kind") => Some(format!(
                "kind {:?}, flagged for the user: {}",
                shots[1].kind,
                state.take_unrecognised_shot_kinds(loaded.loading_id).len()
            )),
            w if w.starts_with("a pending") => Some(format!("kind {:?}", shots[1].kind)),
            _ => Some(format!("undo policy {:?}", state.undo_policy())),
        };
        let expected = match *what {
            w if w.starts_with("a legacy ignore") => "SHORT_SHOT ignored: true".to_owned(),
            w if w.starts_with("a shot kind") => {
                "kind Reference, flagged for the user: 1".to_owned()
            }
            w if w.starts_with("a pending") => "kind Reference".to_owned(),
            _ => format!(
                "undo policy {:?}",
                state.undo_policy_for(Some(&crate::models::SpeleoLink {
                    endpoint: String::new(),
                    project_id: String::new(),
                    project_name: String::new(),
                    base_commit_sha: None,
                    has_unpushed_edits: false,
                    last_edit_at: None,
                    last_verify_at: None,
                    undo_mode: None,
                }))
            ),
        };
        println!(
            "{what}: written where a user keeps projects: {control_written}; where nothing may \
             be written: file unchanged {unchanged}, folder {:?}, in memory: {in_memory:?}",
            files_in(dir.path())
        );
        if !control_written
            || !unchanged
            || files_in(dir.path()) != ["CaveL.stalx"]
            || in_memory.as_deref() != Some(expected.as_str())
        {
            wrong.push(*what);
        }
    }
    assert!(wrong.is_empty(), "wrong for: {wrong:?}");
}
