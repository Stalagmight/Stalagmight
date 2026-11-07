use anyhow::{Context, Result};
use rusqlite::params;
use rusqlite::types::ValueRef;
use std::collections::HashMap;

use super::schema_guard::declares_virtual_table;
use super::{format_blob_placeholder, Database, RawTableData, MAX_RAW_PAGE_ROWS};

impl Database {
    /// Upsert or clear the SpeleoDB hand-off link (`speleo_link`, schema v9)
    /// without rewriting the whole project. `Some(link)` upserts the
    /// single row id=1; `None` deletes it. `base_commit_sha` binds as
    /// `Option<String>` so `None` becomes SQL NULL - **never** `""` (NULL is the
    /// empty-repo BASE sentinel).
    ///
    /// FRESHNESS-FOLD SEAM (D-B1): #185's get_project/list_projects return the
    /// authoritative Project on every call. The persistence fold (cache the
    /// freshness verdict + reset last_verify_at) is a DOCUMENTED NO-OP until the
    /// #202 piggyback path consumes it. base_commit_sha is captured ONCE on pull
    /// and NEVER mutated by #189 (a later Sync/Release issue re-stamps it).
    pub fn save_speleo_link(&self, link: Option<&crate::models::SpeleoLink>) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS speleo_link (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                endpoint TEXT NOT NULL,
                project_id TEXT NOT NULL,
                project_name TEXT NOT NULL,
                base_commit_sha TEXT,
                has_unpushed_edits INTEGER NOT NULL DEFAULT 0,
                last_edit_at TEXT,
                last_verify_at TEXT,
                undo_mode TEXT
            )",
            [],
        )?;
        match link {
            Some(link) => {
                self.conn.execute(
                    "INSERT INTO speleo_link
                        (id, endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits, last_edit_at, last_verify_at, undo_mode)
                     VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(id) DO UPDATE SET
                        endpoint = excluded.endpoint,
                        project_id = excluded.project_id,
                        project_name = excluded.project_name,
                        base_commit_sha = excluded.base_commit_sha,
                        has_unpushed_edits = excluded.has_unpushed_edits,
                        last_edit_at = excluded.last_edit_at,
                        last_verify_at = excluded.last_verify_at,
                        undo_mode = excluded.undo_mode",
                    params![
                        link.endpoint,
                        link.project_id,
                        link.project_name,
                        link.base_commit_sha, // Option<String> → SQL NULL when None
                        i64::from(link.has_unpushed_edits),
                        link.last_edit_at, // Option<String> → SQL NULL when None
                        link.last_verify_at, // Option<String> → SQL NULL when None
                        // [#303] Option<UndoMode> → SQL NULL when None ("follow the
                        // global default"), never the string "full": the two are
                        // different states, and only NULL keeps tracking a later change
                        // to the global setting.
                        link.undo_mode.map(crate::models::UndoMode::as_db_str),
                    ],
                )?;
            }
            None => {
                self.conn.execute("DELETE FROM speleo_link", [])?;
            }
        }
        Ok(())
    }

    /// [#303] Record how far the undo history LOADED by a chain step reaches, as the
    /// instant everything at or before it was inherited rather than authored here.
    /// `None` clears it ("nothing in the stack came from a loaded revision").
    ///
    /// A timestamp rather than a stack position, and that is not a detail. Entries are
    /// written back with `sequence` = position, so a position watermark stops meaning
    /// anything the moment the stack shrinks: load two entries (0, 1), undo one, edit, and
    /// the new entry is written at 1 - the id a LOADED entry had. Cutting `sequence <= 1`
    /// would delete the user's own work. A timestamp belongs to the entry rather than to
    /// its slot, so it survives undo, redo and truncation alike.
    ///
    /// Its own table rather than a column on `speleo_undo_chain`, because the two have
    /// different lifetimes: a forward edit clears the chain pointer, and must NOT clear
    /// this - outliving the edit is the entire point of it.
    pub fn save_speleo_undo_watermark(&self, inherited_through: Option<&str>) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS speleo_undo_watermark (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                inherited_through TEXT NOT NULL
            )",
            [],
        )?;
        match inherited_through {
            Some(ts) => {
                self.conn.execute(
                    "INSERT INTO speleo_undo_watermark (id, inherited_through) VALUES (1, ?1)
                     ON CONFLICT(id) DO UPDATE SET inherited_through = excluded.inherited_through",
                    params![ts],
                )?;
            }
            None => {
                self.conn.execute("DELETE FROM speleo_undo_watermark", [])?;
            }
        }
        Ok(())
    }

    /// [#303] The instant through which this copy's undo stack was inherited from a loaded
    /// revision, if any. `None` means every entry was authored in this session, so a push
    /// carries the lot - which is the ordinary lock-edit-push case.
    pub fn load_speleo_undo_watermark(&self) -> Result<Option<String>> {
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='speleo_undo_watermark'",
            [],
            |row| row.get(0),
        )?;
        if exists == 0 {
            return Ok(None);
        }
        match self.conn.query_row(
            "SELECT inherited_through FROM speleo_undo_watermark WHERE id = 1",
            [],
            |row| row.get::<_, String>(0),
        ) {
            Ok(ts) => Ok(Some(ts)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// [#303] Record which SpeleoDB revision the working copy has been UNWOUND to, so
    /// the next Unwind walks to that revision's first parent instead of back to BASE.
    /// `None` clears it ("not mid-unwind").
    ///
    /// A separate single-row table rather than a `speleo_link` column because it is
    /// per-MACHINE progress through history, not a property of the project: it is listed
    /// in `PUSH_SNAPSHOT_LOCAL_TABLES`, so it never rides to the server. A restore stamps
    /// BASE at the server tip (so a later push lands forward, which is what we want), and
    /// that is precisely why BASE cannot double as this pointer.
    pub fn save_speleo_undo_chain(&self, unwound_from_sha: Option<&str>) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS speleo_undo_chain (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                unwound_from_sha TEXT NOT NULL
            )",
            [],
        )?;
        match unwound_from_sha {
            Some(sha) => {
                self.conn.execute(
                    "INSERT INTO speleo_undo_chain (id, unwound_from_sha) VALUES (1, ?1)
                     ON CONFLICT(id) DO UPDATE SET unwound_from_sha = excluded.unwound_from_sha",
                    params![sha],
                )?;
            }
            None => {
                self.conn.execute("DELETE FROM speleo_undo_chain", [])?;
            }
        }
        Ok(())
    }

    /// [#303] The revision the working copy was last unwound to, if any. `None` means
    /// "not mid-unwind", so the next Unwind targets BASE - the commit the working copy
    /// already equals, whose history is what a push just cleared locally.
    pub fn load_speleo_undo_chain(&self) -> Result<Option<String>> {
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='speleo_undo_chain'",
            [],
            |row| row.get(0),
        )?;
        if exists == 0 {
            return Ok(None);
        }
        match self.conn.query_row(
            "SELECT unwound_from_sha FROM speleo_undo_chain WHERE id = 1",
            [],
            |row| row.get::<_, String>(0),
        ) {
            Ok(sha) => Ok(Some(sha)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Flip the SpeleoDB dirty bit and refresh `last_edit_at` in a SINGLE targeted
    /// `UPDATE`, without loading + re-serialising the whole link. Returns `true` when
    /// a remote-link row was updated (`false` = local-only project: no row, or no
    /// `speleo_link` table at all - both are a no-op).
    ///
    /// Hot path: called on EVERY accepted remote edit, so it avoids the per-edit
    /// `SELECT` + `CREATE TABLE IF NOT EXISTS` + full upsert that load+save did (#197
    /// review): the common (linked) case is exactly one `UPDATE`, zero added cost.
    /// Schema init/migrate is supposed to guarantee `speleo_link` exists, but in
    /// practice a never-linked DB can be missing it (#484), so `table_exists` is
    /// consulted ONLY after the `UPDATE` fails, to tell that once-per-session missing
    /// table apart from a genuinely unexpected persistence error - the former is a
    /// silent no-op (and no more per-edit WARN log noise), the latter still
    /// propagates. This never touches `base_commit_sha`/`endpoint`/`project_id`, so it
    /// cannot disturb the BASE-advance invariant or the immutable hand-off identity.
    pub fn touch_speleo_link_dirty(&self, last_edit_at: &str) -> Result<bool> {
        match self.conn.execute(
            "UPDATE speleo_link SET has_unpushed_edits = 1, last_edit_at = ?1 WHERE id = 1",
            params![last_edit_at],
        ) {
            Ok(rows) => Ok(rows > 0),
            Err(e) => {
                if self.table_exists("speleo_link")? {
                    Err(e.into())
                } else {
                    Ok(false)
                }
            }
        }
    }

    /// Change the per-project undo policy and record the change as forward work in one
    /// transaction. Clearing the chain pointer is part of the same commit: leaving it
    /// behind would make the UI classify this real edit as a disposable Unwind step.
    pub fn set_speleo_undo_mode_and_mark_forward_edit(
        &self,
        mode: crate::models::UndoMode,
        last_edit_at: &str,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let rows = tx.execute(
            "UPDATE speleo_link
             SET undo_mode = ?1, has_unpushed_edits = 1, last_edit_at = ?2
             WHERE id = 1",
            params![mode.as_db_str(), last_edit_at],
        )?;
        if rows == 0 {
            anyhow::bail!("No SpeleoDB link row to update");
        }
        tx.execute("DELETE FROM speleo_undo_chain", [])?;
        tx.commit()?;
        Ok(())
    }

    /// Rename the project's display name without rewriting the whole project.
    /// A targeted `UPDATE` on the single `project` row; leaves `modified_at`
    /// untouched so a rename doesn't reorder the recent list by edit time.
    pub fn save_project_name(&self, name: &str) -> Result<()> {
        let affected = self
            .conn
            .execute("UPDATE project SET name = ?1 WHERE id = 1", params![name])
            .context("Failed to update project name")?;
        if affected == 0 {
            anyhow::bail!("No project row to rename");
        }
        Ok(())
    }

    /// Load all display settings from database
    pub fn load_display_settings(&self) -> Result<HashMap<String, String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM display_settings")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut settings = HashMap::new();
        for row in rows {
            let (key, value) = row?;
            settings.insert(key, value);
        }
        Ok(settings)
    }

    /// Save a display setting to database
    pub fn save_display_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO display_settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    /// Get a single display setting from database
    pub fn get_display_setting(&self, key: &str) -> Result<Option<String>> {
        let result = self.conn.query_row(
            "SELECT value FROM display_settings WHERE key = ?1",
            params![key],
            |row| row.get(0),
        );
        match result {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// List the ordinary tables in the database, excluding sqlite-internal ones.
    ///
    /// Virtual tables are left out: a `.stalx` can come from anyone, and touching
    /// a virtual table runs its module's code (FTS5, RTREE) over the file's own
    /// data. Classified from `sqlite_master` alone (#608): it cannot be shadowed
    /// (`sqlite_` names are reserved) and reading it connects nothing, unlike
    /// `pragma_table_list`, which a file can shadow with its own table or view
    /// and which prepares every virtual table it reports. Shadow tables are plain
    /// b-trees, so they stay listed.
    pub fn list_raw_table_names(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, rootpage, sql FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;

        let mut names = Vec::new();
        for row in rows {
            let (name, rootpage, sql) = row?;
            if rootpage != 0 && !declares_virtual_table(sql.as_deref().unwrap_or("")) {
                names.push(name);
            }
        }
        Ok(names)
    }

    /// Read a page of rows from a raw table, with every value stringified
    /// (NULL → `"NULL"`, blobs → a size placeholder like `<blob 4.2 KB>`).
    ///
    /// Injection guard: `table` must be an exact member of
    /// [`Self::list_raw_table_names`]; the identifier is double-quoted and
    /// LIMIT/OFFSET are bound parameters. `limit` is clamped to
    /// [`MAX_RAW_PAGE_ROWS`].
    pub fn read_raw_table(&self, table: &str, limit: u32, offset: u32) -> Result<RawTableData> {
        let known = self.list_raw_table_names()?;
        if !known.iter().any(|name| name == table) {
            anyhow::bail!("Unknown table: {table}");
        }
        let limit = limit.min(MAX_RAW_PAGE_ROWS);
        let ident = format!("\"{}\"", table.replace('"', "\"\""));

        let total_rows: u32 =
            self.conn
                .query_row(&format!("SELECT COUNT(*) FROM {ident}"), [], |row| {
                    row.get(0)
                })?;

        let mut stmt = self
            .conn
            .prepare(&format!("SELECT * FROM {ident} LIMIT ?1 OFFSET ?2"))?;
        let columns: Vec<String> = stmt.column_names().map(str::to_string).collect();
        let column_count = columns.len();
        let mapped = stmt.query_map(params![limit, offset], |row| {
            let mut values = Vec::with_capacity(column_count);
            for i in 0..column_count {
                values.push(match row.get_ref(i)? {
                    ValueRef::Null => "NULL".to_string(),
                    ValueRef::Integer(v) => v.to_string(),
                    ValueRef::Real(v) => v.to_string(),
                    ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                    ValueRef::Blob(b) => format_blob_placeholder(b.len()),
                });
            }
            Ok(values)
        })?;

        let mut rows = Vec::new();
        for row in mapped {
            rows.push(row?);
        }
        Ok(RawTableData {
            columns,
            rows,
            total_rows,
        })
    }
}
