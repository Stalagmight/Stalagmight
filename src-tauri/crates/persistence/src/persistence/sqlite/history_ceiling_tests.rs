//! [#576 NF-W4U-2] The per-open history ceiling is derived from what a save can write.
//!
//! Since #576 B20 a save keeps, per stack, either a window of at most
//! `UNDO_BYTE_BUDGET_BYTES` or one larger newest entry whose biggest blob the per-blob
//! ceiling bounds. So the largest history the app writes - one maximal entry on each
//! stack - must still be admitted, and anything well past it is not one we wrote.
//!
//! Every blob here is a few bytes of gzip whose ISIZE trailer CLAIMS its size: the
//! pre-scan prices a stack by those claims against what the open's budget has LEFT,
//! before a byte is inflated, so a claim is all it takes to test the admission. (The
//! claim does not match the stream, so an admitted blob then fails its own checksum;
//! that later error is not the budget refusal, which is what is asserted.) What the
//! first stack of an open spends is reserved on the shared budget directly.

use chrono::Utc;

use super::{compress_json, Database, MAX_DECOMPRESSED_BLOB_BYTES, MAX_HISTORY_LOAD_BYTES};
use crate::persistence::undo_limits::UNDO_BYTE_BUDGET_BYTES;

const MIB: u32 = 1024 * 1024;

/// A tiny gzip blob that claims to inflate to `declared` bytes.
fn claiming(declared: u32) -> Vec<u8> {
    let mut blob = compress_json(r#"{"undo":"data"}"#);
    let at = blob.len() - 4;
    blob[at..].copy_from_slice(&declared.to_le_bytes());
    blob
}

fn insert_redo(db: &Database, sequence: i64, undo_data_claim: u32) {
    db.conn
        .execute(
            "INSERT INTO redo_stack (sequence, command_json, undo_data_json, executed_at) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                sequence,
                compress_json(r#"{"DeleteSection":{"section_id":0}}"#),
                claiming(undo_data_claim),
                Utc::now().to_rfc3339()
            ],
        )
        .unwrap();
}

/// An open's redo load after its undo stack spent `undo_spent` bytes of the shared
/// budget. Returns the load's error text, if any.
fn redo_after_undo_spent(db: &Database, undo_spent: u64) -> Option<String> {
    let budget = Database::load_history_budget();
    budget.reserve(undo_spent).unwrap();
    db.load_redo_stack_with_timestamps_within(100, &budget)
        .err()
        .map(|e| format!("{e:#}"))
}

fn is_budget_refusal(err: Option<&String>) -> bool {
    err.is_some_and(|e| e.contains("load budget"))
}

#[test]
fn the_ceiling_is_two_stacks_of_the_capped_writer() {
    let window = UNDO_BYTE_BUDGET_BYTES as u64;
    println!(
        "per-open ceiling {} MiB (per-blob {} MiB, saved window {} MiB per stack)",
        MAX_HISTORY_LOAD_BYTES >> 20,
        MAX_DECOMPRESSED_BLOB_BYTES >> 20,
        window >> 20
    );
    assert_eq!(
        MAX_HISTORY_LOAD_BYTES,
        2 * (MAX_DECOMPRESSED_BLOB_BYTES + window)
    );
}

#[test]
fn the_largest_history_a_save_writes_is_admitted() {
    // One maximal newest entry on each stack: its snapshot blob at the per-blob ceiling,
    // plus a command blob (a few hundred bytes; 1 KiB charged).
    let db = Database::in_memory().unwrap();
    let max_blob = u32::try_from(MAX_DECOMPRESSED_BLOB_BYTES).unwrap();
    insert_redo(&db, 0, max_blob);
    let err = redo_after_undo_spent(&db, MAX_DECOMPRESSED_BLOB_BYTES + 1024);
    println!("one maximal entry per stack -> redo load: {err:?}");
    assert!(!is_budget_refusal(err.as_ref()), "refused: {err:?}");

    // A full saved window on each stack (32 x 1 MiB) is admitted too.
    let db = Database::in_memory().unwrap();
    for sequence in 0..32 {
        insert_redo(&db, sequence, MIB);
    }
    let err = redo_after_undo_spent(&db, UNDO_BYTE_BUDGET_BYTES as u64);
    println!("a 32 MiB window per stack -> redo load: {err:?}");
    assert!(!is_budget_refusal(err.as_ref()), "refused: {err:?}");
}

#[test]
fn a_history_past_two_capped_stacks_is_refused() {
    // 600 MiB in all, every blob inside the per-blob ceiling: three entries of 200 MiB.
    // No capped save writes two of them on one stack (the window keeps an oversized
    // newest entry alone), and the pair is past what two capped stacks can hold.
    let db = Database::in_memory().unwrap();
    insert_redo(&db, 0, 200 * MIB);
    let err = redo_after_undo_spent(&db, 2 * 200 * u64::from(MIB));
    println!("undo spent 400 MiB, redo claims 200 MiB -> redo load: {err:?}");
    assert!(
        is_budget_refusal(err.as_ref()),
        "the pair must be refused by the per-open budget, got: {err:?}"
    );
}
