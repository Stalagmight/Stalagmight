//! The undo/redo stack writer the 0.6.0 save ends with. The legacy reader
//! never loads history (it only counts the rows), so the loaders, the
//! inflation budget and the per-entry writers were pruned from this copy.

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::params;

use super::{compress_json, Database};

impl Database {
    pub fn replace_history_stacks_with_timestamps(
        &self,
        undo_entries: Vec<(String, String, Option<String>)>,
        redo_entries: Vec<(String, String, Option<String>)>,
    ) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("begin undo/redo replacement transaction")?;
        tx.execute("DELETE FROM undo_stack", [])?;
        tx.execute("DELETE FROM redo_stack", [])?;

        for (seq, (command_json, undo_data_json, timestamp)) in undo_entries.iter().enumerate() {
            tx.execute(
                "INSERT INTO undo_stack (sequence, command_json, undo_data_json, executed_at) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    seq as i64,
                    compress_json(command_json),
                    compress_json(undo_data_json),
                    timestamp.clone().unwrap_or_else(|| Utc::now().to_rfc3339()),
                ],
            )?;
        }
        for (seq, (command_json, undo_data_json, timestamp)) in redo_entries.iter().enumerate() {
            tx.execute(
                "INSERT INTO redo_stack (sequence, command_json, undo_data_json, executed_at) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    seq as i64,
                    compress_json(command_json),
                    compress_json(undo_data_json),
                    timestamp.clone().unwrap_or_else(|| Utc::now().to_rfc3339()),
                ],
            )?;
        }
        tx.commit().context("commit undo/redo replacement")?;
        Ok(())
    }
}
