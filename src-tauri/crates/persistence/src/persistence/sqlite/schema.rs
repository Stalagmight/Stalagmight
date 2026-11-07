use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use rusqlite::{params, Connection, Transaction, TransactionBehavior, MAIN_DB};

use crate::models::FEATURE_ID_STRIDE;

use super::{
    Database, PreReleaseFile, UpgradeRefusal, UpgradeRefused, OLDEST_SUPPORTED_SCHEMA_VERSION,
    SCHEMA_VERSION, UNDO_FORMAT_SAFE_VERSION,
};

/// Columns of `speleo_link` that MUST survive every future SCHEMA_VERSION bump
/// unchanged (name, declared type, null-ness). The carry-forward fence (#190)
/// asserts this with `pragma_table_info` so no later migration silently rewrites
/// the SpeleoDB hand-off identity or drops the dirty bit. `(name, decl_type,
/// notnull)`.
///
/// TEST-ONLY, and always has been: the fence is the assertion in
/// `tests::preserved_speleolink_columns_match_head_schema`, and this is the
/// expected schema it compares the live table against. Nothing at runtime reads
/// it - a bump that rewrote the table fails that test before it can ship, which
/// is where the guarantee comes from - so it is `cfg(test)` rather than dead
/// weight the release build carries and warns about. It stays HERE, beside the
/// DDL it fences, because that is what the two DDL comments below point at.
#[cfg(test)]
pub(super) const PRESERVED_SPELEOLINK_COLUMNS: &[(&str, &str, bool)] = &[
    // `id INTEGER PRIMARY KEY` is a rowid alias: SQLite reports notnull=0 for it
    // in pragma_table_info even though a rowid PK can never be NULL. The fence
    // asserts the column as the schema actually reports it, so notnull is `false`
    // here (the PK + CHECK(id=1) still enforce single-row identity).
    ("id", "INTEGER", false),
    ("endpoint", "TEXT", true),
    ("project_id", "TEXT", true),
    ("project_name", "TEXT", true),
    ("base_commit_sha", "TEXT", false), // nullable = empty-repo BASE
    ("has_unpushed_edits", "INTEGER", true),
    ("last_edit_at", "TEXT", false), // v10; nullable = never edited since pull
    ("last_verify_at", "TEXT", false), // v11; nullable = never re-verified since pull
    ("undo_mode", "TEXT", false),    // v16; nullable only for legacy initialization (#303)
];

impl Database {
    /// Run lightweight schema migrations for tables added after initial release
    fn migrate_schema(&self) -> Result<()> {
        // Ensure display_settings table exists (added in v1.x for per-project display toggles)
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS display_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
            [],
        )?;

