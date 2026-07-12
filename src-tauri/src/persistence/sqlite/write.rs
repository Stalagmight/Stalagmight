use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::params;

use crate::models::{MarkerDirection, MarkerType, Project};

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
            DELETE FROM loop_closures;
            DELETE FROM markers;
            DELETE FROM imported_file_hashes;
            DELETE FROM imported_section_hashes;
            DELETE FROM collab_state;
            DELETE FROM project;
            ",
        )?;

        // Save project metadata (including monotonic ID counters)
        let next_closure_id_i64 = i64::try_from(project.next_closure_id).map_err(|_| {
            anyhow::anyhow!(
                "next_closure_id {} exceeds i64 range",
                project.next_closure_id
            )
        })?;
        let next_marker_id_i64 = i64::try_from(project.next_marker_id).map_err(|_| {
            anyhow::anyhow!(
                "next_marker_id {} exceeds i64 range",
                project.next_marker_id
            )
        })?;
        tx.execute(
            "INSERT INTO project (id, name, location, created_at, modified_at, schema_version, next_closure_id, next_marker_id) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                project.name,
                project.location,
                project.created_at.to_rfc3339(),
                project.modified_at.to_rfc3339(),
                SCHEMA_VERSION,
                next_closure_id_i64,
                next_marker_id_i64
            ],
        )?;

        // Save sections and shots
        for section in &project.sections {
            tx.execute(
                "INSERT INTO sections (id, name, surveyor, date, color, direction, quality, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    section.id,
                    section.name,
                    section.surveyor,
                    section.date.map(|d| d.to_string()),
                    section.color,
                    section.direction.to_string(),
                    section.quality,
                    Utc::now().to_rfc3339()
                ],
            )?;

            for (seq, shot) in section.shots.iter().enumerate() {
                // Note: We don't store shot.id - it's regenerated on load based on sequence_order
                // The database uses auto-increment for the id column.
                // `shot.uuid` IS persisted — it's the only stable identity
                // a shot has across save/load and the basis for the
                // collaboration round-trip with Ariane.  Mint one lazily
                // here for any shot that arrived without (e.g. loaded
                // from a pre-v6 .stalx) so we never write NULL twice.
                let uuid = shot
                    .uuid
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                tx.execute(
                    r"INSERT INTO shots (
                        section_id, sequence_order,
                        azimuth_in, azimuth_out, length, depth_in, depth_out,
                        pitch_in, pitch_out, left_distance, right_distance, up_distance, down_distance,
                        comment, is_reference, marker_index, temperature, uuid
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
                        i32::from(shot.is_reference),
                        shot.marker_index,
                        shot.temperature,
                        uuid,
                    ],
                )?;
            }
        }

        // Save station merges. Cross-references are stored as stable
        // `StationKey` anchors in memory but persisted positionally (resolved
        // against the current shots) — the on-disk format stays positional, like
        // the binary IPC wire; load reconstructs anchors and re-anchors them.
        // A dangling endpoint is SKIPPED, never collapsed to (section, 0): the
        // app-layer save gate already blocks this, but the persistence layer must
        // not crash (PK collision) or write a phantom for any path that reaches
        // it directly (collab sibling write, salvage).
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

        // Save station comments
        for (key, comment) in &project.station_comments {
            let Some(sr) = project.resolve_key(key) else {
                tracing::warn!(
                    purpose = "skip orphan station comment on save",
                    "Dropping a station comment whose station no longer resolves"
                );
                continue;
            };
            tx.execute(
                "INSERT INTO station_comments (section_id, station_index, comment) VALUES (?1, ?2, ?3)",
                params![sr.section_id, sr.station_index, comment],
            )?;
        }

        // Save collaboration state (single-row JSON blob)
        if let Some(collab) = &project.collab {
            let json = serde_json::to_string(collab).context("Failed to serialize collab state")?;
            tx.execute(
                "INSERT INTO collab_state (id, state_json) VALUES (1, ?1)",
                params![json],
            )?;
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

        // Build a shot-UUID lookup so markers are tied to their host shot by the
        // stable UUID rather than the positionally-reconstructed in-memory ID.
        let shot_uuid_map: std::collections::HashMap<(u32, u32), &str> = project
            .sections
            .iter()
            .flat_map(|s| {
                s.shots.iter().filter_map(move |shot| {
                    shot.uuid.as_deref().map(|uuid| ((s.id, shot.id), uuid))
                })
            })
            .collect();

        // Save markers
        for marker in &project.markers {
            let marker_id_i64 = i64::try_from(marker.id)
                .map_err(|_| anyhow::anyhow!("Marker ID {} exceeds i64 range", marker.id))?;
            let marker_type_str = match marker.marker_type {
                MarkerType::Ndm => "Ndm",
                MarkerType::Arrow => "Arrow",
            };
            let direction_str = marker.direction.as_ref().map(|d| match d {
                MarkerDirection::TowardStart => "TowardStart",
                MarkerDirection::TowardEnd => "TowardEnd",
            });
            let shot_uuid = shot_uuid_map
                .get(&(marker.section_id, marker.shot_id))
                .copied();
            tx.execute(
                "INSERT INTO markers (id, section_id, shot_id, shot_uuid, position, marker_type, color, text, direction) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    marker_id_i64,
                    marker.section_id,
                    marker.shot_id,
                    shot_uuid,
                    f64::from(marker.position),
                    marker_type_str,
                    marker.color,
                    marker.text,
                    direction_str,
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
