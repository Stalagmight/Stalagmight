use anyhow::Result;

use super::Database;

impl Database {
    /// Run PRAGMA quick_check to detect database corruption.
    /// This is a fast subset of integrity_check that skips index verification.
    /// Called automatically on project open per SPEC 14.4.
    pub(super) fn validate_integrity(&self) -> Result<()> {
        tracing::debug!("Running PRAGMA quick_check");
        let result: String = self
            .conn
            .query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if result != "ok" {
            tracing::error!(result = %result, "Database integrity check failed");
            anyhow::bail!("Database integrity check failed: {result}");
        }
        tracing::debug!("PRAGMA quick_check passed");
        Ok(())
    }

    /// Verify there are no foreign key violations.
    pub(super) fn validate_foreign_keys(&self) -> Result<()> {
        let mut stmt = self.conn.prepare("PRAGMA foreign_key_check")?;
        let mut rows = stmt.query([])?;
        let mut count = 0usize;
        let mut samples = Vec::new();

        while let Some(row) = rows.next()? {
            count += 1;
            if samples.len() < 3 {
                let table: String = row.get(0)?;
                let rowid: i64 = row.get(1)?;
                let parent: String = row.get(2)?;
                let fkid: i64 = row.get(3)?;
                samples.push(format!("{table} rowid={rowid} parent={parent} fkid={fkid}"));
            }
        }

        if count > 0 {
            anyhow::bail!(
                "Foreign key violations detected ({}). Sample: {}",
                count,
                samples.join("; ")
            );
        }

        Ok(())
    }

