//! [#576 RV2-R-6 / NF-W2SAVE-1] What a Done or Push writes into the live file
//! before the save that would refuse it, and what it says when that save refuses.
//!
//! All data is synthetic: a project this build saves, then its settings table
//! broken with SQL so the open cannot read it (the B15 state).

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::models::{Project, SectionData, ShotData};
use crate::persistence::Database;

/// A project whose stored settings the open cannot read (the `value` column is
/// renamed), holding a non-default entrance elevation and shot-length threshold.
fn file_with_unreadable_settings(dir: &Path) -> PathBuf {
    let path = dir.join("CaveB.stalx");
    let mut project = Project::new("CaveB".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionB".to_string()),
        color: "#654321".to_string(),
        shots: vec![ShotData {
            length: 4.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    project.validation.warn_shot_length_max = 77.0;
    project.surface_elevation = Some(123.0);
    let mut db = Database::open(&path).expect("create the fixture");
    db.save_project(&project).expect("save the fixture");
    drop(db);
    set_value_column(&path, "value", "broken_value");
    path
}

fn set_value_column(path: &Path, from: &str, to: &str) {
    rusqlite::Connection::open(path)
        .expect("open the fixture directly")
        .execute_batch(&format!(
            "ALTER TABLE project_settings RENAME COLUMN {from} TO {to};"
        ))
        .expect("rename the settings column");
}

/// What the file now stores for each setting key and the project name.
fn stored(path: &Path) -> (Vec<(String, String)>, String) {
    let conn = rusqlite::Connection::open(path).expect("open the file directly");
    let mut stmt = conn
        .prepare("SELECT key, value FROM project_settings ORDER BY key")
        .expect("read the settings");
    let settings = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query the settings")
        .collect::<Result<Vec<(String, String)>, _>>()
        .expect("settings rows");
    let name = conn
        .query_row("SELECT name FROM project LIMIT 1", [], |row| row.get(0))
        .expect("read the project name");
    (settings, name)
}

#[test]
fn a_refused_pre_push_flush_says_why_the_save_refused() {
    // [#576 NF-W2SAVE-1] The flush Done/Push run before taking the upload snapshot
    // replaced the save's own refusal with "failed to flush project before push",
    // hiding which of the save's reasons applied (here: stored settings the open
    // could not read) and so how to get out of it.
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let path = file_with_unreadable_settings(dir.path());
    let state = AppState::new().expect("state");
    assert!(state.open_project(path).success);
    let save = state.save_project(None);
    let reason = save.error.expect("fixture: the save refuses");

    let err = state
        .speleodb_flush_before_push()
        .expect_err("the flush refuses exactly when the save does");

    println!("the save refuses with: {reason}");
    println!("the pre-push flush reports: {}", err.message);
    assert_eq!(
        err.message, reason,
        "the push reports the save's own reason, not a generic flush failure"
    );
}

#[test]
fn the_server_name_fold_never_writes_over_unread_settings() {
    // [#576 RV2-R-6] Done/Push fold the server's project name into the open file
    // (`speleodb_fold_local_name`) by saving the WHOLE model in place - before the
    // flush that B15 makes refuse while stored settings are unread. The model holds
    // DEFAULTS for those settings, so the fold wrote the defaults over the stored
    // values (and deleted the elevation) the moment the file became writable again,
    // e.g. after a transient read failure on open. The fold now refuses whenever
    // that save would.
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().expect("tempdir");
    let path = file_with_unreadable_settings(dir.path());
    let state = AppState::new().expect("state");
    assert!(state.open_project(path.clone()).success);
    assert!(
        !state.unreadable_project_settings().is_empty(),
        "fixture: the open could not read the stored settings"
    );
    // The read failure was transient: the file is readable and writable again.
    set_value_column(&path, "broken_value", "value");
    let (before, name_before) = stored(&path);

    state.speleodb_fold_local_name("CaveB renamed on SpeleoDB");

    let (after, name_after) = stored(&path);
    println!("stored settings before the fold: {before:?} (name {name_before:?})");
    println!("stored settings after the fold:  {after:?} (name {name_after:?})");
    assert_eq!(
        after, before,
        "the fold must not write the in-memory defaults over the stored settings"
    );
    assert_eq!(
        name_after, name_before,
        "nothing is folded into a file the save would refuse to write"
    );
}
