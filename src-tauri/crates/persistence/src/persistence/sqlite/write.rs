use anyhow::Result;
use chrono::Utc;
use rusqlite::params;

use crate::models::Project;

use super::shot_credits::{
    write_credit_rows, write_recorded_legacy, CREATE_CREDIT_TABLES, RECREATE_CREDIT_TABLES,
};
use super::station_texts::{StationTextTable, CREATE_STATION_NAMES};
use super::{Database, SCHEMA_VERSION};

impl Database {
    /// Save a project to the database
    pub fn save_project(&mut self, project: &Project) -> Result<()> {
        tracing::debug!(
            "Saving project: name={}, sections={}",
            project.name,
            project.sections.len()
        );

        let tx = match self.conn.transaction() {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("Failed to start transaction: {}", e);
                return Err(e.into());
            }
        };

        // `station_names` carries no schema bump (see `station_texts`), so a v18
        // file opened through `ensure_schema_version`'s fast path - the live
        // connection a save can reach, e.g. the SpeleoDB rename path - may not
        // have it yet. Create it here, inside the transaction, the way the
        // `closure_id` column is ensured below, so the DELETE cannot fail.
        tx.execute(CREATE_STATION_NAMES, [])?;
        // The per-shot credit tables carry no bump either (see `shot_credits`);
        // rewritten whole by every save, so recreated rather than emptied.
        tx.execute_batch(RECREATE_CREDIT_TABLES)?;
        tx.execute_batch(CREATE_CREDIT_TABLES)?;

        // Clear existing data (delete children before parents for foreign key compliance)
        // NOTE: speleo_link is intentionally NOT cleared here (see save_speleo_link
        // / D-B13c). save_project wipes-and-rewrites every table it owns; if
        // speleo_link were in that set, a local autosave would clear
        // has_unpushed_edits on every write. The link is mutated ONLY through
        // save_speleo_link (stamp on pull) and the dirty-bit setter (#191).
        tx.execute_batch(
            r"
            DELETE FROM shots;
            DELETE FROM sections;
            DELETE FROM station_merges;
            DELETE FROM zero_datums;
            DELETE FROM section_offsets;
            DELETE FROM station_offsets;
            DELETE FROM station_comments;
            DELETE FROM station_names;
            DELETE FROM loop_closures;
            DELETE FROM imported_file_hashes;
            DELETE FROM imported_section_hashes;
            DELETE FROM project;
            ",
        )?;