    /// Rebuild key tables with foreign key constraints.
    pub(super) fn rebuild_tables_with_foreign_keys(&mut self) -> Result<()> {
        tracing::info!("Rebuilding tables with foreign key constraints (schema v2)");

        let has_lines = self.table_exists("sections")?;
        if !has_lines {
            anyhow::bail!("Missing lines table during FK migration");
        }

        let has_shots = self.table_exists("shots")?;
        let has_station_merges = self.table_exists("station_merges")?;
        let has_zero_datums = self.table_exists("zero_datums")?;
        let has_line_offsets = self.table_exists("section_offsets")?;
        let has_station_offsets = self.table_exists("station_offsets")?;
        let has_loop_closures = self.table_exists("loop_closures")?;
        let has_resolved_warnings = self.table_exists("resolved_warnings")?;

        self.conn.execute("PRAGMA foreign_keys = OFF;", [])?;
        let tx = self.conn.transaction()?;

        // Rename old tables
        tx.execute("ALTER TABLE sections RENAME TO sections_old", [])?;
        if has_shots {
            tx.execute("ALTER TABLE shots RENAME TO shots_old", [])?;
        }
        if has_station_merges {
            tx.execute(
                "ALTER TABLE station_merges RENAME TO station_merges_old",
                [],
            )?;
        }
        if has_zero_datums {
            tx.execute("ALTER TABLE zero_datums RENAME TO zero_datums_old", [])?;
        }
        if has_line_offsets {
            tx.execute(
                "ALTER TABLE section_offsets RENAME TO section_offsets_old",
                [],
            )?;
        }
        if has_station_offsets {
            tx.execute(
                "ALTER TABLE station_offsets RENAME TO station_offsets_old",
                [],
            )?;
        }
        if has_loop_closures {
            tx.execute("ALTER TABLE loop_closures RENAME TO loop_closures_old", [])?;
        }
        if has_resolved_warnings {
            tx.execute(
                "ALTER TABLE resolved_warnings RENAME TO resolved_warnings_old",
                [],
            )?;
        }

        // Recreate tables with FK constraints
        tx.execute_batch(
            r"
            CREATE TABLE sections (
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
                created_at TEXT NOT NULL
            );

            CREATE TABLE shots (
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

            CREATE TABLE station_merges (
                id INTEGER PRIMARY KEY,
                station_a_section INTEGER NOT NULL,
                station_a_index INTEGER NOT NULL,
                station_b_section INTEGER NOT NULL,
                station_b_index INTEGER NOT NULL,
                merged_at TEXT NOT NULL,
                FOREIGN KEY (station_a_section) REFERENCES sections(id) ON DELETE CASCADE,
                FOREIGN KEY (station_b_section) REFERENCES sections(id) ON DELETE CASCADE
            );

            CREATE TABLE zero_datums (
                id INTEGER PRIMARY KEY,
                section_id INTEGER NOT NULL,
                station_index INTEGER NOT NULL,
                latitude REAL NOT NULL,
                longitude REAL NOT NULL,
                depth_reference REAL NOT NULL,
                UNIQUE(section_id, station_index),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );

            CREATE TABLE section_offsets (
                section_id INTEGER PRIMARY KEY,
                offset_x REAL NOT NULL,
                offset_y REAL NOT NULL,
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );

            CREATE TABLE station_offsets (
                section_id INTEGER NOT NULL,
                station_index INTEGER NOT NULL,
                offset_x REAL NOT NULL,
                offset_y REAL NOT NULL,
                offset_z REAL NOT NULL,
                PRIMARY KEY (section_id, station_index),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );

            CREATE TABLE loop_closures (
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

            CREATE TABLE resolved_warnings (
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
            ",
        )?;

        // Copy data
        tx.execute(
            // Every column `migrate_schema` conditionally ADDs to `lines` has to appear
            // here as well. That migration runs BEFORE this rebuild, so `lines_old` already
            // carries them - but this recreates the table from scratch, so anything missing
            // from the DDL and the copy is silently dropped and the file is then stamped
            // current, so no later open re-adds it. `save_project` inserts every column by
            // name, which makes the result an UNSAVEABLE project: loading still works (the
            // read path falls back to a literal for an absent column) and only the next save
            // fails, long after the migration that caused it.
            "INSERT INTO sections (id, name, surveyor, date, color, direction, quality, reverse_parity, gauge_shot_uuid, gauge_neighbour_uuid, created_at)
             SELECT id, name, surveyor, date, color, direction, quality, reverse_parity, gauge_shot_uuid, gauge_neighbour_uuid, created_at FROM sections_old",
            [],
        )?;

        if has_shots {
            // The old table may carry any of the three historical spellings:
            // `kind` (v13+, already a 0/1/2 discriminant), `is_reference` (v4+)
            // or `is_virtual` (pre-v4). The two boolean forms are widened to the
            // discriminant with `!= 0` -> 1 == ShotKind::Jump, matching the
            // v12->v13 migration's seed.
            let virtual_col = if tx.prepare("SELECT kind FROM shots_old LIMIT 0").is_ok() {
                "kind".to_string()
            } else if tx
                .prepare("SELECT is_reference FROM shots_old LIMIT 0")
                .is_ok()
            {
                "CASE WHEN is_reference != 0 THEN 1 ELSE 0 END".to_string()
            } else {
                "CASE WHEN is_virtual != 0 THEN 1 ELSE 0 END".to_string()
            };
            tx.execute(
                &format!(
                    "INSERT INTO shots (
                        id, section_id, sequence_order,
                        azimuth_in, azimuth_out, length, depth_in, depth_out,
                        pitch_in, pitch_out, left_distance, right_distance, up_distance, down_distance,
                        comment, kind, marker_index, temperature
                    )
                    SELECT
                        id, section_id, sequence_order,
                        azimuth_in, azimuth_out, length, depth_in, depth_out,
                        pitch_in, pitch_out, left_distance, right_distance, up_distance, down_distance,
                        comment, {virtual_col}, marker_index, temperature
                    FROM shots_old"
                ),
                [],
            )?;
        }

        if has_station_merges {
            tx.execute(
                "INSERT INTO station_merges (id, station_a_section, station_a_index, station_b_section, station_b_index, merged_at)
                 SELECT id, station_a_section, station_a_index, station_b_section, station_b_index, merged_at
                 FROM station_merges_old",
                [],
            )?;
        }

        if has_zero_datums {
            tx.execute(
                "INSERT INTO zero_datums (id, section_id, station_index, latitude, longitude, depth_reference)
                 SELECT id, section_id, station_index, latitude, longitude, depth_reference
                 FROM zero_datums_old",
                [],
            )?;
        }

        if has_line_offsets {
            tx.execute(
                "INSERT INTO section_offsets (section_id, offset_x, offset_y)
                 SELECT section_id, offset_x, offset_y FROM section_offsets_old",
                [],
            )?;
        }

        if has_station_offsets {
            tx.execute(
                "INSERT INTO station_offsets (section_id, station_index, offset_x, offset_y, offset_z)
                 SELECT section_id, station_index, offset_x, offset_y, offset_z FROM station_offsets_old",
                [],
            )?;
        }

        if has_loop_closures {
            tx.execute(
                "INSERT INTO loop_closures (
                    id, station_a_section, station_a_index, station_b_section, station_b_index,
                    path_length, misclosure_distance, error_percentage, horizontal_misclosure, vertical_misclosure,
                    closure_id, strategy
                )
                SELECT
                    id, station_a_section, station_a_index, station_b_section, station_b_index,
                    path_length, misclosure_distance, error_percentage, horizontal_misclosure, vertical_misclosure,
                    closure_id, strategy
                FROM loop_closures_old",
                [],
            )?;
        }

        if has_resolved_warnings {
            tx.execute(
                "INSERT INTO resolved_warnings (id, section_id, shot_id, warning_code, resolved_at, resolved_comment)
                 SELECT id, section_id, shot_id, warning_code, resolved_at, resolved_comment
                 FROM resolved_warnings_old",
                [],
            )?;
        }

        // Drop old tables
        tx.execute("DROP TABLE sections_old", [])?;
        if has_shots {
            tx.execute("DROP TABLE shots_old", [])?;
        }
        if has_station_merges {
            tx.execute("DROP TABLE station_merges_old", [])?;
        }
        if has_zero_datums {
            tx.execute("DROP TABLE zero_datums_old", [])?;
        }
        if has_line_offsets {
            tx.execute("DROP TABLE section_offsets_old", [])?;
        }
        if has_station_offsets {
            tx.execute("DROP TABLE station_offsets_old", [])?;
        }
        if has_loop_closures {
            tx.execute("DROP TABLE loop_closures_old", [])?;
        }
        if has_resolved_warnings {
            tx.execute("DROP TABLE resolved_warnings_old", [])?;
        }

        // Recreate indexes
        tx.execute(
            "CREATE INDEX IF NOT EXISTS idx_shots_section ON shots(section_id)",
            [],
        )?;

        tx.commit()?;
        self.enable_foreign_keys()?;
        Ok(())
    }
}
