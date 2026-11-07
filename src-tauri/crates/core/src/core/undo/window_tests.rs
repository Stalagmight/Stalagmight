//! [#576 B20 / NF-AN6-2] What a count-window stack (a local project, Disabled
//! mode) keeps in memory and writes on save, measured in bytes as well as
//! entries, and the per-entry compressed form a save reuses.
//!
//! Sizes are set through `command_bytes`, which `UndoStack::estimated_size`
//! charges verbatim, so a "20 MiB entry" here costs a few bytes of real memory.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use super::{
    Command, PersistStack, UndoData, UndoEntry, UndoStack, COUNT_WINDOW_MEMORY_BYTES,
    UNDO_BYTE_BUDGET_BYTES,
};
use crate::persistence::{compress_history_json, Database};

const MIB: usize = 1024 * 1024;

fn stamp(i: usize) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + Duration::seconds(1_700_000_000 + i as i64)
}

/// An entry whose estimated size is `bytes` plus a two-byte payload.
fn heavy(i: usize, bytes: usize) -> UndoEntry {
    UndoEntry {
        command: Command::DeleteSection {
            section_id: i as u32,
        },
        undo_data: UndoData::ProjectSnapshot("{}".to_string()),
        timestamp: Some(stamp(i)),
        command_bytes: Some(bytes),
        replayed_at: None,
    }
}

/// The section ids (= entry numbers) of the rows a save would write.
fn saved_ids(stack: &UndoStack, which: PersistStack, limit: usize) -> Vec<u32> {
    stack
        .rows_to_persist(which, Some(limit))
        .iter()
        .map(
            |(command, _, _)| match serde_json::from_str::<Command>(command) {
                Ok(Command::DeleteSection { section_id }) => section_id,
                other => panic!("unexpected row {other:?}"),
            },
        )
        .collect()
}

fn stack_of(entries: impl IntoIterator<Item = UndoEntry>) -> UndoStack {
    let mut stack = UndoStack::new_with_limit(100);
    for entry in entries {
        stack.restore_undo(entry);
    }
    stack
}

#[test]
fn a_saved_count_window_keeps_only_the_newest_entries_that_fit_the_byte_budget() {
    // 20 entries of 3 MiB: the 20-entry window is 60 MiB; 10 of them fit in 32 MiB.
    let stack = stack_of((0..20).map(|i| heavy(i, 3 * MIB)));
    assert_eq!(stack.len(), 20, "all 20 stay undoable in memory");
    let saved = saved_ids(&stack, PersistStack::Undo, 20);
    assert_eq!(
        saved,
        (10..20).collect::<Vec<u32>>(),
        "newest 10, oldest first"
    );
    let bytes: usize = stack
        .undo_entries()
        .take(saved.len())
        .map(UndoStack::estimated_size)
        .sum();
    assert!(bytes <= UNDO_BYTE_BUDGET_BYTES, "{bytes} over the budget");
}

#[test]
fn the_redo_half_of_a_save_is_capped_the_same_way() {
    let mut stack = UndoStack::new_with_limit(100);
    for i in 0..20 {
        stack.push_redo(heavy(i, 3 * MIB));
    }
    assert_eq!(
        saved_ids(&stack, PersistStack::Redo, 20),
        (10..20).collect::<Vec<u32>>()
    );
}

#[test]
fn the_newest_entry_is_saved_even_when_it_alone_exceeds_the_budget() {
    let mut entries: Vec<UndoEntry> = (0..5).map(|i| heavy(i, 1024)).collect();
    entries.push(heavy(5, 40 * MIB));
    let stack = stack_of(entries);
    assert_eq!(saved_ids(&stack, PersistStack::Undo, 20), vec![5]);
}

#[test]
fn ordinary_entries_keep_the_whole_count_window() {
    let stack = stack_of((0..30).map(|i| heavy(i, 1024)));
    assert_eq!(
        saved_ids(&stack, PersistStack::Undo, 20),
        (10..30).collect::<Vec<u32>>()
    );
}

#[test]
fn the_in_memory_count_window_is_bounded_in_bytes_too() {
    // 20 entries of 20 MiB: 12 fit under the 256 MiB ceiling.
    let stack = stack_of((0..20).map(|i| heavy(i, 20 * MIB)));
    assert_eq!(stack.len(), 12);
    assert!(stack.estimated_undo_bytes() <= COUNT_WINDOW_MEMORY_BYTES);
    let kept: Vec<u32> = stack
        .undo_entries()
        .map(|e| match e.command {
            Command::DeleteSection { section_id } => section_id,
            _ => u32::MAX,
        })
        .collect();
    assert_eq!(kept, (8..20).rev().collect::<Vec<u32>>(), "newest kept");

    let mut redo = UndoStack::new_with_limit(100);
    for i in 0..20 {
        redo.push_redo(heavy(i, 20 * MIB));
    }
    assert_eq!(redo.redo_len(), 12);

    // One entry past the ceiling on its own is still kept.
    let alone = stack_of([heavy(0, 300 * MIB)]);
    assert_eq!(alone.len(), 1);
}

