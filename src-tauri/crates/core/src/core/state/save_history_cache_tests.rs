//! [#576 B20-D2] A save compresses only the history entries that are new since
//! the last save: every earlier entry's gzipped rows are reused from the stack.
//!
//! `ENTRIES_COMPRESSED` counts the entries `UndoStack::stored_rows_to_persist`
//! compresses on this thread, so the delta across one `save_project` is exactly
//! the number of entries that save had to gzip.

use crate::core::state::AppState;
use crate::core::undo::ENTRIES_COMPRESSED;
use crate::core::Command;

use super::perf_probe_tests::synthetic_state;

fn compressed_so_far() -> usize {
    ENTRIES_COMPRESSED.with(std::cell::Cell::get)
}

/// Save to `path` and return how many entries that save compressed.
fn entries_compressed_by_save(state: &AppState, path: &std::path::Path) -> usize {
    let before = compressed_so_far();
    let result = state.save_project(Some(path.to_path_buf()));
    assert!(result.success, "save failed: {:?}", result.error);
    compressed_so_far() - before
}

fn reverse(state: &AppState, section_id: u32) {
    let result = state.execute(Command::ReverseSection {
        section_id,
        swap_left_right: false,
    });
    assert!(result.success, "{:?}", result.error);
}

/// The history rows as stored in the saved file, oldest first.
fn saved_rows(path: &std::path::Path, table: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT command_json, undo_data_json FROM {table} ORDER BY sequence"
        ))
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn a_second_save_compresses_only_the_entries_new_since_the_first() {
    let state = synthetic_state(3, 4);
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("cache.stalx");
    for section_id in 0..3 {
        reverse(&state, section_id);
    }
    let undo_entries = state.undo_stack.lock().len();
    assert!(undo_entries >= 3, "expected at least 3 undo entries");

    let first = entries_compressed_by_save(&state, &path);
    println!(
        "first save: {first} entries compressed through the cache ({undo_entries} on the stack)"
    );
    assert_eq!(
        first, undo_entries,
        "the first save must compress every entry once, through the stack's cache"
    );
    let rows_after_first = saved_rows(&path, "undo_stack");

    let idle = entries_compressed_by_save(&state, &path);
    println!("idle save (nothing changed): {idle} entries compressed");
    assert_eq!(idle, 0, "an idle save must reuse every compressed entry");
    assert_eq!(
        saved_rows(&path, "undo_stack"),
        rows_after_first,
        "a reused row must be byte for byte what the first save wrote"
    );

    reverse(&state, 0);
    let after_edit = entries_compressed_by_save(&state, &path);
    println!("save after one more edit: {after_edit} entries compressed");
    assert_eq!(after_edit, 1, "only the new entry may be compressed");

    // Undo moves the newest entry to the redo stack: it is compressed afresh
    // there, and nothing else is.
    assert!(state.undo().success);
    let after_undo = entries_compressed_by_save(&state, &path);
    println!("save after one undo: {after_undo} entries compressed");
    assert_eq!(
        after_undo, 1,
        "only the entry that moved to redo may be compressed"
    );
    assert_eq!(saved_rows(&path, "redo_stack").len(), 1);
}

#[test]
fn a_save_through_the_cache_reopens_with_the_same_history() {
    let state = synthetic_state(2, 4);
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("reopen.stalx");
    reverse(&state, 0);
    reverse(&state, 1);
    assert!(state.undo().success);
    assert!(state.save_project(Some(path.clone())).success);
    // Save again so the second save's rows are all reused ones.
    assert!(state.save_project(Some(path.clone())).success);
    let (undo_before, redo_before) = {
        let stack = state.undo_stack.lock();
        (stack.len(), stack.redo_len())
    };

    let reopened = AppState::new().unwrap();
    assert!(reopened.open_project(path).success);
    let stack = reopened.undo_stack.lock();
    assert_eq!((stack.len(), stack.redo_len()), (undo_before, redo_before));
}
