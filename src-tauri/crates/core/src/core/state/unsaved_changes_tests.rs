//! [#576 W81-1] The backend says whether the open project holds changes its file
//! does not (`AppState::has_unsaved_changes`, carried as
//! `OpenFileFlags::has_unsaved_changes`).
//!
//! Only the frontend kept an unsaved mark, and a webview reload drops the page
//! while the backend keeps the project open with its edits. The frontend's re-read
//! of that project (the ALREADY_OPEN branch of `loadProject`) marked it clean, so
//! the close and quit prompts let the edits go. The re-read now takes the mark
//! from here. Driven through the real load (`do_load_project_full`), edit
//! (`execute`) and save (`execute(SaveProject)`) paths. All data is synthetic, in
//! a temp dir.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{Project, SectionData, SectionUpdates, ShotData};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

/// A file this build saved: CaveA with one section of two shots.
fn saved_cave(dir: &Path) -> PathBuf {
    let mut project = Project::new("CaveA".to_owned(), None);
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
    project.ensure_shot_uuids();
    let path = dir.join("CaveA.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    path
}

/// Rename the open project's only section: an ordinary survey edit.
fn rename_section(state: &AppState, name: &str) {
    let section_id = state
        .get_project_state()
        .unwrap()
        .sections
        .first()
        .unwrap()
        .id;
    let renamed = state.execute(Command::UpdateSectionMetadata {
        section_id,
        updates: SectionUpdates {
            name: Some(Some(name.to_owned())),
            ..Default::default()
        },
    });
    assert!(renamed.success, "rename: {:?}", renamed.error);
}

fn save(state: &AppState) {
    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "save: {:?}", saved.error);
}

/// Opened and not edited: clean. Edited: unsaved. Saved: clean again. Edited and
/// undone after that save: unsaved, since the file holds the edit the undo took
/// back. Each answer as the frontend reads it, through the open-file flags.
#[test]
fn the_open_file_flags_say_whether_the_session_holds_changes_its_file_does_not() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let path = saved_cave(dir.path());
    let state = state_with_null_sink();
    state.do_load_project_full(path).unwrap();
    let unsaved = || state.open_file_flags().has_unsaved_changes;

    let opened = unsaved();
    rename_section(&state, "SectionB");
    let edited = unsaved();
    save(&state);
    let saved = unsaved();
    rename_section(&state, "SectionC");
    let edited_again = unsaved();
    let undone = state.undo();
    assert!(undone.success, "undo: {:?}", undone.error);
    let undone_to_saved = unsaved();
    save(&state);
    let saved_again = unsaved();
    let undone = state.undo();
    assert!(undone.success, "undo: {:?}", undone.error);
    let undone_past_save = unsaved();

    println!(
        "opened {opened}, edited {edited}, saved {saved}, edited again {edited_again}, \
         undone to the saved state {undone_to_saved}, saved again {saved_again}, \
         undone past that save {undone_past_save}"
    );
    assert!(
        !opened,
        "a project opened from its file and not edited is clean"
    );
    assert!(edited, "an edit is a change the file does not hold");
    assert!(!saved, "a save writes it");
    assert!(edited_again, "and the next edit is unsaved again");
    assert!(!saved_again, "a save after the undo writes it");
    assert!(
        undone_past_save,
        "an undo past a save takes back what the file holds"
    );
}

/// No project open: nothing to save.
#[test]
fn no_project_open_holds_no_unsaved_changes() {
    let _cfg = crate::test_helpers::isolated_config();
    let state = state_with_null_sink();
    let unsaved = state.open_file_flags().has_unsaved_changes;
    println!("no project open -> unsaved {unsaved}");
    assert!(!unsaved);
}