        // Ensure project_settings table exists (#57: per-project validation
        // thresholds stored in the .stalx so they travel with the file).
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS project_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
            [],
        )?;

        // Add direction column to sections table if it doesn't exist (added for direction field)
        // Check if column exists first
        let has_direction_column: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sections') WHERE name='direction'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !has_direction_column {
            self.conn.execute(
                "ALTER TABLE sections ADD COLUMN direction TEXT NOT NULL DEFAULT 'in'",
                [],
            )?;
            tracing::info!("Added direction column to sections table");
        }

        // Add the gauge pair to sections if absent (#24). Nullable: NULL means "not captured
        // yet", which falls back to the legacy index rule, so an existing file keeps exactly
        // the gauge station it already had until its first save records the pair.
        for column in ["gauge_shot_uuid", "gauge_neighbour_uuid"] {
            let present: bool = self
                .conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('sections') WHERE name='{column}'"
                    ),
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map(|c| c > 0)
                .unwrap_or(false);
            if !present {
                self.conn.execute(
                    &format!("ALTER TABLE sections ADD COLUMN {column} TEXT"),
                    [],
                )?;
                tracing::info!(column, "Added gauge column to sections table");
            }
        }

        // Add the north-reference quartet to sections if absent (#438, v17 -> v18).
        // All four nullable and NULL by default: that is "no reference recorded",
        // which resolves to a 0 correction, so an existing file computes exactly
        // the geometry it did before the columns existed.
        for (column, decl) in [
            ("azimuth_north", "TEXT"),
            ("azimuth_correction", "REAL"),
            ("azimuth_source", "TEXT"),
            ("azimuth_epoch_year", "INTEGER"),
        ] {
            let present: bool = self
                .conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('sections') WHERE name='{column}'"
                    ),
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map(|c| c > 0)
                .unwrap_or(false);
            if !present {
                self.conn.execute(
                    &format!("ALTER TABLE sections ADD COLUMN {column} {decl}"),
                    [],
                )?;
                tracing::info!(column, "Added azimuth-reference column to sections table");
            }
        }

        // Add reverse_parity to sections if absent (#24). Flips on every section reverse; the
        // least-squares gauge reads it to keep pinning the same physical station. Default 0 is
        // exactly right for an existing file - it means "station 0", which is where the gauge
        // already is, so upgrading moves nobody's coordinates.
        let has_reverse_parity_column: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sections') WHERE name='reverse_parity'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !has_reverse_parity_column {
            self.conn.execute(
                "ALTER TABLE sections ADD COLUMN reverse_parity INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
            tracing::info!("Added reverse_parity column to sections table");
        }

        // Add quality column to sections table if it doesn't exist (added for weighted adjustments)
        let has_quality_column: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('sections') WHERE name='quality'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !has_quality_column {
            self.conn.execute(
                "ALTER TABLE sections ADD COLUMN quality REAL NOT NULL DEFAULT 1.0",
                [],
            )?;
            tracing::info!("Added quality column to sections table");
        }

        // Ensure station_offsets table exists (added for individual station moves)
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS station_offsets (
                section_id INTEGER NOT NULL,
                station_index INTEGER NOT NULL,
                offset_x REAL NOT NULL,
                offset_y REAL NOT NULL,
                offset_z REAL NOT NULL,
                PRIMARY KEY (section_id, station_index),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            )",
            [],
        )?;

        // Ensure resolved_warnings table exists (added for user-resolved warnings)
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS resolved_warnings (
                id INTEGER PRIMARY KEY,
                section_id INTEGER NOT NULL,
                shot_id INTEGER NOT NULL,
                warning_code TEXT NOT NULL,
                resolved_at TEXT NOT NULL,
                resolved_comment TEXT,
                shot_uuid TEXT,
                UNIQUE(section_id, shot_id, warning_code),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            )",
            [],
        )?;

        // Ensure imported_file_hashes table exists (added for duplicate import detection)
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS imported_file_hashes (
                id INTEGER PRIMARY KEY,
                hash TEXT NOT NULL UNIQUE
            )",
            [],
        )?;

        // Ensure imported_section_hashes table exists (section-level duplicate detection)
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS imported_section_hashes (
                id INTEGER PRIMARY KEY,
                hash TEXT NOT NULL UNIQUE
            )",
            [],
        )?;

        // Add closure_id column to loop_closures table if it doesn't exist (added for closure ID tracking)
        let has_closure_id: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('loop_closures') WHERE name='closure_id'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !has_closure_id {
            // Table may not exist in very old projects, check first
            let loop_table_exists: bool = self.conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='loop_closures'",
                [],
                |row| row.get::<_, i64>(0),
            ).map(|c| c > 0).unwrap_or(false);

            if loop_table_exists {
                self.conn.execute(
                    "ALTER TABLE loop_closures ADD COLUMN closure_id INTEGER NOT NULL DEFAULT 0",
                    [],
                )?;
                tracing::info!("Added closure_id column to loop_closures table");
            }
        }

        // Add strategy column to loop_closures table if it doesn't exist
        let has_strategy: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('loop_closures') WHERE name='strategy'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !has_strategy {
            let loop_table_exists: bool = self.conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='loop_closures'",
                [],
                |row| row.get::<_, i64>(0),
            ).map(|c| c > 0).unwrap_or(false);

            if loop_table_exists {
                self.conn.execute(
                    "ALTER TABLE loop_closures ADD COLUMN strategy TEXT NOT NULL DEFAULT 'Batch'",
                    [],
                )?;
                tracing::info!("Added strategy column to loop_closures table");
            }
        }

        // Add is_aligned column to loop_closures table if it doesn't exist
        let has_is_aligned: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('loop_closures') WHERE name='is_aligned'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !has_is_aligned {
            let loop_table_exists: bool = self.conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='loop_closures'",
                [],
                |row| row.get::<_, i64>(0),
            ).map(|c| c > 0).unwrap_or(false);

            if loop_table_exists {
                self.conn.execute(
                    "ALTER TABLE loop_closures ADD COLUMN is_aligned INTEGER NOT NULL DEFAULT 0",
                    [],
                )?;
                tracing::info!("Added is_aligned column to loop_closures table");
            }
        }

        // Add next_closure_id column to project table if it doesn't exist (persists monotonic counter)
        let has_next_closure_id: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('project') WHERE name='next_closure_id'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !has_next_closure_id {
            self.conn.execute(
                "ALTER TABLE project ADD COLUMN next_closure_id INTEGER NOT NULL DEFAULT 1",
                [],
            )?;
            tracing::info!("Added next_closure_id column to project table");
        }

        // Ensure station_comments table exists (added for station-level comments)
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS station_comments (
                section_id INTEGER NOT NULL,
                station_index INTEGER NOT NULL,
                comment TEXT NOT NULL,
                PRIMARY KEY (section_id, station_index),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            )",
            [],
        )?;

        // Ensure speleo_link table exists (schema v9: SpeleoDB hand-off identity
        // + dirty bit, #189). This CREATE IF NOT EXISTS is what makes the table
        // appear on open_existing() of a legacy v8 file (a whole-table
        // CREATE-IF-NOT-EXISTS, not the shot_uuid conditional-ALTER pattern). No ALTER, no backfill. The
        // DDL is byte-identical to init_schema's so the carry-forward fence
        // (PRESERVED_SPELEOLINK_COLUMNS) holds on either open shape.
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

        // Add last_edit_at column to an EXISTING v9 speleo_link (schema v9→v10:
        // guardrail-7 push-start comparison, #193). CREATE IF NOT EXISTS above is
        // a no-op when the table already exists, so a pre-v10 table needs the
        // column added explicitly - same idempotent ALTER pattern as shot uuid.
        let has_last_edit_at_column: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('speleo_link') WHERE name='last_edit_at'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);
        if !has_last_edit_at_column {
            self.conn
                .execute("ALTER TABLE speleo_link ADD COLUMN last_edit_at TEXT", [])?;
            // The backfill also runs from the `current < 10` arm of
            // `ensure_schema_version`, so it no longer depends on this column add.
            Self::backfill_legacy_dirty_last_edit_at(&self.conn)?;
            tracing::info!("Added last_edit_at column to speleo_link table (schema v10)");
        }

        // Add last_verify_at column to an EXISTING v10 speleo_link (schema v10→v11:
        // lazy 20-min lock re-verify, #202). Same idempotent ALTER pattern as
        // last_edit_at above; CREATE IF NOT EXISTS is a no-op on a pre-v11 table.
        // No backfill: a NULL last_verify_at means "never verified since pull", which
        // simply forces the first lock-held edit to re-verify - safe, since #202 only
        // bounds wasted work (the release fence is the correctness backstop).
        let has_last_verify_at_column: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('speleo_link') WHERE name='last_verify_at'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);
        if !has_last_verify_at_column {
            self.conn
                .execute("ALTER TABLE speleo_link ADD COLUMN last_verify_at TEXT", [])?;
            tracing::info!("Added last_verify_at column to speleo_link table (schema v11)");
        }

        // Add undo_mode column to an EXISTING pre-v16 speleo_link (schema v15→v16: the
        // per-project undo-history policy, #303). Same idempotent ALTER pattern as the
        // two above. No SQL backfill: the application materializes NULL from its configured
        // default on the first open, when that machine-specific value is available.
        let has_undo_mode_column: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('speleo_link') WHERE name='undo_mode'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);
        if !has_undo_mode_column {
            self.conn
                .execute("ALTER TABLE speleo_link ADD COLUMN undo_mode TEXT", [])?;
            tracing::info!("Added undo_mode column to speleo_link table (schema v16)");
        }

        // Ensure the speleo_undo_chain table exists (schema v16: the Unwind chain
        // pointer, #303). CREATE IF NOT EXISTS - the speleo_link pattern, not the ALTER
        // pattern - so a legacy file gains it on open_existing. Local-only, so nothing
        // to backfill and nothing to carry forward: an absent row simply means "not
        // mid-unwind".
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS speleo_undo_chain (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                unwound_from_sha TEXT NOT NULL
            )",
            [],
        )?;

        // [#303] Its sibling, on the same terms. Created HERE rather than lazily on first
        // write (which is what `save_speleo_undo_watermark` used to be the only creator of)
        // so that "the table is absent" and "there is no watermark" stop being the same
        // observable state - the push has to tell them apart to decide whether it may keep
        // undo history - and so a maintainer tracing the migration finds it where its
        // sibling is rather than inside a setter.
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS speleo_undo_watermark (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                inherited_through TEXT NOT NULL
            )",
            [],
        )?;

        // Add uuid column to shots table (schema v5→v6: per-shot UUIDs,
        // the stable station identity every cross-reference anchors to
        // (`StationKey::AfterShot`) and the `<UUID>` the Ariane export
        // emits).  We don't backfill
        // existing rows here - `load_project` lazily mints a UUID for
        // any row that comes back with NULL, and the next save persists
        // it.  That keeps the migration cost a single ALTER TABLE for
        // even very large pre-v6 databases.
        let has_shot_uuid_column: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('shots') WHERE name='uuid'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);
        if !has_shot_uuid_column {
            self.conn
                .execute("ALTER TABLE shots ADD COLUMN uuid TEXT", [])?;
            tracing::info!("Added uuid column to shots table (schema v6)");
        }

        // Add shot_uuid column to resolved_warnings (schema v7→v8: key "Ignore"
        // suppression by the shot's stable UUID, issue #57). Pre-v8 rows keep
        // shot_uuid NULL and continue to match by the legacy
        // (section_id, shot_id) key; they are backfilled lazily on the next
        // resolve of the same shot. A single idempotent ALTER, like the shots
        // uuid column above.
        let has_resolved_warnings_uuid_column: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('resolved_warnings') WHERE name='shot_uuid'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);
        if !has_resolved_warnings_uuid_column {
            self.conn.execute(
                "ALTER TABLE resolved_warnings ADD COLUMN shot_uuid TEXT",
                [],
            )?;
            // Backfill the UUID for existing rows so suppressions survive the
            // migration. The legacy key stored `shot_id`, which on load is
            // `section_id * FEATURE_ID_STRIDE + sequence_order`, so we can
            // recover each shot's row and copy its UUID. Rows whose shot has no
            // UUID yet (pre-v6 .stalx, before the lazy in-memory mint) stay NULL
            // and fall back to legacy-position matching. The stride is
            // interpolated from the constant so this stays correct if it changes.
            self.conn.execute(
                &format!(
                    "UPDATE resolved_warnings \
                     SET shot_uuid = ( \
                         SELECT s.uuid FROM shots s \
                         WHERE s.section_id = resolved_warnings.section_id \
                           AND s.sequence_order = resolved_warnings.shot_id - resolved_warnings.section_id * {FEATURE_ID_STRIDE} \
                     ) \
                     WHERE shot_uuid IS NULL"
                ),
                [],
            )?;
            tracing::info!("Added shot_uuid column to resolved_warnings table (schema v8)");
        }

        // [#576 RV3-R-4] A file an earlier build upgraded from v2 lost this index
        // in the v2 -> v3 undo-table rebuild; restore it on its next upgrade.
        Self::ensure_undo_sequence_index(&self.conn)?;

        tracing::debug!("Schema migration complete");
        Ok(())
    }

    /// Stamp the Unix epoch into `last_edit_at` of a speleo_link row dirtied
    /// before v10 (schema v9 to v10).
    ///
    /// (#197 review) A row already dirtied under v9 has has_unpushed_edits=1 but
    /// no last_edit_at. Without a timestamp, edits_captured_by_push(None,
    /// push_start) is false, so a confirmed push could NEVER clear that legacy
    /// dirty bit - the project would show un-pushed edits forever and re-push a
    /// no-op on every Done. The edit demonstrably predates any future push, so
    /// stamp the Unix epoch (guaranteed <= any push-start) and let the next
    /// confirmed push clear it. Fresh v10 rows stamp last_edit_at at edit time.
    ///
    /// [#576 B6] Called from BOTH the column add in `migrate_schema` and the
    /// version-gated `current < 10` arm. It used to run only when the column was
    /// absent, and the ALTER and this UPDATE committed separately, so a crash
    /// between them left the column present, the backfill unrun and the stamp
    /// still 9 - and no later open ever ran it. A dirty row with no timestamp in
    /// a file below v10 can only be a v9 edit, so re-running it there is exact,
    /// and idempotent (`last_edit_at IS NULL`).
    fn backfill_legacy_dirty_last_edit_at(conn: &Connection) -> Result<()> {
        conn.execute(
            "UPDATE speleo_link SET last_edit_at = '1970-01-01T00:00:00+00:00' \
             WHERE has_unpushed_edits = 1 AND last_edit_at IS NULL",
            [],
        )?;
        Ok(())
    }

    /// Rename `is_virtual` column to `is_reference` in shots table, and
    /// rename the `showVirtualShots` display-settings key to `showReferenceShots`
    /// (schema v3→v4).
    fn migrate_is_virtual_to_is_reference(conn: &Connection) -> Result<()> {
        tracing::info!("Renaming is_virtual → is_reference in shots table (v3→v4)");
        // Only rename the column if it still has the old name (earlier migrations
        // may have already created the table with `is_reference` directly).
        let needs_rename = conn.prepare("SELECT is_virtual FROM shots LIMIT 0").is_ok();
        if needs_rename {
            conn.execute_batch("ALTER TABLE shots RENAME COLUMN is_virtual TO is_reference;")?;
        }
        // Always update display_settings key (idempotent UPDATE).
        conn.execute_batch(
            "UPDATE display_settings SET key = 'showReferenceShots' WHERE key = 'showVirtualShots';"
        )?;
        Ok(())
    }

    /// Replace the boolean `is_reference` column with a 3-way `kind`
    /// (schema v12→v13, #27).
    ///
    /// Every legacy `is_reference = 1` row becomes [`ShotKind::Jump`] (issue #27
    /// q2: most user-toggled reference shots are swum shortcuts in practice).
    /// That is only a seed: `pending_kind_classification` is set so the next
    /// project load runs the chain classifier once and demotes the runs that are
    /// not actually bounded, landing existing files where a fresh import would.
    /// The classifier needs sections, merges and closures assembled into a
    /// `Project`, which is not available on this bare connection, hence the flag
    /// rather than doing it inline.
    ///
    /// Idempotent: the column rewrite is guarded on the old column still
    /// existing, but the pending flag is set unconditionally.
    ///
    /// The two halves are guarded separately on purpose: a file can reach this
    /// arm with `kind` already in place and no `is_reference` left to convert
    /// (an unstamped file whose row claims an older version than its tables
    /// have), yet its rows still hold nothing but q2's Jump seed and still need
    /// the chain classifier. Guarding the flag on the column too (which is what
    /// the first cut did) silently skipped the backfill for exactly those files,
    /// leaving their reference ties counted as swum jumps forever.
    fn migrate_is_reference_to_kind(conn: &Connection) -> Result<()> {
        let needs_column_rewrite = conn
            .prepare("SELECT is_reference FROM shots LIMIT 0")
            .is_ok();
        if needs_column_rewrite {
            tracing::info!("Converting shots.is_reference → shots.kind (v12→v13, #27)");

            let has_kind = conn.prepare("SELECT kind FROM shots LIMIT 0").is_ok();
            if !has_kind {
                conn.execute_batch(
                    "ALTER TABLE shots ADD COLUMN kind INTEGER NOT NULL DEFAULT 0;",
                )?;
            }
            // 1 == ShotKind::Jump. Kept as a literal because this migration must
            // stay pinned to the v13 encoding even if the enum grows later.
            //
            // `!= 0`, not `= 1`: the pre-#27 loader read this column as
            // `row.get::<_, i32>()? != 0`, so ANY truthy value meant virtual. A
            // hand-edited or otherwise noncanonical `-1` / `2` would keep the
            // new column's `Normal` default and then be dropped along with
            // `is_reference` - permanently turning a reference tie into a
            // swimmable leg, with the backfill unable to help because the shot
            // no longer looks virtual to it.
            conn.execute_batch(
                r"
                UPDATE shots SET kind = 1 WHERE is_reference != 0;
                ALTER TABLE shots DROP COLUMN is_reference;
                ",
            )?;
        }
        // Unconditional: reaching this function at all means the file predates
        // v13, so its kinds are a seed and the next load owes it one chain pass.
        conn.execute_batch(
            r"
            INSERT OR REPLACE INTO project_settings (key, value)
                VALUES ('pending_kind_classification', '1');
            ",
        )?;

        // Materialise `showJumps` now, from whatever single toggle this file
        // used, so the two split toggles are independent from the first open.
        //
        // The load-time fallback (`showJumps` -> `showReferenceShots` ->
        // `showVirtualShots`) gets the FIRST open right, but it is a read-time
        // default, not a stored value: `save_display_setting` writes only the key
        // it was given, so a user who then changes References alone still has no
        // `showJumps` row, and the next open reads their NEW reference value as
        // the jump default - silently moving a toggle they never touched.
        // Writing the row here closes that window. `INSERT ... WHERE NOT EXISTS`
        // keeps it idempotent and never overwrites a real choice.
        conn.execute_batch(
            r"
            INSERT INTO display_settings (key, value)
            SELECT 'showJumps', COALESCE(
                (SELECT value FROM display_settings WHERE key = 'showReferenceShots'),
                (SELECT value FROM display_settings WHERE key = 'showVirtualShots'),
                'true')
            WHERE NOT EXISTS (SELECT 1 FROM display_settings WHERE key = 'showJumps');
            ",
        )?;
        Ok(())
    }

    /// Normalise stored pitch to the +down convention (schema v11→v12, #296).
    ///
    /// DMP import used to keep the mNemo's +up reading, so shots from a DMP
    /// were stored with a pitch that contradicted their own depth change while
    /// Walls- and Compass-sourced shots were already +down. Nothing on a shot
    /// or a section records which importer produced it, so the origin cannot be
    /// looked up, it has to be inferred from the numbers.
    ///
    /// Decided per SECTION, never per shot. A single shot is often ambiguous:
    /// near-horizontal and very short shots carry almost no signal (1 cm depth
    /// resolution swamps the implied angle), and on real data individual shots
    /// do score better in the wrong direction. A section is decisive because
    /// the convention is a property of the file it was imported from, uniform
    /// across it. Deciding per section also means a genuinely blundered shot
    /// cannot flip itself into agreement with its own depth change: it inherits
    /// the section verdict and stays flagged by validation, which is the whole
    /// point of the cross-check.
    ///
    /// Each usable shot casts ONE vote, and the votes are counted; the angular
    /// errors are deliberately not summed. Summing let a single shot decide the
    /// section by sheer magnitude: three correct shallow shots disagreeing by
    /// 4 degrees each are outweighed by one blunder recorded at -89 against
    /// +89 implied, which flips all four and launders the blunder, the exact
    /// opposite of the paragraph above. Counting votes bounds any one shot's
    /// influence at one vote.
    ///
    /// A shot votes only when its own evidence is lopsided (one direction's
    /// error beats the other by `DECISIVE_RATIO`); otherwise it abstains, which
    /// is what keeps near-horizontal and very short shots from voting noise. A
    /// section flips when at least `MIN_VOTING_SHOTS` shots vote to flip and
    /// the flip votes outnumber the keep votes by `DECISIVE_RATIO`. That margin
    /// is what lets a real DMP section carry one genuine blunder without being
    /// held hostage by it, while a section that is mostly correct can never be
    /// flipped by an outlier. Sections whose pitch is entirely zero (Ariane
    /// never populates inclination) produce no votes at all and are untouched.
    /// Returns the ids of the sections that were flipped, so the caller can
    /// invalidate the state that still holds their pre-flip pitch (persisted
    /// undo history, dismissed inclination warnings). Empty means nothing moved.
    pub(super) fn migrate_pitch_to_down_convention(conn: &Connection) -> Result<Vec<i64>> {
        /// Below this a shot's implied angle is mostly depth-gauge quantisation.
        const MIN_LENGTH_M: f64 = 0.05;
        /// Same, for the vertical change.
        const MIN_DZ_M: f64 = 0.05;
        /// Fewer usable shots than this is not a section-wide verdict.
        const MIN_VOTING_SHOTS: usize = 3;
        /// Flipped must beat as-is by this factor to count as decisive.
        const DECISIVE_RATIO: f64 = 2.0;
        /// A leg steeper than vertical is a blunder and casts no vote.
        const MAX_VOTING_PITCH_DEG: f64 = 90.0;

        let mut stmt = conn.prepare(
            "SELECT section_id, length, depth_in, depth_out, pitch_in, pitch_out
             FROM shots
             WHERE pitch_in IS NOT NULL OR pitch_out IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<f64>>(5)?,
            ))
        })?;

        // section_id -> (votes to flip, votes to keep). Shots whose own
        // evidence is not lopsided abstain and appear in neither count.
        let mut tally: HashMap<i64, (usize, usize)> = HashMap::new();
        for row in rows {
            let (section_id, length, depth_in, depth_out, pitch_in, pitch_out) = row?;
            // Reject out-of-range legs BEFORE averaging, exactly as
            // `Validator::validate_shot` does. Testing only the average would
            // let a shot with one impossible leg vote whenever the other leg
            // pulls the mean back inside the range.
            let leg_in_range =
                |leg: Option<f64>| leg.is_none_or(|v| v.abs() <= MAX_VOTING_PITCH_DEG);
            if !leg_in_range(pitch_in) || !leg_in_range(pitch_out) {
                continue;
            }
            let recorded = match (pitch_in, pitch_out) {
                (Some(a), Some(b)) => (a + b) / 2.0,
                (Some(a), None) | (None, Some(a)) => a,
                (None, None) => continue,
            };
            let dz = depth_out - depth_in;
            // Skip shots the geometry cannot speak to, and impossible ones.
            if length <= MIN_LENGTH_M || dz.abs() <= MIN_DZ_M || length < dz.abs() {
                continue;
            }
            if !recorded.is_finite() || !dz.is_finite() || !length.is_finite() {
                continue;
            }
            let horizontal = (length * length - dz * dz).max(0.0).sqrt();
            let implied = dz.atan2(horizontal).to_degrees();

            let as_is_error = (recorded - implied).abs();
            let flipped_error = (-recorded - implied).abs();

            let entry = tally.entry(section_id).or_insert((0, 0));
            if flipped_error * DECISIVE_RATIO < as_is_error {
                entry.0 += 1;
            } else if as_is_error * DECISIVE_RATIO < flipped_error {
                entry.1 += 1;
            }
        }
        drop(stmt);

        let flip: Vec<i64> = tally
            .iter()
            .filter(|(_, (flip_votes, keep_votes))| {
                *flip_votes >= MIN_VOTING_SHOTS
                    && (*flip_votes as f64) > (*keep_votes as f64) * DECISIVE_RATIO
            })
            .map(|(section_id, _)| *section_id)
            .collect();

        // Sections that had usable geometry but were not decisive enough to
        // flip keep whatever convention they were stored in, and the v12 stamp
        // means they are never re-examined.  If one of those was in fact a +up
        // section, validation will now flag every sloped shot in it, so name
        // them in the log rather than leaving the user to guess.
        let declined: Vec<i64> = tally
            .iter()
            .filter(|(section_id, (flip_votes, keep_votes))| {
                flip_votes + keep_votes >= MIN_VOTING_SHOTS && !flip.contains(section_id)
            })
            .map(|(section_id, _)| *section_id)
            .collect();
        if !declined.is_empty() {
            tracing::warn!(
                declined_sections = ?declined,
                "Pitch convention was not decisive for these sections; leaving them as stored (v11 to v12)"
            );
        }

        if flip.is_empty() {
            tracing::info!(
                sections_examined = tally.len(),
                "No sections needed pitch-convention normalisation (v11 to v12)"
            );
            return Ok(flip);
        }

        // No transaction here on purpose: the caller wraps this call and the
        // invalidation that follows it in one, so the two cannot be separated
        // by a crash.
        for section_id in &flip {
            conn.execute(
                "UPDATE shots
                 SET pitch_in  = CASE WHEN pitch_in  IS NULL THEN NULL ELSE -pitch_in  END,
                     pitch_out = CASE WHEN pitch_out IS NULL THEN NULL ELSE -pitch_out END
                 WHERE section_id = ?1",
                [section_id],
            )?;
        }

        tracing::info!(
            sections_flipped = flip.len(),
            sections_examined = tally.len(),
            "Normalised stored pitch to the +down convention (v11 to v12)"
        );
        Ok(flip)
    }

    /// Migrate undo/redo tables from TEXT to BLOB columns (schema v2→v3).
    /// Existing TEXT data is CAST to BLOB; on read, `decompress_json_under` detects
    /// whether content is gzip-compressed or plain UTF-8, so old entries remain
    /// readable without re-encoding.
    ///
    /// [#576 B6] No transaction and no `PRAGMA foreign_keys` toggle of its own any
    /// more: it runs inside the one transaction `ensure_schema_version` wraps the
    /// whole chain in (where the pragma would be a no-op anyway), and nothing has a
    /// foreign key into the undo tables, so dropping and renaming them is safe with
    /// enforcement on.
    pub(super) fn migrate_undo_redo_to_blob(&self) -> Result<()> {
        tracing::info!("Migrating undo/redo tables to BLOB columns (v2→v3)");

        let tx = &self.conn;
        for table in &["undo_stack", "redo_stack"] {
            let has_table: bool = tx
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    params![table],
                    |row| row.get::<_, i64>(0),
                )
                .map(|c| c > 0)
                .unwrap_or(false);

            if !has_table {
                continue;
            }

            // Check if command_json is already BLOB (idempotency)
            let col_type: String = tx
                .query_row(
                    &format!(
                        "SELECT type FROM pragma_table_info('{table}') WHERE name='command_json'"
                    ),
                    [],
                    |row| row.get(0),
                )
                .unwrap_or_default();

            if col_type.eq_ignore_ascii_case("BLOB") {
                tracing::debug!("{table} already has BLOB columns, skipping");
                continue;
            }

            let new_table = format!("{table}_new");
            tx.execute_batch(&format!(
                r"
                CREATE TABLE {new_table} (
                    id INTEGER PRIMARY KEY,
                    sequence INTEGER NOT NULL,
                    command_json BLOB NOT NULL,
                    undo_data_json BLOB NOT NULL,
                    executed_at TEXT NOT NULL
                );
                INSERT INTO {new_table} SELECT id, sequence, CAST(command_json AS BLOB), CAST(undo_data_json AS BLOB), executed_at FROM {table};
                DROP TABLE {table};
                ALTER TABLE {new_table} RENAME TO {table};
                "
            ))?;
            tracing::info!("Migrated {table} to BLOB columns");
        }
        // [#576 RV3-R-4] `DROP TABLE undo_stack` took `idx_undo_sequence` with it,
        // and `open_existing` never runs `init_schema`, so every file upgraded
        // from v2 lacked the index a fresh file has. Put it back.
        Self::ensure_undo_sequence_index(tx)?;

        Ok(())
    }

    /// `init_schema`'s `idx_undo_sequence`, for a file whose `undo_stack` lacks
    /// it (#576 RV3-R-4). A no-op without an `undo_stack` table.
    fn ensure_undo_sequence_index(conn: &Connection) -> Result<()> {
        let has_undo_stack: bool = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='undo_stack'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)?;
        if has_undo_stack {
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_undo_sequence ON undo_stack(sequence)",
                [],
            )?;
        }
        Ok(())
    }

    /// Normalise stored azimuth readings into `[0, 360)` (schema v13 to v14).
    ///
    /// Nothing derived is invalidated (see "What is deliberately NOT invalidated"), so
    /// the sections holding a shot whose walked BEARING actually moved are only logged:
    /// that warning is the sole record that those shots used to render 180 degrees off.
    ///
    /// ## What changed, and why this is not a v11-to-v12-shaped flip
    ///
    /// `Shot::average_azimuth` is the circular mean of the two forward readings. It used
    /// Rust's `%`, which is *truncated* and keeps the sign of the dividend, so for a
    /// reading pair more than 540 degrees apart in the negative direction
    /// (`azimuth_out - azimuth_in < -540`, bar the exact `-360k` offsets where both wraps
    /// agree) the intermediate `diff` stayed outside `(-180, 180]` and the mean came back
    /// as the ANTIPODE of the true bearing. It now wraps with `rem_euclid` and returns the
    /// true mean.
    ///
    /// Unlike the pitch flip, NO stored value changed meaning: an azimuth is still degrees
    /// clockwise from north, and the old rendering was simply wrong. So this migration must
    /// NOT rewrite readings to preserve the old geometry - that would bake the 180-degree
    /// error in permanently. What it does instead is remove the ambiguity from the file: a
    /// reading in `[0, 360)` cannot reach the truncation bug, in this build or an older one,
    /// so the same `.stalx` now renders identically either side of the change.
    ///
    /// ## Reachability
    ///
    /// Both readings in `[0, 360)` put their difference in `(-360, 360)`, so an affected
    /// shot always has at least one out-of-range reading. Every importer's shots now pass
    /// the core import gate (`execute_import_sections`), which normalises them, so this is
    /// a one-off cleanup of what earlier builds admitted: DMP legs (raw `i16 * 0.1`,
    /// `-3276.8..3276.7`) and hand-entered values, which `Validator::validate_shot` only
    /// WARNS about.
    ///
    /// ## What is deliberately NOT invalidated
    ///
    /// * **Persisted undo/redo.** The v11-to-v12 flip had to drop it because replaying a
    ///   pre-flip payload wrote a geometrically WRONG pitch into a migrated file. Here a
    ///   payload's raw azimuth means what it always meant and walks to the same place under
    ///   `rem_euclid`, so replaying one is geometrically exact - it can only reintroduce an
    ///   unnormalised READING, which the next open of a v14 file no longer re-examines but
    ///   which is still rendered correctly. Losing the history would cost more than it buys.
    /// * **Warning dismissals.** `INVALID_AZIMUTH` simply stops firing once the reading is
    ///   in range, so its rows go inert rather than suppressing something live. That holds
    ///   for every row because the wrap is [`crate::models::normalize_azimuth`], whose
    ///   output is `[0, 360)` by construction; a bare `rem_euclid` would have left the
    ///   claim false for a sub-ulp negative reading, which it maps to exactly `360.0` -
    ///   still `INVALID_AZIMUTH`, minted by the migration itself.
    ///   `AZIMUTH_DEVIATION` measures `(azimuth_out - azimuth_in).abs()` folded about 180:
    ///   for an out-of-range pair that fold produced a NEGATIVE "difference" and the warning
    ///   could never fire, so there is no dismissal to clear - and after normalisation it
    ///   starts firing correctly, which is the user-visible signal that these shots were
    ///   touched.
    /// * **`section_offsets`.** These are the one derived value that a corrected bearing
    ///   really can leave inconsistent, and they are still left alone - deliberately, not by
    ///   omission. An offset is an ACCUMULATED translation: `adjust_section_offset` adds or
    ///   subtracts a shot's displacement each time a shot is inserted at or removed from a
    ///   section's head, so what is stored is the running sum of past edits, not a function of
    ///   the current shots. A compensation banked under the antipodal bearing cannot be
    ///   recomputed from the file - only replaying the edit history would do it, and that
    ///   history is not kept.
    ///
    ///   The affected population is the intersection of two already-rare conditions: the
    ///   section must have held an out-of-range azimuth PAIR (see Reachability) *and* have had
    ///   a shot inserted or removed at its head. For such a section the migration still fixes
    ///   what matters - the SHAPE, i.e. the direction its shots walk - while its absolute
    ///   placement can be out by up to twice the affected shot's horizontal vector. That is a
    ///   visible displacement the user can correct by moving the section, not a silent
    ///   corruption of measured values.
    ///
    ///   The alternatives are worse. Zeroing offsets would move every section that has one,
    ///   including the overwhelming majority whose azimuths were always in range and whose
    ///   placement is correct today; recomputing them is impossible for the reason above. So
    ///   the rule is: correct the readings, leave the accumulated translation, and accept a
    ///   recoverable misplacement in a rare intersection rather than a certain one everywhere.
    ///   Pinned by `migration_v14_leaves_section_offsets_untouched`.
    pub(super) fn migrate_azimuths_to_canonical_range(conn: &Connection) -> Result<()> {
        let mut stmt = conn.prepare(
            "SELECT id, section_id, azimuth_in, azimuth_out
             FROM shots
             WHERE azimuth_in < 0.0 OR azimuth_in >= 360.0
                OR azimuth_out < 0.0 OR azimuth_out >= 360.0",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?;

        // (shot id, normalised in, normalised out) plus the sections whose bearing moved.
        let mut updates: Vec<(i64, f64, f64)> = Vec::new();
        let mut reinterpreted: Vec<i64> = Vec::new();
        for row in rows {
            let (id, section_id, azimuth_in, azimuth_out) = row?;
            // A non-finite reading (SQLite rejects NaN into a NOT NULL REAL, but stores
            // infinities) has no canonical form; `normalize_azimuth` passes it through, so
            // without this guard the row would take a no-op UPDATE and still inflate the
            // count. Leave it for `ShotData::all_finite` and the integrity banner.
            if !azimuth_in.is_finite() || !azimuth_out.is_finite() {
                continue;
            }
            if Self::old_wrap_returned_the_antipode(azimuth_in, azimuth_out) {
                reinterpreted.push(section_id);
            }
            updates.push((
                id,
                crate::models::normalize_azimuth(azimuth_in),
                crate::models::normalize_azimuth(azimuth_out),
            ));
        }
        drop(stmt);

        if updates.is_empty() {
            return Ok(());
        }
        // No transaction here on purpose: the caller opens one so these per-row UPDATEs
        // commit as a batch instead of N autocommits.
        for (id, azimuth_in, azimuth_out) in &updates {
            conn.execute(
                "UPDATE shots SET azimuth_in = ?2, azimuth_out = ?3 WHERE id = ?1",
                params![id, azimuth_in, azimuth_out],
            )?;
        }
        reinterpreted.sort_unstable();
        reinterpreted.dedup();
        tracing::info!(
            shots_normalised = updates.len(),
            sections_reinterpreted = reinterpreted.len(),
            "Normalised stored azimuth readings into [0, 360) (v13 to v14)"
        );
        if !reinterpreted.is_empty() {
            tracing::warn!(
                sections = ?reinterpreted,
                "These sections held a shot whose two azimuth readings were more than 540 \
                 degrees apart; earlier builds walked it 180 degrees off, and it now walks \
                 to the bearing its readings actually describe"
            );
        }
        Ok(())
    }

    /// Whether the pre-#24 truncated wrap walked this reading pair 180 degrees off.
    ///
    /// `average_azimuth_of` wraps `azimuth_out - azimuth_in + 540.0`. Rust's `%` is
    /// truncated - it keeps the dividend's sign - so it differs from `rem_euclid` by
    /// exactly 360 when that expression is negative AND not an exact multiple of 360,
    /// and that halves into the 180-degree error. The naive
    /// `azimuth_out - azimuth_in < -540.0` test drops the second clause, so it
    /// false-positives on every exact `-360k` offset: a pair with
    /// `azimuth_out - azimuth_in == -900.0` walks to the same bearing under either
    /// wrap and was never misread.
    ///
    /// Only feeds [`Self::migrate_azimuths_to_canonical_range`]'s warning - nothing is
    /// invalidated on the strength of it.
    pub(super) fn old_wrap_returned_the_antipode(azimuth_in: f64, azimuth_out: f64) -> bool {
        let wrap_input = azimuth_out - azimuth_in + 540.0;
        wrap_input < 0.0 && wrap_input.rem_euclid(360.0) != 0.0
    }

    /// Drop the persisted undo/redo stacks. Called by every migration whose
    /// change makes older payloads unsafe to replay.
    ///
    /// Callers, and what each is protecting against:
    ///
    /// - **v6→v7** (the original): explicit-null `ShotUpdates` fields, detailed
    ///   below. Also runs for any file below `UNDO_FORMAT_SAFE_VERSION`.
    /// - **v11→v12**: entries holding a pre-#296 pitch, via
    ///   [`invalidate_pre_pitch_flip_state`](Self::invalidate_pre_pitch_flip_state).
    /// - **v12→v13**: entries holding a legacy `is_reference` boolean. A bare
    ///   `true` cannot say which virtual kind it meant, so `de_shot_kind` maps it
    ///   to `Jump`; replaying one after the #27 backfill settled that shot on
    ///   `Reference` would make a non-swimmable tie swimmable again, and no
    ///   classifier pass runs on an undo to correct it.
    /// - **v14→v15**: entries naming the removed `ApplyCollabChanges` command.
    /// - **v15→v16**: privacy - pre-v16 project snapshots carry `file_path`.
    /// - **v16→v17**: explicit-null `SectionUpdates` fields (#413 finding 5) -
    ///   the v6→v7 hazard below, one struct over.
    ///
    /// Pre-v7 builds serialized every untouched nullable `ShotUpdates` field as
    /// an explicit `"left": null, "pitch_in": null, ...` inside the persisted
    /// `command_json` / `undo_data_json` payloads. Under the v7 `double_option`
    /// semantics an explicit `null` means "clear the field", so replaying such
    /// an entry would silently wipe pitch/LRUD/comment/marker/temperature on
    /// the target shot - an upgrade-path data loss. The legacy JSON is
    /// ambiguous (null = "untouched" and "restore-to-empty" look identical),
    /// so the only safe option is to drop the persisted stacks: they are
    /// best-effort history (capped at ~20 entries), and losing them once at
    /// upgrade is acceptable.
    pub(super) fn drop_legacy_undo_history(&self) -> Result<()> {
        for table in ["undo_stack", "redo_stack"] {
            if !self.table_exists(table)? {
                continue;
            }
            let removed = self.conn.execute(&format!("DELETE FROM {table}"), [])?;
            if removed > 0 {
                tracing::info!(
                    table,
                    removed,
                    purpose = "prevent legacy payloads from being replayed unsafely",
                    "Dropped persisted undo history (legacy payloads are unsafe to replay)"
                );
            }
        }
        Ok(())
    }

    /// Discard everything outside the `shots` table that still holds a
    /// pre-#296 pitch, after `migrate_pitch_to_down_convention` flipped at
    /// least one section (schema v11 to v12).
    ///
    /// The migration rewrites `shots` only. Two other stores hold the pitch of
    /// the sections it moved, or a verdict derived from it, and neither is
    /// versioned, so a stale copy would outlive the migration and there is no
    /// second pass to catch it:
    ///
    /// 1. Persisted undo/redo. Payloads carry absolute pitch values and whole
    ///    project snapshots, so one undo of a pre-upgrade edit writes the +up
    ///    value straight back into a file already stamped v12. Whole-project
    ///    snapshots are why this is all-or-nothing rather than per section.
    /// 2. Dismissed inclination warnings. `resolved_warnings` is keyed by
    ///    `(section_id, shot_id, warning_code)` with no value component, so a
    ///    mismatch the user dismissed under the old convention would keep
    ///    suppressing the check now that the number means something else. Only
    ///    the flipped sections are cleared, and only the one warning code whose
    ///    meaning actually changed: `LENGTH_BELOW_DEPTH_CHANGE` compares length
    ///    against the depth change and never reads pitch, so its dismissals are
    ///    still valid.
    fn invalidate_pre_pitch_flip_state(&self, flipped_sections: &[i64]) -> Result<()> {
        self.drop_legacy_undo_history()?;

        if self.table_exists("resolved_warnings")? {
            let mut cleared = 0usize;
            for section_id in flipped_sections {
                cleared += self.conn.execute(
                    "DELETE FROM resolved_warnings
                     WHERE warning_code = 'INCLINATION_DEPTH_MISMATCH'
                       AND section_id = ?1",
                    [section_id],
                )?;
            }
            if cleared > 0 {
                tracing::info!(
                    cleared,
                    "Cleared inclination warning dismissals taken under the pre-#296 convention"
                );
            }
        }

        Ok(())
    }

    /// Enable foreign key enforcement on this connection.
    pub(super) fn enable_foreign_keys(&self) -> Result<()> {
        self.conn.execute("PRAGMA foreign_keys = ON;", [])?;
        Ok(())
    }

    /// Check if a table exists in the database.
    pub(super) fn table_exists(&self, name: &str) -> Result<bool> {
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            params![name],
            |row| row.get(0),
        )?;
        Ok(exists > 0)
    }

    /// Read schema_version from the project table, if present.
    fn get_project_schema_version(&self) -> Result<Option<i32>> {
        if !self.table_exists("project")? {
            return Ok(None);
        }

        let has_column: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('project') WHERE name='schema_version'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !has_column {
            return Ok(None);
        }

        let version: i32 =
            self.conn
                .query_row("SELECT schema_version FROM project LIMIT 1", [], |row| {
                    row.get(0)
                })?;
        Ok(Some(version))
    }

    /// Read the current schema version from PRAGMA user_version.
    pub(super) fn get_user_version(&self) -> Result<i32> {
        let version: i32 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        Ok(version)
    }

    /// Persist the schema version to PRAGMA user_version.
    pub(super) fn set_user_version(&self, version: i32) -> Result<()> {
        self.conn
            .execute(&format!("PRAGMA user_version = {version}"), [])?;
        Ok(())
    }

    /// [#576 B1-D1] Refuse a file from before the first public release, before
    /// anything writes to it - from `ensure_schema_version`, and from
    /// `Database::open` before its `init_schema` (#576 NF-PNEWER-1). Reads only.
    ///
    /// The first release (v0.0.3) already shipped schema 2 and stamped
    /// `user_version`, and no schema-1 build ever wrote the stamp, so a project
    /// row claiming below 2 with no stamp of 2 or more to contradict it is a
    /// pre-release test file or a hand-edited one. A file with NO project row is
    /// not refused: that is an empty file `Database::open` is about to make a
    /// project of (Android's document picker leaves a 0-byte placeholder at the
    /// chosen path), not an old one. Such files used to go through a frozen
    /// "rebuild with foreign keys" step that had fallen behind the schema (it
    /// dropped the #438 azimuth columns and the values of `shots.uuid`,
    /// `loop_closures.is_aligned` and `resolved_warnings.shot_uuid`, and left
    /// `station_comments` pointing at a dropped table) and then stamped the file
    /// current. docs/saving.md ("Schema version and pre-release files") states
    /// the policy: no migration path, re-import from the survey source.
    pub(super) fn refuse_pre_release_file(&self) -> Result<()> {
        let stamped = self.get_user_version().unwrap_or(0);
        // `Ok(None)` (no table or column) and an error (no row yet) alike mean
        // the row makes no claim.
        let row_version = self.get_project_schema_version().ok().flatten();
        if stamped < OLDEST_SUPPORTED_SCHEMA_VERSION
            && row_version.is_some_and(|row| row < OLDEST_SUPPORTED_SCHEMA_VERSION)
        {
            let current = if stamped == 0 {
                row_version.unwrap_or(stamped)
            } else {
                stamped
            };
            tracing::warn!(
                version = current,
                oldest_supported = OLDEST_SUPPORTED_SCHEMA_VERSION,
                purpose = "refuse a pre-release project file",
                "Database predates the oldest supported schema; refusing to open it"
            );
            return Err(PreReleaseFile { version: current }.into());
        }
        Ok(())
    }

    /// Ensure schema migrations are applied and user_version is current.
    pub(super) fn ensure_schema_version(&self) -> Result<()> {
        self.ensure_schema_version_with(None)
    }

    /// [`Self::ensure_schema_version`], refusing the upgrade with `refuse_upgrade`
    /// (an [`UpgradeRefused`] carrying the message that fits the file) when it would
    /// write - the migration chain and the pre-upgrade copy alike (#576
    /// RV4-R-M3-OPEN). A file it would not write to (current, newer, unstamped on
    /// read-only media) passes as before.
    pub(super) fn ensure_schema_version_with(
        &self,
        refuse_upgrade: Option<&UpgradeRefusal>,
    ) -> Result<()> {
        // Two different claims, and only one of them is evidence that migrations
        // RAN. `PRAGMA user_version` is written by this app in exactly two places:
        // at the end of a completed migration chain (below), and on a file it just
        // created at the current DDL. `project.schema_version` is an ordinary
        // column that `save_project` stamps with the writing build's constant, so
        // it says "which build last saved me", not "which ALTERs my tables have
        // seen" - and it travels with the file through every copy, push/pull and
        // hand edit. Keep them apart: the project row may only pick which
        // version-gated arms below apply, never certify that the chain is done.
        let stamped = self.get_user_version().unwrap_or(0);
        // `Ok(None)` (no table or column) and an error (no row yet) alike mean the
        // row makes no claim.
        let row_version = self.get_project_schema_version().ok().flatten();
        let mut current = stamped;
        if current == 0 {
            if let Some(project_version) = row_version {
                current = project_version;
            }
        }
        // Did the version we are about to act on come from the row rather than
        // the stamp? Everything below that must not be decided on an unverified
        // claim keys off this.
        let claimed_only = stamped == 0 && current > 0;

        // Fast path: already current.  migrate_schema() scans pragma
        // table_info() for every conditional column add on every open,
        // which previously cost ~10 ms per project load even when no
        // migration was needed.
        //
        // Gated on `stamped`, NOT `current` (#485). A file whose `user_version` is
        // 0 while its project row claims the current version used to take this
        // return, so `migrate_schema` never ran on it - on ANY connection, ever,
        // because nothing here stamps `user_version` either, and the next open
        // repeats the same skip. The v8 `resolved_warnings.shot_uuid` ALTER is one
        // of the columns that then stays missing, and the save path's
        // "carry resolved warnings across save" read fails with
        // `no such column: shot_uuid` - dropping every "Ignore" the user had
        // recorded.
        if stamped == SCHEMA_VERSION {
            return Ok(());
        }

        if stamped > SCHEMA_VERSION {
            tracing::warn!(
                current = stamped,
                expected = SCHEMA_VERSION,
                purpose = "open a project saved by a newer version read-only",
                "Database schema version is newer than this app; leaving the file untouched"
            );
            // [#576 B6-D1] A file a NEWER build wrote is opened without a single
            // write: no arm (there is nothing sensible this build could run) and,
            // unlike before, no undo purge either. The project is opened
            // read-only (`AppState::newer_file_schema`), so its history stays
            // the newer build's to replay, and the file is left byte-for-byte
            // as that build wrote it.
            //
            // The purge used to be the fail-safe against replaying payloads no
            // arm here has vetted - a file merely CLAIMING to be from the future
            // keeps exactly the pre-v17 entries the v17 arm drops, which is all it
            // takes to turn `PRAGMA user_version = 99` into a replay that wipes a
            // section's name, surveyor and date. That guarantee now lives in the
            // load path instead: it restores none of a newer file's history into
            // this build's undo stack (`project_load.rs`), and nothing here may
            // edit the project anyway.
            return Ok(());
        }

        // [#576 B1-D1] A file from before the first public release is REFUSED,
        // before anything here writes to it (`refuse_pre_release_file`).
        self.refuse_pre_release_file()?;

        // Everything past this point WRITES. An unstamped file whose row claims
        // exactly the current version used to take the fast path on that claim
        // alone, so it reached no write at all - and a `.stalx` on
        // write-protected media opened clean. `open_existing` asks for
        // SQLITE_OPEN_READ_WRITE, but the unix VFS silently falls back to
        // read-only rather than failing the open, so the refusal only arrives at
        // the first write: `ALTER TABLE` returns SQLITE_READONLY and `?`
        // propagates it out of `open_existing`, refusing the user their own
        // project. Degrade instead, exactly as before #485: warn, skip the
        // chain, open the file, and the moment the file is writable the chain
        // runs. A Save As off such a file no longer costs the ignores either
        // (#517): the save carries them with `load_resolved_warnings_verbatim`,
        // which reads the un-migrated table in the shape it actually has.
        //
        // `== SCHEMA_VERSION`, not `>=`. A row claiming the FUTURE does not
        // belong here: its branch below drops the undo history precisely because
        // no arm has sanitized those payloads, and bypassing that would let the
        // load path restore pre-v17 entries this function classifies as unsafe to
        // replay. Such a file reaches the DELETE and fails the open on read-only
        // media - which is exactly what master did with it (master derived
        // `current` from the row too, and hit the same DELETE), so nothing
        // regresses.
        //
        // Deliberately NOT extended to a file with a real stamp below
        // SCHEMA_VERSION either: that one already reached these writes and
        // already failed this way, and opening it unmigrated would be worse than
        // refusing it (a pre-v12 file read without the pitch flip shows inverted
        // depths).
        if claimed_only
            && current == SCHEMA_VERSION
            && self.conn.is_readonly(MAIN_DB).unwrap_or(false)
        {
            tracing::warn!(
                claimed = current,
                expected = SCHEMA_VERSION,
                purpose = "open a project on write-protected media",
                "Database carries no schema stamp and cannot be written; opening it without \
                 running migrations"
            );
            return Ok(());
        }

        // [#576 B6] ONE transaction for the whole chain and its stamp, so an
        // upgrade lands completely or not at all. Every step used to commit on
        // its own (autocommit, or an arm-local transaction) and the stamp after
        // them, so a crash or an arm failing part-way left a half-migrated file:
        // columns an earlier step had added, the stamp still old. `user_version`
        // is transactional in SQLite, so it commits with the steps. IMMEDIATE
        // takes the write lock up front: nothing else can write the file between
        // the copy below and the first migration write, and on media that cannot
        // be written this fails before any step runs, exactly as the first write
        // used to. Dropped on an error, which rolls everything back.
        //
        // [#576 RV4-R-M3-OPEN] The first write is the transaction itself: a file the
        // caller placed where nothing may be written is refused here, untouched,
        // with no pre-upgrade copy beside it.
        if let Some(refusal) = refuse_upgrade {
            // [#576 RV6b-R3] An unstamped file whose row claims this version or a
            // newer one is not an older file, and gets no copy below: say what the
            // open would really have done.
            let unstamped = claimed_only && current >= SCHEMA_VERSION;
            tracing::warn!(
                version = current,
                expected = SCHEMA_VERSION,
                unstamped,
                purpose =
                    "refuse to upgrade a project where nothing may be written (#576 RV4-R-M3)",
                "Not upgrading a project in place: it is in a folder the app does not write to"
            );
            let message = if unstamped {
                &refusal.unstamped
            } else {
                &refusal.older_format
            };
            return Err(UpgradeRefused {
                refusal: message.clone(),
            }
            .into());
        }
        let chain = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;

        // [#576 B6-D2] Keep one copy of the file as it was before this upgrade,
        // before the first change. Not for an unstamped file whose row claims the
        // current version or a newer one: nothing is stamped there (see the end
        // of this function), so it is not an upgrade.
        if !(claimed_only && current >= SCHEMA_VERSION) {
            self.keep_pre_upgrade_copy(current)?;
        }

        if current > SCHEMA_VERSION {
            // Only reachable with `claimed_only`: a real stamp above
            // SCHEMA_VERSION returned above. The row's word is enough to skip
            // every version-gated arm (they are all `current < N`, so none run)
            // and to drop the history those arms exist to sanitize - but NOT to
            // skip `migrate_schema`, which is idempotent and adds only columns
            // and tables this build needs to read the file at all. Returning
            // here instead is how the #485 shape survived one branch earlier:
            // permanently, since nothing stamps afterwards either.
            tracing::warn!(
                claimed = current,
                expected = SCHEMA_VERSION,
                "Unstamped database whose project row claims a version newer than this app"
            );
            self.drop_legacy_undo_history()?;
        }

        // `migrate_schema` is idempotent - every step is a CREATE IF NOT EXISTS
        // or a pragma_table_info-guarded ALTER - so it is safe to reach on every
        // open of a file we decline to stamp below.
        self.migrate_schema()?;
        if current < 10 {
            Self::backfill_legacy_dirty_last_edit_at(&self.conn)?;
        }
        if current < 3 {
            self.migrate_undo_redo_to_blob()?;
        }
        if current < 4 {
            Self::migrate_is_virtual_to_is_reference(&self.conn)?;
        }
        if current < UNDO_FORMAT_SAFE_VERSION {
            self.drop_legacy_undo_history()?;
        }
        if current < 12 {
            // The flip and the invalidation have to land together. The verdict
            // is re-derived from the rows themselves, so a flip that committed
            // alone would look already-correct on the next open: the migration
            // would decline, the invalidation would never run, and v12 would be
            // stamped over permanently stale undo history and warning
            // dismissals. The chain's one transaction removes that window: the
            // invalidation runs on this same connection, so its statements join
            // it, and they commit with the flip and the stamp or not at all.
            let flipped = Self::migrate_pitch_to_down_convention(&self.conn)?;
            if !flipped.is_empty() {
                self.invalidate_pre_pitch_flip_state(&flipped)?;
            }
        }
        if current < 13 {
            Self::migrate_is_reference_to_kind(&self.conn)?;
            // Drop persisted undo/redo, for the same reason v11->v12 does after a
            // pitch flip. `UNDO_FORMAT_SAFE_VERSION` is 7, so a v12 file keeps
            // its history, and those payloads still carry the legacy
            // `is_reference` boolean - which `de_shot_kind` maps to `Jump`
            // unconditionally, since a bare `true` cannot say which virtual kind
            // it meant. Replaying one after the backfill has settled a tie on
            // `Reference` would restore it as a Jump: swimmable again, and back
            // in gas, penetration and the ruler, with no classifier pass to
            // correct it. The history is not worth an unsafe shortcut.
            self.drop_legacy_undo_history()?;
        }
        if current < 14 {
            // The callee issues one `execute` per affected shot and documents that
            // the caller wraps them: the chain's transaction batches them.
            Self::migrate_azimuths_to_canonical_range(&self.conn)?;
        }
        if current < 15 {
            // Ariane collaboration mode is gone: drop its single-row sidecar
            // table outright rather than leaving a dead one in every existing
            // file. Destructive but inert - nothing reads it after this
            // release, and the `.stalx` it described (the sibling `.tml`) is no
            // longer written either.
            self.conn.execute("DROP TABLE IF EXISTS collab_state", [])?;
            // The persisted undo history has to go with it, for the same reason
            // v11->v12 and v12->v13 drop theirs. `Command` is an internally
            // tagged enum with no catch-all, so a stored
            // `{"type":"apply_collab_changes"}` entry no longer deserializes -
            // and `load_from_persisted_with_timestamps` skips a bad entry
            // WITHOUT truncating, keeping the entries on both sides. That
            // leaves a HOLE: the surviving older entries (position-keyed
            // `ShotSnapshot`s, and the `ProjectSnapshotPair` the apply itself
            // wrote) would replay against a tree that still carries the applied
            // changes, silently reverting them or shifting every station past
            // the apply's inserts. `UNDO_FORMAT_SAFE_VERSION` is 7, so a v14
            // file reaches here with its stacks intact and would hit exactly
            // that. The history is not worth the risk.
            self.drop_legacy_undo_history()?;
        }
        if current < 16 {
            // [#303] Same drop again, for a different reason: PRIVACY, not replay safety.
            //
            // Until v16 an undo `ProjectSnapshot` / `ProjectSnapshotPair` was
            // `serde_json::to_string(project)` over the WHOLE `Project`, including
            // `file_path` - an absolute path on the authoring machine
            // (`/home/<user>/…`, `C:\Users\<Real Name>\…`). That was harmless while the
            // push stripped `undo_stack` unconditionally; v16 makes Full mode ship it, so
            // those rows would ride into the project's git history, permanently, readable
            // by every collaborator and by whoever runs the server.
            //
            // `AppState::serialize_project_snapshot` clears the field from here on, but it
            // is producer-side: it cannot reach rows already on disk. Nor does the
            // clear-on-lock path, which deliberately skips a copy that is already dirty
            // ("the user's own history survives") - i.e. exactly the upgrade-with-unpushed-
            // work case. Dropping the pre-v16 stacks at the migration is the only place
            // that catches every existing file before its first push.
            //
            // The cost is the same one v13 and v15 already charged: a one-time loss of
            // local undo history at upgrade, for history that was capped and best-effort
            // anyway.
            self.drop_legacy_undo_history()?;
        }
        if current < 17 {
            // [#413 finding 5] The v7 hazard again, one struct over.
            //
            // `SectionUpdates`' `name` / `surveyor` / `date` gained the
            // `double_option` + `skip_serializing_if` pair `ShotUpdates` has had
            // since v7. Pre-v17 writers serialized every untouched one as an
            // explicit `null` into both `command_json` (the
            // `update_section_metadata` command carries a `SectionUpdates`) and
            // `undo_data_json` (`PreviousValues { shot_id: None }`'s
            // `values_json`). Those payloads still PARSE - only their meaning
            // moved, from "leave this field alone" to "clear this field" - so
            // replaying one would wipe the name, surveyor and date off the
            // section it names. `UNDO_FORMAT_SAFE_VERSION` is 7 and stays there
            // (it is the floor for the pre-v7 purge alone; bumping it would make
            // the v13 and v15 arms' own tests pass without those arms), so a v16
            // file arrives here with its stacks intact and needs this drop.
            //
            // The legacy JSON is ambiguous in exactly the way v7's was - null =
            // "untouched" and null = "restore-to-empty" are the same six
            // characters - so there is nothing to rewrite and the only safe
            // option is the one v7, v13, v15 and v16 all took.
            self.drop_legacy_undo_history()?;
        }

        // Stamp, with ONE exception: a row-derived version at or above
        // SCHEMA_VERSION. That is the only shape where the row's word caused
        // every version-gated arm to be skipped, so the stamp - which claims all
        // of them have been applied - would certify work that never ran. A
        // genuinely pre-v12 file whose row lies about being current would be
        // sealed as current, the pitch flip would never run, and the
        // `user_version == 0` evidence that something is off would be destroyed.
        // It re-runs the idempotent `migrate_schema` on each open instead, about
        // 10 ms, for a file shape nothing this app writes.
        //
        // Every OTHER row-derived version is stamped, exactly as master did.
        // Withholding it there re-arms one-shot migrations: an unstamped file
        // whose row says 12 re-derives 12 on every open, re-enters the
        // `current < 13` arm, and `migrate_is_reference_to_kind` unconditionally
        // re-inserts `pending_kind_classification` - which the load path is
        // documented to honour ONCE, because re-running the classifier overwrites
        // the user's own Jump/Reference choices. Ongoing corruption of real user
        // data beats a hypothetical lying row.
        if !(claimed_only && current >= SCHEMA_VERSION) {
            self.set_user_version(SCHEMA_VERSION)?;
            // Keep project-level schema_version in sync with PRAGMA user_version.
            // Inside the same guard: writing it while declining to stamp would
            // overwrite the very claim the next open reads to pick its arms - a
            // row of 7 clobbered to 18 stops the v12..v17 arms running ever
            // again, on a file we just said we could not vouch for.
            let _ = self.conn.execute(
                "UPDATE project SET schema_version = ?1",
                params![SCHEMA_VERSION],
            );
        }

        chain.commit()?;
        Ok(())
    }

    /// [#576 B6-D2] Copy the file this connection has open to
    /// `<name>.stalx.pre-v<from_version>.backup` beside it, ONCE: when that copy
    /// already exists it is kept as it is, so a later upgrade from the same version
    /// never replaces the first copy of the file as it was. The recovery dialog
    /// lists it with the other backups.
    ///
    /// Called inside the migration chain's IMMEDIATE transaction before its first
    /// write, so the bytes on disk are the committed file before any upgrade step
    /// touched them (the open's quick_check already rolled back any hot journal).
    /// Skipped for an in-memory database and for a file with no project yet (an
    /// empty placeholder `Database::open` is about to fill): there is nothing to
    /// keep. A copy that cannot be written refuses the upgrade - the upgrade is
    /// one-way, and this copy is the only way back - and leaves no partial copy.
    fn keep_pre_upgrade_copy(&self, from_version: i32) -> Result<()> {
        let Some(path) = self.conn.path().filter(|p| !p.is_empty()) else {
            return Ok(());
        };
        if !self.has_project_data()? {
            return Ok(());
        }
        let path = Path::new(path);
        let Some(file_name) = path.file_name() else {
            return Ok(());
        };
        let sibling = |suffix: &str| {
            let mut name = file_name.to_os_string();
            name.push(suffix);
            path.with_file_name(name)
        };
        let copy = sibling(&format!(".pre-v{from_version}.backup"));
        if copy.symlink_metadata().is_ok() {
            tracing::info!(
                copy = ?copy,
                purpose = "keep one copy per upgrade",
                "A pre-upgrade copy from this version already exists; keeping it"
            );
            return Ok(());
        }
        // Written under a name the backup listing does not offer, then renamed,
        // so a half-written copy never shows up as a backup. [#576 RVF-b-1] As a
        // new file: `fs::copy` wrote through a link left at that name (creating
        // the file a dangling one named, overwriting the one it led to); a stale
        // copy a crash left there is replaced, and one that cannot be removed
        // refuses the upgrade.
        let partial = sibling(&format!(".pre-v{from_version}.partial"));
        let written = super::repair::copy_to_new_file(path, &partial)
            .and_then(|_| std::fs::File::open(&partial)?.sync_all())
            .and_then(|()| std::fs::rename(&partial, &copy));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&partial);
            tracing::error!(
                copy = ?copy,
                error = %e,
                purpose = "keep one copy per upgrade",
                "Could not keep a copy of the project before upgrading it; not upgrading"
            );
            anyhow::bail!(
                "Could not keep a copy of this project before upgrading it to the current \
                 file format, so it was not upgraded and was left as it was: {e}"
            );
        }
        tracing::info!(
            copy = ?copy,
            from_version,
            purpose = "keep one copy per upgrade",
            "Kept a copy of the project as it was before the upgrade"
        );
        Ok(())
    }

    /// Initialize the database schema
    pub(super) fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r"
            -- Project metadata
            CREATE TABLE IF NOT EXISTS project (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                location TEXT,
                created_at TEXT NOT NULL,
                modified_at TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                next_closure_id INTEGER NOT NULL DEFAULT 1
            );

            -- Sections
            CREATE TABLE IF NOT EXISTS sections (
                id INTEGER PRIMARY KEY,
                name TEXT,
                surveyor TEXT,
                date TEXT,
                color TEXT NOT NULL,
                direction TEXT NOT NULL DEFAULT 'in',
                quality REAL NOT NULL DEFAULT 1.0,
                reverse_parity INTEGER NOT NULL DEFAULT 0,
                gauge_shot_uuid TEXT,
                gauge_neighbour_uuid TEXT,
                azimuth_north TEXT,
                azimuth_correction REAL,
                azimuth_source TEXT,
                azimuth_epoch_year INTEGER,
                created_at TEXT NOT NULL
            );

            -- Shots
            CREATE TABLE IF NOT EXISTS shots (
                id INTEGER PRIMARY KEY,
                section_id INTEGER NOT NULL,
                sequence_order INTEGER NOT NULL,
                azimuth_in REAL NOT NULL,
                azimuth_out REAL NOT NULL,
                length REAL NOT NULL,
                depth_in REAL NOT NULL,
                depth_out REAL NOT NULL,
                pitch_in REAL,
                pitch_out REAL,
                left_distance REAL,
                right_distance REAL,
                up_distance REAL,
                down_distance REAL,
                comment TEXT,
                kind INTEGER NOT NULL DEFAULT 0,
                marker_index INTEGER,
                temperature REAL,
                uuid TEXT,
                UNIQUE(section_id, sequence_order),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );

            -- Station merges
            CREATE TABLE IF NOT EXISTS station_merges (
                id INTEGER PRIMARY KEY,
                station_a_section INTEGER NOT NULL,
                station_a_index INTEGER NOT NULL,
                station_b_section INTEGER NOT NULL,
                station_b_index INTEGER NOT NULL,
                merged_at TEXT NOT NULL,
                FOREIGN KEY (station_a_section) REFERENCES sections(id) ON DELETE CASCADE,
                FOREIGN KEY (station_b_section) REFERENCES sections(id) ON DELETE CASCADE
            );

            -- Zero datums
            CREATE TABLE IF NOT EXISTS zero_datums (
                id INTEGER PRIMARY KEY,
                section_id INTEGER NOT NULL,
                station_index INTEGER NOT NULL,
                latitude REAL NOT NULL,
                longitude REAL NOT NULL,
                depth_reference REAL NOT NULL,
                UNIQUE(section_id, station_index),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );

            -- Section offsets (for aligning merged sections)
            CREATE TABLE IF NOT EXISTS section_offsets (
                section_id INTEGER PRIMARY KEY,
                offset_x REAL NOT NULL,
                offset_y REAL NOT NULL,
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );

            -- Station offsets (for individually moved stations)
            CREATE TABLE IF NOT EXISTS station_offsets (
                section_id INTEGER NOT NULL,
                station_index INTEGER NOT NULL,
                offset_x REAL NOT NULL,
                offset_y REAL NOT NULL,
                offset_z REAL NOT NULL,
                PRIMARY KEY (section_id, station_index),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );

            -- Station comments
            CREATE TABLE IF NOT EXISTS station_comments (
                section_id INTEGER NOT NULL,
                station_index INTEGER NOT NULL,
                comment TEXT NOT NULL,
                PRIMARY KEY (section_id, station_index),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );

            -- SpeleoDB single-writer hand-off link (schema v9). Single row id=1.
            -- base_commit_sha NULL = empty-repo BASE (never coerced to ''). is_remote is
            -- DERIVED from EXISTS(this table), never stored. ADDITIVE: no future migration
            -- rewrites this table (see PRESERVED_SPELEOLINK_COLUMNS / carry-forward fence).
            -- undo_mode NULL (v16) = legacy/uninitialized; materialized on open (#303).
            CREATE TABLE IF NOT EXISTS speleo_link (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                endpoint TEXT NOT NULL,
                project_id TEXT NOT NULL,
                project_name TEXT NOT NULL,
                base_commit_sha TEXT,
                has_unpushed_edits INTEGER NOT NULL DEFAULT 0,
                last_edit_at TEXT,
                last_verify_at TEXT,
                undo_mode TEXT
            );

            -- [#303] Which SpeleoDB revision the working copy has been UNWOUND to, so
            -- the next Unwind walks to that revision's first parent rather than back to
            -- BASE. Single row id=1; absent row = not mid-unwind (next target is BASE).
            -- LOCAL-ONLY: per-machine progress through history, stripped from every push
            -- snapshot (PUSH_SNAPSHOT_LOCAL_TABLES).
            CREATE TABLE IF NOT EXISTS speleo_undo_chain (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                unwound_from_sha TEXT NOT NULL
            );

            -- [#303] How far the loaded history reaches: every undo row stamped at or
            -- before this instant came off a revision a chain step downloaded, so the next
            -- push cuts them rather than shipping them a second time. Single row id=1;
            -- absent row = nothing inherited. LOCAL-ONLY, stripped from every push
            -- snapshot (PUSH_SNAPSHOT_LOCAL_TABLES).
            CREATE TABLE IF NOT EXISTS speleo_undo_watermark (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                inherited_through TEXT NOT NULL
            );

            -- Undo/redo stack (BLOB columns hold gzip-compressed JSON)
            CREATE TABLE IF NOT EXISTS undo_stack (
                id INTEGER PRIMARY KEY,
                sequence INTEGER NOT NULL,
                command_json BLOB NOT NULL,
                undo_data_json BLOB NOT NULL,
                executed_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS redo_stack (
                id INTEGER PRIMARY KEY,
                sequence INTEGER NOT NULL,
                command_json BLOB NOT NULL,
                undo_data_json BLOB NOT NULL,
                executed_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS loop_closures (
                id INTEGER PRIMARY KEY,
                station_a_section INTEGER NOT NULL,
                station_a_index INTEGER NOT NULL,
                station_b_section INTEGER NOT NULL,
                station_b_index INTEGER NOT NULL,
                path_length REAL NOT NULL,
                misclosure_distance REAL NOT NULL,
                error_percentage REAL NOT NULL,
                horizontal_misclosure REAL NOT NULL,
                vertical_misclosure REAL NOT NULL,
                closure_id INTEGER NOT NULL DEFAULT 0,
                strategy TEXT NOT NULL DEFAULT 'Batch',
                is_aligned INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (station_a_section) REFERENCES sections(id) ON DELETE CASCADE,
                FOREIGN KEY (station_b_section) REFERENCES sections(id) ON DELETE CASCADE
            );

            -- Display settings
            CREATE TABLE IF NOT EXISTS display_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- Per-project settings (validation thresholds, #57)
            CREATE TABLE IF NOT EXISTS project_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- Resolved warnings (user acknowledged)
            CREATE TABLE IF NOT EXISTS resolved_warnings (
                id INTEGER PRIMARY KEY,
                section_id INTEGER NOT NULL,
                shot_id INTEGER NOT NULL,
                warning_code TEXT NOT NULL,
                resolved_at TEXT NOT NULL,
                resolved_comment TEXT,
                shot_uuid TEXT,
                UNIQUE(section_id, shot_id, warning_code),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );

            -- Imported file hashes (for duplicate detection)
            CREATE TABLE IF NOT EXISTS imported_file_hashes (
                id INTEGER PRIMARY KEY,
                hash TEXT NOT NULL UNIQUE
            );

            -- Imported section hashes (section-level duplicate detection)
            CREATE TABLE IF NOT EXISTS imported_section_hashes (
                id INTEGER PRIMARY KEY,
                hash TEXT NOT NULL UNIQUE
            );

            -- Indexes
            CREATE INDEX IF NOT EXISTS idx_shots_section ON shots(section_id);
            CREATE INDEX IF NOT EXISTS idx_undo_sequence ON undo_stack(sequence);
            ",
        )?;

        Ok(())
    }
}
