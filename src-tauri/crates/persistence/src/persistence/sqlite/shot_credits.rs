//! The `shot_credits` table (#707): who surveyed a shot and when, for the shots
//! whose credit differs from their section's.
//!
//! One row per shot that overrides its section, keyed by the shot's position
//! like the `shots` table itself. A shot with no row inherits both values. In a
//! row, each column follows [`ShotCredit`]'s encoding: `NULL` inherits that one
//! value, `''` means explicitly unknown, anything else is the shot's own value.
//!
//! Added WITHOUT a schema version bump, for the reason `station_names` was (see
//! `station_texts`): SpeleoDB reads an uploaded `.stalx` through OpenSpeleo's
//! Python `stalagmight_lib`, which refuses any file not at exactly v18 and reads
//! only its known tables by explicit column lists. A new table is invisible to
//! it; new `shots` columns plus a bump would lock it out. The price is the same
//! too: a build without this table drops the overrides on its next save, since
//! every save rebuilds the file from `init_schema`.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{params, Transaction};

use crate::models::{Project, SectionId, ShotCredit};

use super::Database;

/// DDL for `shot_credits`, shared by `init_schema`, `migrate_schema` and the
/// save transaction so they cannot drift.
pub(super) const CREATE_SHOT_CREDITS: &str = "CREATE TABLE IF NOT EXISTS shot_credits (
                section_id INTEGER NOT NULL,
                sequence_order INTEGER NOT NULL,
                surveyor TEXT,
                date TEXT,
                PRIMARY KEY (section_id, sequence_order),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            )";

/// Write a row for every shot that overrides its section's credit. The caller
/// has already cleared the table inside the same transaction.
pub(super) fn save_shot_credits(tx: &Transaction<'_>, project: &Project) -> Result<()> {
    let mut stmt = tx.prepare(
        "INSERT INTO shot_credits (section_id, sequence_order, surveyor, date) VALUES (?1, ?2, ?3, ?4)",
    )?;
    for section in &project.sections {
        for (seq, shot) in section.shots.iter().enumerate() {
            if let Some(credit) = &shot.credit {
                stmt.execute(params![
                    section.id,
                    seq as i64,
                    credit.surveyor_column(),
                    credit.date_column(),
                ])?;
            }
        }
    }
    Ok(())
}

impl Database {
    /// Every stored override, keyed by `(section_id, sequence_order)`.
    ///
    /// A missing table reads as empty: it arrived without a schema bump, so a
    /// v18 file takes `ensure_schema_version`'s fast path and is never migrated
    /// on open, and read-only opens never migrate at all.
    pub(super) fn load_shot_credits(&self) -> Result<HashMap<(SectionId, u32), ShotCredit>> {
        let table_exists: bool = self
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='shot_credits'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)?;
        if !table_exists {
            return Ok(HashMap::new());
        }
        let mut stmt = self
            .conn
            .prepare("SELECT section_id, sequence_order, surveyor, date FROM shot_credits")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                (row.get::<_, SectionId>(0)?, row.get::<_, u32>(1)?),
                ShotCredit::from_columns(row.get(2)?, row.get(3)?),
            ))
        })?;
        let mut credits = HashMap::new();
        for row in rows {
            let (key, credit) = row?;
            // A row whose columns are both NULL overrides nothing.
            if let Some(credit) = credit.into_stored() {
                credits.insert(key, credit);
            }
        }
        Ok(credits)
    }
}
