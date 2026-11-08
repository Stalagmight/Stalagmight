//! `.plnx` sibling store (#244): named dive-plan snapshots in a standalone SQLite
//! file next to the `.stalx`. Deliberately independent of the `.stalx` [`Database`]
//! (different schema, different lifecycle) - it reuses only the rusqlite plumbing.
//!
//! Atomicity/durability come from SQLite's in-place ACID transactions (rollback
//! journal + `synchronous=FULL`), which is the right model for an incrementally
//! maintained store: a single insert/update/delete either commits whole or rolls
//! back, and a power loss leaves the old-or-new committed file, never a torn one.
//! (The `.stalx` uses tmp->rename because it rewrites the whole file from the
//! in-memory project; a maintained row store does not.)
//!
//! The snapshot payload stays an opaque JSON string here; serde/model types live in
//! the state layer that owns the domain (`core::state::dive_plan`).

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

/// `.plnx` file/table schema version (distinct from the payload schema version).
const PLNX_SCHEMA_VERSION: i32 = 1;

/// A dive plan's metadata row (no payload) for cheap listing.
pub struct PlnxMeta {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A full dive plan row: metadata plus the opaque snapshot JSON.
pub struct PlnxRow {
    pub meta: PlnxMeta,
    pub snapshot_json: String,
}

pub struct PlnxStore {
    conn: Connection,
}

impl PlnxStore {
    /// Open or create the `.plnx` at `path`, ensuring the schema. Identity metadata
    /// (which project this file belongs to) is stamped separately via
    /// [`ensure_identity`](Self::ensure_identity).
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("Failed to open dive-plan file")?;
        // Crash-safe in-place writes; ignore pragma failures (defaults are already safe).
        let _ = conn.pragma_update(None, "journal_mode", "DELETE");
        let _ = conn.pragma_update(None, "synchronous", "FULL");
        // Two overlapping saves each open their own connection; wait briefly for the lock
        // instead of surfacing an immediate SQLITE_BUSY as "could not save".
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        let store = Self { conn };
        store.init_schema()?;
        store.set_user_version(PLNX_SCHEMA_VERSION)?;
        Ok(store)
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        let conn =
            Connection::open_in_memory().context("Failed to create in-memory dive-plan db")?;
        let store = Self { conn };
        store.init_schema()?;
        store.set_user_version(PLNX_SCHEMA_VERSION)?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS dive_plans (
                 id             INTEGER PRIMARY KEY AUTOINCREMENT,
                 name           TEXT    NOT NULL,
                 created_at     TEXT    NOT NULL,
                 updated_at     TEXT    NOT NULL,
                 schema_version INTEGER NOT NULL,
                 snapshot_json  TEXT    NOT NULL
             );
             CREATE TABLE IF NOT EXISTS plnx_meta (
                 id                 INTEGER PRIMARY KEY CHECK (id = 1),
                 project_created_at TEXT,
                 project_name       TEXT,
                 app_schema_version INTEGER
             );",
        )?;
        Ok(())
    }

    fn set_user_version(&self, v: i32) -> Result<()> {
        self.conn.pragma_update(None, "user_version", v)?;
        Ok(())
    }

    /// The project identity (`created_at`) this file was stamped with, if any.
    pub fn stored_project_created_at(&self) -> Result<Option<String>> {
        match self.conn.query_row(
            "SELECT project_created_at FROM plnx_meta WHERE id = 1",
            [],
            |row| row.get::<_, Option<String>>(0),
        ) {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Read-only identity probe: opens `path` (creating an empty file handle if it does
    /// not exist yet - `Connection::open` always creates the file, but never a table)
    /// and reads the stamped project identity WITHOUT calling [`init_schema`](Self::init_schema)
    /// or [`set_user_version`](Self::set_user_version) - safe to call on a `.plnx` that may
    /// belong to a different project, since ownership must be resolved BEFORE anything is
    /// ever allowed to mutate a `Foreign` file (#244 Copilot finding: `open()` unconditionally
    /// initializes schema/pragma before ownership was checked).
    ///
    /// Returns `Ok(None)` when the file has no `plnx_meta` table yet (schema never
    /// initialized - a pre-schema or brand-new file) or no row 1 in it (a `Fresh` file
    /// that reached [`init_schema`](Self::init_schema) but not
    /// [`ensure_identity`](Self::ensure_identity)). Only the "no such table" case is
    /// swallowed; any other SQLite error (corruption, I/O, permissions) propagates.
    pub fn probe_stored_project_created_at(path: &Path) -> Result<Option<String>> {
        let conn = Connection::open(path).context("Failed to open dive-plan file")?;
        match conn.query_row(
            "SELECT project_created_at FROM plnx_meta WHERE id = 1",
            [],
            |row| row.get::<_, Option<String>>(0),
        ) {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            // No `plnx_meta` table yet (schema was never initialized on this file) - not
            // owned by anyone. "No such table" has the bare SQLITE_ERROR code, so match on
            // the message. rusqlite reports it as SqliteFailure, or as SqlInputError when
            // SQLite also gives a byte offset into the SQL; accept both (guard test below,
            // rusqlite 0.41 / SQLite 3.54). Any other failure (corruption, I/O) propagates.
            Err(
                rusqlite::Error::SqliteFailure(_, Some(ref msg))
                | rusqlite::Error::SqlInputError { ref msg, .. },
            ) if msg.contains("no such table") => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Stamp the file's project identity if it is not stamped yet (a no-op once set,
    /// so re-saving never rewrites it).
    pub fn ensure_identity(&self, project_created_at: &str, project_name: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO plnx_meta (id, project_created_at, project_name, app_schema_version)
             VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO NOTHING",
            rusqlite::params![project_created_at, project_name, PLNX_SCHEMA_VERSION],
        )?;
        Ok(())
    }

    /// All plans, newest activity first.
    pub fn list(&self) -> Result<Vec<PlnxMeta>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, created_at, updated_at
               FROM dive_plans ORDER BY updated_at DESC, id DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(PlnxMeta {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    created_at: r.get(2)?,
                    updated_at: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// One plan's full row (with snapshot), or `None` if `id` is unknown.
    pub fn get(&self, id: i64) -> Result<Option<PlnxRow>> {
        match self.conn.query_row(
            "SELECT id, name, created_at, updated_at, snapshot_json
               FROM dive_plans WHERE id = ?1",
            [id],
            |r| {
                Ok(PlnxRow {
                    meta: PlnxMeta {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        created_at: r.get(2)?,
                        updated_at: r.get(3)?,
                    },
                    snapshot_json: r.get(4)?,
                })
            },
        ) {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Insert a new plan; `now` seeds both created_at and updated_at.
    pub fn insert(
        &self,
        name: &str,
        now: &str,
        schema_version: i32,
        snapshot_json: &str,
    ) -> Result<PlnxMeta> {
        self.conn.execute(
            "INSERT INTO dive_plans (name, created_at, updated_at, schema_version, snapshot_json)
             VALUES (?1, ?2, ?2, ?3, ?4)",
            rusqlite::params![name, now, schema_version, snapshot_json],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(PlnxMeta {
            id,
            name: name.to_string(),
            created_at: now.to_string(),
            updated_at: now.to_string(),
        })
    }

    /// Update an existing plan's name + snapshot, bumping updated_at. Returns the
    /// refreshed metadata, or `None` if `id` is unknown.
    pub fn update(
        &self,
        id: i64,
        name: &str,
        now: &str,
        schema_version: i32,
        snapshot_json: &str,
    ) -> Result<Option<PlnxMeta>> {
        let changed = self.conn.execute(
            "UPDATE dive_plans
                SET name = ?2, updated_at = ?3, schema_version = ?4, snapshot_json = ?5
              WHERE id = ?1",
            rusqlite::params![id, name, now, schema_version, snapshot_json],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        Ok(self.get(id)?.map(|row| row.meta))
    }

    /// Delete a plan; `true` if a row was removed.
    pub fn delete(&self, id: i64) -> Result<bool> {
        let removed = self
            .conn
            .execute("DELETE FROM dive_plans WHERE id = ?1", [id])?;
        Ok(removed > 0)
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "tests build from known-valid literals; a failed unwrap should fail the test"
    )]
    use super::*;

    #[test]
    fn insert_get_list_delete_round_trip() {
        let store = PlnxStore::in_memory().unwrap();
        assert!(store.list().unwrap().is_empty());

        let a = store
            .insert("Deep push", "2026-07-06T10:00:00Z", 1, r#"{"a":1}"#)
            .unwrap();
        let b = store
            .insert("Shallow", "2026-07-06T11:00:00Z", 1, r#"{"b":2}"#)
            .unwrap();
        assert_ne!(a.id, b.id);

        // Newest updated_at first.
        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, b.id);
        assert_eq!(list[1].id, a.id);

        let got = store.get(a.id).unwrap().unwrap();
        assert_eq!(got.meta.name, "Deep push");
        assert_eq!(got.snapshot_json, r#"{"a":1}"#);

        assert!(store.delete(a.id).unwrap());
        assert!(!store.delete(a.id).unwrap()); // second delete is a no-op
        assert_eq!(store.list().unwrap().len(), 1);
        assert!(store.get(a.id).unwrap().is_none());
    }

    #[test]
    fn update_changes_snapshot_and_bumps_updated_at() {
        let store = PlnxStore::in_memory().unwrap();
        let m = store
            .insert("Plan", "2026-07-06T10:00:00Z", 1, r#"{"v":1}"#)
            .unwrap();

        let updated = store
            .update(m.id, "Plan v2", "2026-07-06T12:00:00Z", 1, r#"{"v":2}"#)
            .unwrap()
            .unwrap();
        assert_eq!(updated.name, "Plan v2");
        assert_eq!(updated.created_at, "2026-07-06T10:00:00Z"); // created_at is preserved
        assert_eq!(updated.updated_at, "2026-07-06T12:00:00Z"); // updated_at bumped
        assert_eq!(
            store.get(m.id).unwrap().unwrap().snapshot_json,
            r#"{"v":2}"#
        );
    }

    #[test]
    fn update_missing_id_returns_none() {
        let store = PlnxStore::in_memory().unwrap();
        assert!(store
            .update(999, "x", "2026-07-06T10:00:00Z", 1, "{}")
            .unwrap()
            .is_none());
    }

    #[test]
    fn identity_stamp_is_write_once() {
        let store = PlnxStore::in_memory().unwrap();
        assert!(store.stored_project_created_at().unwrap().is_none());

        store
            .ensure_identity("2026-01-01T00:00:00Z", "Cave A")
            .unwrap();
        assert_eq!(
            store.stored_project_created_at().unwrap().as_deref(),
            Some("2026-01-01T00:00:00Z")
        );

        // A second (different) stamp must NOT overwrite the first - the file keeps
        // its original owner so a re-save can never silently re-key it.
        store
            .ensure_identity("2099-12-31T00:00:00Z", "Cave B")
            .unwrap();
        assert_eq!(
            store.stored_project_created_at().unwrap().as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
    }

    #[test]
    fn probe_stored_project_created_at_on_a_never_opened_file_is_none_and_does_not_init_schema() {
        let dir = std::env::temp_dir().join(format!("plnx_probe_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("probe.plnx");
        let _ = std::fs::remove_file(&path);

        // The file does not exist yet at all - Connection::open creates an empty sqlite
        // file with no tables and user_version 0.
        assert_eq!(
            PlnxStore::probe_stored_project_created_at(&path).unwrap(),
            None
        );

        // The probe must NOT have run init_schema/set_user_version: reopening via a real
        // Connection shows user_version is still 0 and no dive_plans/plnx_meta tables exist.
        let conn = Connection::open(&path).unwrap();
        let user_version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(user_version, 0, "probe must not touch user_version");
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('dive_plans', 'plnx_meta')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 0, "probe must not create any tables");
        drop(conn);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn probe_stored_project_created_at_on_a_stamped_file_reads_it_without_mutating() {
        let dir = std::env::temp_dir().join(format!("plnx_probe_test2_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("probe_stamped.plnx");
        let _ = std::fs::remove_file(&path);

        {
            let store = PlnxStore::open(&path).unwrap();
            store
                .ensure_identity("2026-01-01T00:00:00Z", "Cave A")
                .unwrap();
        }

        assert_eq!(
            PlnxStore::probe_stored_project_created_at(&path)
                .unwrap()
                .as_deref(),
            Some("2026-01-01T00:00:00Z")
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn persists_across_reopen_on_a_temp_file() {
        let dir = std::env::temp_dir().join(format!("plnx_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cave.plnx");
        let _ = std::fs::remove_file(&path);

        {
            let store = PlnxStore::open(&path).unwrap();
            store
                .ensure_identity("2026-05-05T00:00:00Z", "Cave")
                .unwrap();
            store
                .insert("P1", "2026-07-06T10:00:00Z", 1, r#"{"x":1}"#)
                .unwrap();
        }
        {
            let store = PlnxStore::open(&path).unwrap();
            assert_eq!(
                store.stored_project_created_at().unwrap().as_deref(),
                Some("2026-05-05T00:00:00Z")
            );
            let list = store.list().unwrap();
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].name, "P1");
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