/// Real entries of several shapes, pushed the way `execute()` pushes them.
fn mixed_stack() -> UndoStack {
    let mut stack = UndoStack::new();
    stack.push_with_data(
        Command::DeleteSection { section_id: 1 },
        UndoData::ProjectSnapshotPair {
            before: format!("{{\"sections\":[{}]}}", "1,".repeat(5000)),
            after: "{\"sections\":[]}".to_string(),
        },
    );
    stack.push_with_data(
        Command::RemoveLoopClosure { closure_id: 3 },
        UndoData::InverseCommand(Command::RemoveLoopClosure { closure_id: 3 }),
    );
    stack.push_with_data(
        Command::ReverseSection {
            section_id: 2,
            swap_left_right: true,
        },
        UndoData::ProjectSnapshot("{\"name\":\"CaveA\"}".to_string()),
    );
    stack
}

/// The stored rows are byte for byte the JSON rows gzipped the way the
/// database gzips them.
fn assert_stored_rows_are_fresh(stack: &UndoStack, which: PersistStack) {
    let json = stack.rows_to_persist(which, None);
    let stored = stack.stored_rows_to_persist(which, None);
    assert_eq!(json.len(), stored.len());
    for ((command, undo_data, timestamp), row) in json.iter().zip(&stored) {
        assert_eq!(&*row.command, compress_history_json(command).as_slice());
        assert_eq!(&*row.undo_data, compress_history_json(undo_data).as_slice());
        assert_eq!(&row.timestamp, timestamp);
    }
}

#[test]
fn stored_rows_are_compressed_once_and_reused() {
    let stack = mixed_stack();
    assert_stored_rows_are_fresh(&stack, PersistStack::Undo);
    let first = stack.stored_rows_to_persist(PersistStack::Undo, None);
    let again = stack.stored_rows_to_persist(PersistStack::Undo, None);
    for (a, b) in first.iter().zip(&again) {
        assert!(Arc::ptr_eq(&a.command, &b.command), "command recompressed");
        assert!(
            Arc::ptr_eq(&a.undo_data, &b.undo_data),
            "undo data recompressed"
        );
    }
}

#[test]
fn a_moved_or_changed_entry_is_compressed_again() {
    let mut stack = mixed_stack();
    let before = stack.stored_rows_to_persist(PersistStack::Undo, None);

    // Undo moves the newest entry to the redo stack: it is compressed afresh there.
    let entry = stack.pop_undo().unwrap();
    stack.push_redo(entry);
    assert_stored_rows_are_fresh(&stack, PersistStack::Redo);
    assert_stored_rows_are_fresh(&stack, PersistStack::Undo);
    let after = stack.stored_rows_to_persist(PersistStack::Undo, None);
    assert!(Arc::ptr_eq(&before[0].undo_data, &after[0].undo_data));

    // Any change to an entry in place drops its compressed form.
    if let Some(slot) = stack.undo_stack.back_mut() {
        slot.undo_data = UndoData::ProjectSnapshot("{\"changed\":true}".to_string());
    }
    assert_stored_rows_are_fresh(&stack, PersistStack::Undo);
}

type Row = (String, String, Option<String>);

fn without_sequence(rows: Vec<(i64, String, String, Option<String>)>) -> Vec<Row> {
    rows.into_iter().map(|(_, c, u, t)| (c, u, t)).collect()
}

#[test]
fn stored_rows_round_trip_through_the_database() {
    let mut stack = mixed_stack();
    let entry = stack.pop_undo().unwrap();
    stack.push_redo(entry);
    let db = Database::in_memory().unwrap();
    db.replace_history_stacks_stored(
        &stack.stored_rows_to_persist(PersistStack::Undo, None),
        &stack.stored_rows_to_persist(PersistStack::Redo, None),
    )
    .unwrap();
    assert_eq!(
        without_sequence(db.load_undo_stack_with_timestamps().unwrap()),
        stack.rows_to_persist(PersistStack::Undo, None)
    );
    assert_eq!(
        without_sequence(db.load_redo_stack_with_timestamps().unwrap()),
        stack.rows_to_persist(PersistStack::Redo, None)
    );
}
