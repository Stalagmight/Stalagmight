//! [#576 B35(b) review RV1-RUST-3] Opening a file whose SpeleoDB link cannot be
//! read must say so.
//!
//! The open treats an unreadable `speleo_link` row as "no link", so the session
//! runs as a plain local project - but every save that would rewrite that file
//! is refused (B35(b), `save_project`), because the rebuilt file would lose the
//! row. Without a word at open, the first autosave failed with a message about a
//! link the user was never shown. The load result now carries the verdict so the
//! frontend can tell the user when the project opens.
//!
//! All data is synthetic: a project built in memory, saved by this build with a
//! link row, then the row's table hand-narrowed so it cannot be read.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::models::{Project, SectionData, ShotData, SpeleoLink};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

/// A saved project with one shot and a SpeleoDB link row.
fn linked_file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    let mut project = Project::new("CaveA".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        shots: vec![ShotData {
            length: 4.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    db.save_speleo_link(Some(&SpeleoLink {
        endpoint: "http://speleodb.invalid/".to_string(),
        project_id: "11111111-1111-1111-1111-111111111111".to_string(),
        project_name: "CaveA".to_string(),
        base_commit_sha: Some("abc123".to_string()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }))
    .unwrap();
    drop(db);
    path
}

/// The same hand-narrowing as the B35(b) save tests: a column the SELECT names
/// is renamed, so the row is there but cannot be read.
fn make_link_unreadable(path: &Path) {
    rusqlite::Connection::open(path)
        .unwrap()
        .execute_batch("ALTER TABLE speleo_link RENAME COLUMN project_name TO project_title;")
        .unwrap();
    assert!(Database::open_for_metadata(path)
        .unwrap()
        .load_speleo_link()
        .is_err());
}

/// Only the envelope key under test; every other key is skipped. Optional, so a
/// build whose envelope lacks the key reads as `None` instead of failing to decode.
#[derive(serde::Deserialize)]
struct EnvelopeFlag {
    #[serde(default)]
    speleo_link_unreadable: Option<bool>,
}

/// What the frontend reads off a load: the binary envelope's flag.
fn envelope_flag(state: &AppState, path: PathBuf) -> (bool, Option<bool>) {
    let loaded = state.do_load_project_full(path).unwrap();
    let is_remote = loaded.is_remote;
    let bytes = crate::core::load_binary::encode(loaded, 0).unwrap();
    let flag: EnvelopeFlag = rmp_serde::from_slice(&bytes).unwrap();
    (is_remote, flag.speleo_link_unreadable)
}

#[test]
fn opening_a_file_whose_link_cannot_be_read_reports_it() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = linked_file(temp.path(), "linked.stalx");
    make_link_unreadable(&path);

    let state = state_with_null_sink();
    let (is_remote, unreadable) = envelope_flag(&state, path);
    let save = state.save_project(None);
    println!("open with an unreadable link: is_remote = {is_remote}, speleo_link_unreadable = {unreadable:?}");
    println!(
        "in-place save afterwards: success = {}, error = {:?}",
        save.success, save.error
    );
    assert!(!is_remote, "the project still opens as a local one");
    // The save it explains stays refused (B35(b) is fail-closed)...
    assert!(!save.success);
    // ...and the open now tells the frontend why, before the first save fails.
    assert_eq!(
        unreadable,
        Some(true),
        "the load the frontend reads must report the link it could not read"
    );
}

#[test]
fn a_readable_link_or_no_link_reports_nothing() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let linked = linked_file(temp.path(), "linked.stalx");
    let local = temp.path().join("local.stalx");
    let mut db = Database::open(&local).unwrap();
    db.save_project(&Project::new("CaveB".to_string(), None))
        .unwrap();
    drop(db);

    let state = state_with_null_sink();
    assert_eq!(envelope_flag(&state, linked), (true, Some(false)));
    assert!(state.close_project().success);
    assert_eq!(envelope_flag(&state, local), (false, Some(false)));
}