        // Save project metadata (including next_closure_id for monotonic ID persistence)
        let next_closure_id_i64 = i64::try_from(project.next_closure_id).map_err(|_| {
            anyhow::anyhow!(
                "next_closure_id {} exceeds i64 range",
                project.next_closure_id
            )
        })?;
        tx.execute(
            "INSERT INTO project (id, name, location, created_at, modified_at, schema_version, next_closure_id) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                project.name,
                project.location,
                project.created_at.to_rfc3339(),
                project.modified_at.to_rfc3339(),
                SCHEMA_VERSION,
                next_closure_id_i64
            ],
        )?;

        // Credit rows are keyed by (section, shot uuid): see `shot_credits`. A
        // uuid repeated INSIDE one section (a corrupt file the save heals
        // elsewhere) keeps its first shot's row.
        let mut credit_rows: Vec<((u32, String), crate::models::SurveyCredit)> = Vec::new();
        let mut credit_keys: std::collections::HashSet<(u32, String)> =
            std::collections::HashSet::new();

        // Save sections and shots
        for section in &project.sections {
            // The legacy columns keep their meaning for every older reader: the
            // distinct NAMES (two people sharing a name appear once), as the
            // surveyors joined with ", " or, when anyone explored the section,
            // "explorers / surveyors" - all an older build keeps after it saves
            // (#718) - and the earliest date. A shot-less section writes back
            // the legacy pair it keeps.
            let (legacy_surveyor, first_date) = if section.shots.is_empty() {
                (section.legacy_surveyor.clone(), section.legacy_date)
            } else {
                // The text the section was read with while its people are
                // unchanged, so a save does not cut or respell it.
                (
                    section.legacy_surveyor_to_write(),
                    section.first_survey_date(),
                )
            };
            let legacy_date = first_date.map(|d| d.to_string());
            // What a shot with no credit row reads back as (`shot_credits`),
            // computed from the text actually written: a shot whose credit it
            // does not reproduce (both same-named people, an email, a name
            // whose " / " the text spells " - ") gets a row.
            let fallback =
                crate::models::SurveyCredit::from_legacy(legacy_surveyor.as_deref(), first_date);
            tx.execute(
                "INSERT INTO sections (id, name, surveyor, date, color, direction, quality, reverse_parity, gauge_shot_uuid, gauge_neighbour_uuid, azimuth_north, azimuth_correction, azimuth_source, azimuth_epoch_year, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    section.id,
                    section.name,
                    legacy_surveyor,
                    legacy_date,
                    section.color,
                    section.direction.to_string(),
                    section.quality,
                    i64::from(section.reverse_parity),
                    section.gauge_shot_uuid.as_deref(),
                    section.gauge_neighbour_uuid.as_deref(),
                    // All four NULL together mean "no reference recorded" (#438),
                    // which reads back as `None` - inherit the project default.
                    section.azimuth_reference.map(|r| r.north.as_str()),
                    // `applied_correction`, not the raw field: what is stored has
                    // to be the rotation geometry APPLIED, or a project saved
                    // with a non-finite correction (which applies as 0) reloads
                    // as a NaN column and the read path's repair is the only
                    // thing between it and the walk.
                    section.azimuth_reference.map(|r| r.applied_correction()),
                    section.azimuth_reference.map(|r| r.source.as_str()),
                    section
                        .azimuth_reference
                        .and_then(|r| r.source.epoch_year()),
                    Utc::now().to_rfc3339()
                ],
            )?;
            // What this build wrote, so a later open can tell a pair retyped
            // by something else from ours (`shot_credits`).
            write_recorded_legacy(
                &tx,
                section.id,
                (legacy_surveyor.as_deref(), legacy_date.as_deref()),
            )?;

            for (seq, shot) in section.shots.iter().enumerate() {
                // Note: We don't store shot.id - it's regenerated on load based on sequence_order
                // The database uses auto-increment for the id column.
                // `shot.uuid` IS persisted - it's the only stable identity
                // a shot has across save/load: station cross-references
                // anchor to it (`StationKey::AfterShot`), resolved-warning
                // rows are keyed by it, and the Ariane export emits it as
                // `<UUID>`.  Mint one lazily here for any shot that arrived
                // without (e.g. loaded from a pre-v6 .stalx) so we never
                // write NULL twice.
                let recorded = shot.uuid.as_deref().filter(|u| !u.is_empty());
                let uuid =
                    recorded.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_string);
                // A row only where the legacy fallback would read back something
                // else - so an uncredited shot, or one that agrees with its
                // section's columns, costs nothing.
                if shot.credit() != &fallback && credit_keys.insert((section.id, uuid.clone())) {
                    credit_rows.push(((section.id, uuid.clone()), shot.credit().clone()));
                }
                tx.execute(
                    r"INSERT INTO shots (
                        section_id, sequence_order,
                        azimuth_in, azimuth_out, length, depth_in, depth_out,
                        pitch_in, pitch_out, left_distance, right_distance, up_distance, down_distance,
                        comment, kind, marker_index, temperature, uuid
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                    params![
                        section.id,
                        seq as i64,
                        shot.azimuth_in,
                        shot.azimuth_out,
                        shot.length,
                        shot.depth_in,
                        shot.depth_out,
                        shot.pitch_in,
                        shot.pitch_out,
                        shot.left,
                        shot.right,
                        shot.up,
                        shot.down,
                        shot.comment,
                        i32::from(shot.kind.as_u8()),
                        shot.marker_index,
                        shot.temperature,
                        uuid,
                    ],
                )?;
            }
        }

        write_credit_rows(&tx, &credit_rows)?;

        // Save station merges. Cross-references are stored as stable
        // `StationKey` anchors in memory but persisted positionally (resolved
        // against the current shots) - the on-disk format stays positional, like
        // the binary IPC wire; load reconstructs anchors and re-anchors them.
        // A dangling endpoint is SKIPPED, never collapsed to (section, 0): the
        // app-layer save gate already blocks this, but the persistence layer must
        // not crash (PK collision) or write a phantom for any path that reaches
        // it directly (salvage).
        for merge in &project.station_merges {
            let (Some(a), Some(b)) = (
                project.resolve_key(&merge.station_a),
                project.resolve_key(&merge.station_b),
            ) else {
                tracing::warn!(
                    purpose = "skip dangling merge on save",
                    "Dropping a station merge whose endpoint no longer resolves"
                );
                continue;
            };
            tx.execute(
                "INSERT INTO station_merges (station_a_section, station_a_index, station_b_section, station_b_index, merged_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    a.section_id,
                    a.station_index,
                    b.section_id,
                    b.station_index,
                    merge.merged_at.to_rfc3339()
                ],
            )?;
        }

        // Save zero datums
        for zd in &project.zero_datums {
            let Some(sr) = project.resolve_key(&zd.station_ref) else {
                tracing::warn!(
                    purpose = "skip dangling zero datum on save",
                    "Dropping a zero datum whose station no longer resolves"
                );
                continue;
            };
            tx.execute(
                "INSERT INTO zero_datums (section_id, station_index, latitude, longitude, depth_reference) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    sr.section_id,
                    sr.station_index,
                    zd.gps.latitude,
                    zd.gps.longitude,
                    zd.gps.depth_reference
                ],
            )?;
        }

        // Save section offsets
        for (section_id, (offset_x, offset_y)) in &project.section_offsets {
            tx.execute(
                "INSERT INTO section_offsets (section_id, offset_x, offset_y) VALUES (?1, ?2, ?3)",
                params![section_id, offset_x, offset_y],
            )?;
        }

        // Save station offsets
        for (key, (offset_x, offset_y, offset_z)) in &project.station_offsets {
            let Some(sr) = project.resolve_key(key) else {
                tracing::warn!(
                    purpose = "skip orphan station offset on save",
                    "Dropping a station offset whose station no longer resolves"
                );
                continue;
            };
            tx.execute(
                "INSERT INTO station_offsets (section_id, station_index, offset_x, offset_y, offset_z) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![sr.section_id, sr.station_index, offset_x, offset_y, offset_z],
            )?;
        }

        // Save station comments and names
        for (table, texts) in [
            (StationTextTable::Comments, &project.station_comments),
            (StationTextTable::Names, &project.station_names),
        ] {
            let mut insert = tx.prepare_cached(table.insert_sql())?;
            for (key, text) in texts {
                let Some(sr) = project.resolve_key(key) else {
                    tracing::warn!(
                        purpose = "skip orphan station text on save",
                        table = table.name(),
                        "Dropping a station comment or name whose station no longer resolves"
                    );
                    continue;
                };
                insert.execute(params![sr.section_id, sr.station_index, text])?;
            }
        }

        // Save loop closures (with closure_id column)
        // Ensure closure_id column exists
        let has_closure_id: bool = tx
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('loop_closures') WHERE name='closure_id'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);
        if !has_closure_id {
            tx.execute(
                "ALTER TABLE loop_closures ADD COLUMN closure_id INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }

        // Ensure strategy column exists
        let has_strategy: bool = tx
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('loop_closures') WHERE name='strategy'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);
        if !has_strategy {
            tx.execute(
                "ALTER TABLE loop_closures ADD COLUMN strategy TEXT NOT NULL DEFAULT 'Batch'",
                [],
            )?;
        }

        // Ensure is_aligned column exists
        let has_is_aligned: bool = tx
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('loop_closures') WHERE name='is_aligned'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);
        if !has_is_aligned {
            tx.execute(
                "ALTER TABLE loop_closures ADD COLUMN is_aligned INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }

        for lc in &project.loop_closures {
            let closure_id_i64 = i64::try_from(lc.id)
                .map_err(|_| anyhow::anyhow!("Closure ID {} exceeds i64 range", lc.id))?;
            let (Some(a), Some(b)) = (
                project.resolve_key(&lc.station_a),
                project.resolve_key(&lc.station_b),
            ) else {
                tracing::warn!(
                    purpose = "skip dangling loop closure on save",
                    "Dropping a loop closure whose endpoint no longer resolves"
                );
                continue;
            };
            tx.execute(
                "INSERT INTO loop_closures (station_a_section, station_a_index, station_b_section, station_b_index, path_length, misclosure_distance, error_percentage, horizontal_misclosure, vertical_misclosure, closure_id, strategy, is_aligned) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    a.section_id,
                    a.station_index,
                    b.section_id,
                    b.station_index,
                    lc.path_length,
                    lc.misclosure_distance,
                    lc.error_percentage,
                    lc.horizontal_misclosure,
                    lc.vertical_misclosure,
                    closure_id_i64,
                    lc.strategy.as_str(),
                    i32::from(lc.is_aligned)
                ],
            )?;
        }

        // Save imported file hashes
        for hash in &project.imported_file_hashes {
            tx.execute(
                "INSERT INTO imported_file_hashes (hash) VALUES (?1)",
                params![hash],
            )?;
        }

        // Save imported section hashes
        for hash in &project.imported_section_hashes {
            tx.execute(
                "INSERT INTO imported_section_hashes (hash) VALUES (?1)",
                params![hash],
            )?;
        }

        // Per-project validation thresholds (#57) travel in the .stalx.
        tx.execute(
            "INSERT OR REPLACE INTO project_settings (key, value) VALUES ('validation', ?1)",
            params![serde_json::to_string(&project.validation)?],
        )?;

        // Entrance elevation (#431), same key/value table as `validation`.
        // `None` deletes the key rather than writing a sentinel, so a project
        // that never had one round-trips through save/load without ever
        // gaining a `surface_elevation` row.
        match project.surface_elevation {
            Some(elevation) => {
                tx.execute(
                    "INSERT OR REPLACE INTO project_settings (key, value) VALUES ('surface_elevation', ?1)",
                    params![elevation.to_string()],
                )?;
            }
            None => {
                tx.execute(
                    "DELETE FROM project_settings WHERE key = 'surface_elevation'",
                    [],
                )?;
            }
        }

        // Project-level default north reference (#438), same key/value table.
        // Absent rather than a sentinel for the same reason `surface_elevation`
        // is: a project that never declared one must round-trip unchanged.
        match project.default_azimuth_reference {
            Some(reference) => {
                tx.execute(
                    "INSERT OR REPLACE INTO project_settings (key, value) VALUES ('default_azimuth_reference', ?1)",
                    params![serde_json::to_string(&reference)?],
                )?;
            }
            None => {
                tx.execute(
                    "DELETE FROM project_settings WHERE key = 'default_azimuth_reference'",
                    [],
                )?;
            }
        }

        // Shots whose record parity runs against their section's (dive-plan
        // pins only), same table. Only shots the project still holds are
        // written; absent when none, so a project that never combined is
        // unchanged.
        let held: std::collections::HashSet<&str> = project
            .sections
            .iter()
            .flat_map(|s| s.shots.iter().filter_map(|shot| shot.uuid.as_deref()))
            .collect();
        let counter_parity: Vec<&String> = project
            .counter_parity_shots
            .iter()
            .filter(|uuid| held.contains(uuid.as_str()))
            .collect();
        if counter_parity.is_empty() {
            tx.execute(
                "DELETE FROM project_settings WHERE key = 'counter_parity_shots'",
                [],
            )?;
        } else {
            tx.execute(
                "INSERT OR REPLACE INTO project_settings (key, value) VALUES ('counter_parity_shots', ?1)",
                params![serde_json::to_string(&counter_parity)?],
            )?;
        }

        // Stations a split, combine or delete-and-split held as their
        // component's least-squares gauge (`Project::held_gauges`), same
        // table. Only the ones that still name a station; absent when none.
        let held_gauges: Vec<&crate::models::HeldGauge> = project
            .held_gauges
            .iter()
            .filter(|held| project.held_gauge_resolves(held))
            .collect();
        if held_gauges.is_empty() {
            tx.execute("DELETE FROM project_settings WHERE key = 'held_gauges'", [])?;
        } else {
            tx.execute(
                "INSERT OR REPLACE INTO project_settings (key, value) VALUES ('held_gauges', ?1)",
                params![serde_json::to_string(&held_gauges)?],
            )?;
        }

        tx.commit()?;
        tracing::debug!("Transaction committed");

        // Verify the data was written
        let verify_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0))
            .unwrap_or(0);
        tracing::debug!("Save verification: project table has {} rows", verify_count);

        Ok(())
    }
}
