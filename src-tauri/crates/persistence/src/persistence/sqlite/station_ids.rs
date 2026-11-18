//! Station IDs: the `station_ids` side table and the `next_station_id`
//! project setting.
//!
//! Every station's project-unique Station ID (`Project::station_ids`) is
//! stored positionally, `(section_id, station_index, station_id)` with the
//! station as the primary key, like `station_names`; the allocation counter
//! (`Project::next_station_id`) is the `project_settings` row
//! `next_station_id`. Both arrived WITHOUT a schema bump, for the reason
//! `station_texts` gives: SpeleoDB's Python reader accepts exactly v18 and
//! reads only the tables it knows (`project_settings` by key), so a new table
//! and a new key are invisible to it. Plain DDL (no CHECK, no partial or
//! expression index, no DEFAULT expression) so the schema guard admits it.
//!
//! Tolerant read: a missing table or key reads as none; a table of another
//! shape gives none, with a warning; an unreadable row is skipped and counted.
//! What is missing is re-minted on open, in line order then station order
//! (`Project::ensure_station_identities`), so a file an older build rewrote
//! without the table - an older build saves by writing a fresh file - opens
//! with the same ids every time. SQLite failing for a reason of its own (busy,
//! I/O, corrupt) still fails the open, so a save cannot then drop the rows.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{params, Connection, Transaction};

use crate::models::{Project, StationId, StationKey};

use super::shot_credits::{extended_code, is_schema_error};

const TABLE: &str = "station_ids";

/// The `project_settings` key of `Project::next_station_id`.
pub(super) const NEXT_STATION_ID_KEY: &str = "next_station_id";

/// DDL for `station_ids`, shared by `init_schema`, `migrate_schema` and the
/// save (a v18 file opened through the fast path may not have it yet).
pub(super) const CREATE_STATION_IDS: &str = "CREATE TABLE IF NOT EXISTS station_ids (
                section_id INTEGER NOT NULL REFERENCES sections(id) ON DELETE CASCADE,
                station_index INTEGER NOT NULL,
                station_id INTEGER NOT NULL,
                PRIMARY KEY (section_id, station_index)
            )";

/// Write every station's id (orphans skipped, as for the station texts) and
/// the counter. The caller has emptied the table.
pub(super) fn write_station_ids(tx: &Transaction<'_>, project: &Project) -> Result<()> {
    let mut insert = tx.prepare_cached(
        "INSERT OR REPLACE INTO station_ids (section_id, station_index, station_id) \
         VALUES (?1, ?2, ?3)",
    )?;
    for (key, id) in &project.station_ids {
        let Some(sr) = project.resolve_key(key) else {
            tracing::warn!(
                purpose = "skip orphan station id on save",
                "Dropping a Station ID whose station no longer resolves"
            );
            continue;
        };
        insert.execute(params![sr.line_id, sr.station_index, id])?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO project_settings (key, value) VALUES (?1, ?2)",
        params![NEXT_STATION_ID_KEY, project.next_station_id.to_string()],
    )?;
    Ok(())
}

/// Whether `error` says the table is of another shape than this build reads
/// (the read then gives no rows), rather than SQLite failing on its own.
fn degrade<T: Default>(read: rusqlite::Result<T>) -> Result<T> {
    match read {
        Ok(value) => Ok(value),
        Err(error) if is_schema_error(&error) => {
            tracing::warn!(
                table = TABLE,
                sqlite_code = ?extended_code(&error),
                purpose = "read station ids",
                "The station id table cannot be read; every station's id is re-minted"
            );
            Ok(T::default())
        }
        Err(error) => Err(anyhow::Error::new(error).context("Failed to read the station id table")),
    }
}

/// Load `station_ids` as transitional positional keys (the load re-anchor
/// pass canonicalizes them). See the module doc for what is tolerated.
pub(super) fn load_station_ids(conn: &Connection) -> Result<HashMap<StationKey, StationId>> {
    let is_table: bool = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [TABLE],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)?;
    if !is_table {
        return Ok(HashMap::new());
    }
    degrade(read_rows(conn))
}

fn read_rows(conn: &Connection) -> rusqlite::Result<HashMap<StationKey, StationId>> {
    let mut stmt = conn.prepare("SELECT section_id, station_index, station_id FROM station_ids")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0).ok(),
            row.get::<_, i64>(1).ok(),
            row.get::<_, i64>(2).ok(),
        ))
    })?;
    let mut ids = HashMap::new();
    let mut skipped = 0_usize;
    for row in rows {
        let (line, index, id) = row?;
        let decoded = (|| {
            Some((
                u32::try_from(line?).ok()?,
                u32::try_from(index?).ok()?,
                // Past the bound is unreadable too: the station is
                // re-minted (`MAX_STATION_ID`).
                StationId::try_from(id?)
                    .ok()
                    .filter(|id| *id < crate::models::MAX_STATION_ID)?,
            ))
        })();
        match decoded {
            Some((line, index, id)) => {
                ids.insert(StationKey::positional(line, index), id);
            }
            None => skipped += 1,
        }
    }
    if skipped > 0 {
        tracing::warn!(
            table = TABLE,
            skipped,
            purpose = "read station ids",
            "Skipped unreadable station id rows; their stations get new ids"
        );
    }
    Ok(ids)
}

/// The stored `next_station_id`, or `None` when the row (or the table) is
/// missing or does not read as an id. The open then derives it from the ids.
pub(super) fn load_next_station_id(conn: &Connection) -> Option<StationId> {
    conn.query_row(
        "SELECT value FROM project_settings WHERE key = ?1",
        [NEXT_STATION_ID_KEY],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|value| value.trim().parse::<StationId>().ok())
    .filter(|next| *next <= crate::models::MAX_STATION_ID)
}
