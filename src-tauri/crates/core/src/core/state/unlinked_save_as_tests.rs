//! [#576, W2-SAVE open question 1, decision W2SAVE-MARKERS] A Save As away from
//! a file whose SpeleoDB link cannot be read writes an UNLINKED project - and so,
//! like Save a Copy, sheds the undo-chain markers too.
//!
//! Those markers (`speleo_undo_chain`, `speleo_undo_watermark`) describe this
//! machine's relationship to a server project (#303). A project without a link
//! that keeps them is a state no detached copy ever has: linked again later, its
//! first push would cut the user's own history as "inherited".
//!
//! All data is synthetic: a project this build saves with a link row and both
//! markers, then the link's table hand-narrowed so the row cannot be read.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::models::{Project, SectionData, ShotData, SpeleoLink};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

/// A linked project with both undo-chain markers, whose link row then cannot be
/// read (a column the SELECT names is renamed).
fn file_with_markers_and_an_unreadable_link(dir: &Path) -> PathBuf {
    let path = dir.join("CaveA.stalx");
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
    db.save_speleo_undo_chain(Some("def456")).unwrap();
    db.save_speleo_undo_watermark(Some("2026-10-01T00:00:00+00:00"))
        .unwrap();
    drop(db);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("ALTER TABLE speleo_link RENAME COLUMN project_name TO project_title;")
        .unwrap();
    path
}

/// THE regression: the Save As escape from an unreadable link goes ahead, and the
/// file it writes carries neither the link nor the markers.
#[test]
fn a_save_as_away_from_an_unreadable_link_sheds_the_undo_chain_markers() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let path = file_with_markers_and_an_unreadable_link(dir.path());
    let state = state_with_null_sink();
    let loaded = state.do_load_project_full(path).unwrap();
    assert!(
        loaded.speleo_link_unreadable,
        "premise: the link cannot be read"
    );

    let rescue = dir.path().join("CaveA-rescue.stalx");
    let saved = state.save_project(Some(rescue.clone()));
    assert!(
        saved.success,
        "the Save As escape goes ahead: {:?}",
        saved.error
    );

    let db = Database::open_existing(&rescue).unwrap();
    let link = db.load_speleo_link().unwrap();
    let chain = db.load_speleo_undo_chain().unwrap();
    let watermark = db.load_speleo_undo_watermark().unwrap();
    println!("saved-as file: link={link:?} undo_chain={chain:?} undo_watermark={watermark:?}");
    assert!(link.is_none(), "an unlinked project");
    assert!(
        chain.is_none() && watermark.is_none(),
        "an unlinked project sheds the undo-chain markers, as Save a Copy does"
    );
    assert_eq!(
        db.load_project().unwrap().sections[0].shots.len(),
        1,
        "the survey itself is all there"
    );
}
