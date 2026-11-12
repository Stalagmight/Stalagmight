//! The `shot_credits` table (#707): who surveyed a shot and when, for the shots
//! whose credit differs from their section's.
//!
//! One row per shot that overrides its section, keyed by the shot's persisted
//! `uuid`. Not by its position: an older build that saves in place rewrites the
//! `shots` rows without knowing this table exists, and a positional key would
//! then quietly attach credit to whatever shot moved into that slot. A uuid row
//! whose shot is gone simply matches nothing. A shot with no row inherits both
//! values; in a row, each column follows [`ShotCredit`]'s encoding: `NULL`
//! inherits that one value, `''` means explicitly unknown, anything else is the
//! shot's own value.
//!
//! Added WITHOUT a schema version bump, for the reason `station_names` was (see
//! `station_texts`): SpeleoDB reads an uploaded `.stalx` through OpenSpeleo's
//! Python `stalagmight_lib`, which refuses any file not at exactly v18 and reads
//! only its known tables by explicit column lists. A new table is invisible to
//! it; new `shots` columns plus a bump would lock it out. The price is the same
//! too: a build without this table drops the overrides when it saves to a fresh
//! file, which every ordinary save does.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::types::ValueRef;
use rusqlite::{params, Transaction};

use crate::models::ShotCredit;

use super::Database;

/// DDL for `shot_credits`, shared by `init_schema`, `migrate_schema` and the
/// save transaction so they cannot drift.
pub(super) const CREATE_SHOT_CREDITS: &str = "CREATE TABLE IF NOT EXISTS shot_credits (
                shot_uuid TEXT PRIMARY KEY,
                surveyor TEXT,
                date TEXT
            )";

/// Write `credit` for the shot saved under `uuid`. The caller has cleared the
/// table inside the same transaction. `OR IGNORE` keeps the first row should two
/// shots ever share a uuid: a save must not fail over credit.
pub(super) fn save_shot_credit(
    tx: &Transaction<'_>,
    uuid: &str,
    credit: &ShotCredit,
) -> Result<()> {
    tx.prepare_cached(
        "INSERT OR IGNORE INTO shot_credits (shot_uuid, surveyor, date) VALUES (?1, ?2, ?3)",
    )?
    .execute(params![
        uuid,
        credit.surveyor_column(),
        credit.date_column()
    ])?;
    Ok(())
}

/// A TEXT cell, `Some(None)` for NULL, or `None` when the cell holds anything
/// else (an INTEGER, REAL or BLOB, or text that is not UTF-8).
fn text_or_null(value: ValueRef<'_>) -> Option<Option<String>> {
    match value {
        ValueRef::Null => Some(None),
        ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok().map(|s| Some(s.to_owned())),
        _ => None,
    }
}

impl Database {
    /// Every stored override, keyed by shot uuid.
    ///
    /// A missing table reads as empty: it arrived without a schema bump, so a
    /// v18 file takes `ensure_schema_version`'s fast path and is never migrated
    /// on open, and read-only opens never migrate at all.
    ///
    /// A row this build cannot read is skipped, never an error: the table only
    /// carries overrides, and one bad cell in a hand-edited or hostile file
    /// must not make the whole project unopenable (the rule the `shots` loader
    /// follows too). The shot then inherits its section's credit.
    pub(super) fn load_shot_credits(&self) -> Result<HashMap<String, ShotCredit>> {
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
            .prepare("SELECT shot_uuid, surveyor, date FROM shot_credits")?;
        let mut rows = stmt.query([])?;
        let mut credits = HashMap::new();
        let mut skipped = 0usize;
        while let Some(row) = rows.next()? {
            let cells = (
                text_or_null(row.get_ref(0)?),
                text_or_null(row.get_ref(1)?),
                text_or_null(row.get_ref(2)?),
            );
            let (Some(Some(uuid)), Some(surveyor), Some(date)) = cells else {
                skipped += 1;
                continue;
            };
            if uuid.is_empty() {
                skipped += 1;
                continue;
            }
            // A row whose columns are both NULL overrides nothing.
            if let Some(credit) = ShotCredit::from_columns(surveyor, date).into_stored() {
                credits.insert(uuid, credit);
            }
        }
        if skipped > 0 {
            tracing::warn!(
                skipped,
                "Skipped unreadable shot_credits rows; those shots inherit"
            );
        }
        Ok(credits)
    }
}
