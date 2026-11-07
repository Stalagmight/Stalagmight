//! [#576 RV4-R-NITS S-9] Publish's relocation reports a refused save as the save's
//! own refusal, not as a filesystem error.
//!
//! The push path already passes the pre-push save's refusal through as written
//! (`speleodb_flush_before_push`, NF-W4S2-2). Publish wrapped the same refusal in
//! `SpeleoDbError::Io`, so the user read "filesystem error: failed to write
//! project into the SpeleoDB cache: <marker>" for a project whose stored settings
//! could not be read - not a disk problem at all.
//!
//! All data is synthetic, built in a temp dir.

use tempfile::TempDir;

use super::AppState;
use crate::models::{Project, SectionData, ShotData, SpeleoLink};
use crate::persistence::Database;

/// A local project whose stored project settings cannot be read, opened.
fn open_with_unreadable_settings(dir: &TempDir) -> AppState {
    let path = dir.path().join("CaveP.stalx");
    let mut project = Project::new("CaveP".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionP".to_string()),
        shots: vec![ShotData {
            length: 4.0,
            azimuth_in: 30.0,
            azimuth_out: 210.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("ALTER TABLE project_settings RENAME COLUMN value TO broken_value;")
        .unwrap();
    let state = AppState::new().unwrap();
    assert!(state.open_project(path).success, "open the fixture");
    state
}

#[test]
fn a_refused_save_during_publish_reads_as_the_save_refusal() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let state = open_with_unreadable_settings(&dir);
    let dest = dir.path().join("cache").join("published.stalx");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();

    let refused = state
        .speleodb_publish_relocate(&dest, |undo_mode| SpeleoLink {
            endpoint: "http://127.0.0.1:9/".to_owned(),
            project_id: "33333333-3333-3333-3333-333333333333".to_owned(),
            project_name: "CaveP".to_owned(),
            base_commit_sha: None,
            has_unpushed_edits: true,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: Some(undo_mode),
        })
        .err()
        .map(|e| e.to_string());

    println!("publish relocation with unreadable settings -> {refused:?}");
    let message = refused.unwrap_or_default();
    assert!(!message.is_empty(), "the save is refused");
    assert!(
        !message.starts_with("filesystem error"),
        "a save refusal is not a filesystem error: {message:?}"
    );
}
