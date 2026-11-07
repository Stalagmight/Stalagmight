//! [#576 RV4-R-S5] The open-file flags re-asked after a SpeleoDB in-place tree
//! swap carry the unread-settings list too.
//!
//! The list of stored project settings an open could not read (B15) is, like the
//! newer-format stamp and the unreadable link, a property of the FILE: set at the
//! slot commit, it refuses every save. The swap's reload re-sets it in the
//! backend, but the frontend only learned it from `loadProject`, so after a swap
//! saves were refused with no notice (or a stale notice stood over a healthy
//! file). All data is synthetic, in a temp dir.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::AppState;
use crate::models::Project;
use crate::persistence::Database;

fn saved(dir: &Path, file: &str, break_settings: bool) -> PathBuf {
    let path = dir.join(file);
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("CaveF".to_owned(), None))
        .unwrap();
    drop(db);
    if break_settings {
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch("ALTER TABLE project_settings RENAME COLUMN value TO broken_value;")
            .unwrap();
    }
    path
}

#[test]
fn the_open_file_flags_carry_the_unread_settings_of_the_file_open_now() {
    let _cfg = crate::test_helpers::isolated_config();
    let dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();

    assert!(
        state
            .open_project(saved(dir.path(), "broken.stalx", true))
            .success
    );
    let unread = state.open_file_flags().unreadable_project_settings;
    println!("file with unreadable settings -> unread {unread:?}");
    assert!(!unread.is_empty(), "the unread list rides the flags");
    assert_eq!(unread, state.unreadable_project_settings());

    assert!(
        state
            .open_project(saved(dir.path(), "healthy.stalx", false))
            .success
    );
    let unread = state.open_file_flags().unreadable_project_settings;
    println!("healthy file -> unread {unread:?}");
    assert!(
        unread.is_empty(),
        "and clears for a file that read them all"
    );
}
