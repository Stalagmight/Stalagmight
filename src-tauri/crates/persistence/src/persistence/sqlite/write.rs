use anyhow::Result;
use chrono::Utc;
use rusqlite::params;

use crate::models::{CreationStamp, Project};

use super::{Database, SCHEMA_VERSION};

/// Every table [`Database::save_project`] writes from the [`Project`] model: the
/// survey tables it wipes and rewrites, and `project_settings`, whose keys it
/// upserts or deletes.
///
/// The app's save builds a FRESH file and renames it over the destination, so a
/// table neither listed here nor carried by hand is emptied by every save. The
/// save pipeline's side-table registry (`core::state::save_sidecars`) carries the
/// rest, and its coverage test pins that the two lists together name every table
/// `init_schema` creates (#576 A3). The sibling test pins that this list is what
/// the transaction below actually writes.
pub const SAVE_PROJECT_TABLES: &[&str] = &[
    "project",
    "sections",
    "shots",
    "station_merges",
    "zero_datums",
    "section_offsets",
    "station_offsets",
    "station_comments",
    "loop_closures",
    "imported_file_hashes",
    "imported_section_hashes",
    "project_settings",
];

impl Database {
    /// Save a project to the database. Writes exactly [`SAVE_PROJECT_TABLES`].
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
                // [#576 NF-W18-1] As stored, byte for byte (as a section's stamp).
                project.created_at.as_str(),
                project.modified_at.to_rfc3339(),
                SCHEMA_VERSION,
                next_closure_id_i64
            ],
        )?;

        // Save sections and shots. [#576 NF-W14BE-1] `created_at` is the
        // section's creation time, stamped once: the model carries the value its
        // first save recorded (read back on open, stamped by the app's save heals
        // before that first save), and only a section that has none yet - a
        // writer that skips the heals - takes this save's time. [#576 RV14-2]
        // A stamp is written as the text it holds: as stored, byte for byte.
        let now = Utc::now().to_rfc3339();
        for section in &project.sections {
            tx.execute(
                "INSERT INTO sections (id, name, surveyor, date, color, direction, quality, reverse_parity, gauge_shot_uuid, gauge_neighbour_uuid, azimuth_north, azimuth_correction, azimuth_source, azimuth_epoch_year, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    section.id,
                    section.name,
                    section.surveyor,
                    section.date.map(|d| d.to_string()),
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
                    section
                        .created_at
                        .as_ref()
                        .map_or(now.as_str(), CreationStamp::as_str)
                ],
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
                let uuid = shot
                    .uuid
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
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

        // [#576 NF-W13BE-3] The three tables held in a `HashMap` (section
        // offsets, station offsets, station comments) are written in key order,
        // not map order. Every map built anew - an open builds them from the
        // file - has its own random seed, so the same project opened and saved
        // again wrote these rows in another order: other rowids, other page
        // bytes, though nothing had changed.

        // Save section offsets
        let mut section_offsets: Vec<_> = project.section_offsets().iter().collect();
        section_offsets.sort_by_key(|(section_id, _)| **section_id);
        for (section_id, (offset_x, offset_y)) in section_offsets {
            tx.execute(
                "INSERT INTO section_offsets (section_id, offset_x, offset_y) VALUES (?1, ?2, ?3)",
                params![section_id, offset_x, offset_y],
            )?;
        }

        // [#576 RV13-6] One row per station: two keys naming one station each
        // wrote a row, and the second failed the whole save on the tables'
        // primary key. They are settled as the re-anchor on open settles them
        // (`Project::one_row_per_station`): the canonical key's offset, both
        // comments.
        let mut offsets_dropped = 0_usize;
        let mut comments_joined = 0_usize;

        // Save station offsets
        let mut station_offsets = Vec::with_capacity(project.station_offsets.len());
        for (key, offset) in &project.station_offsets {
            let Some(sr) = project.resolve_key(key) else {
                tracing::warn!(
                    purpose = "skip orphan station offset on save",
                    "Dropping a station offset whose station no longer resolves"
                );
                continue;
            };
            station_offsets.push((key, sr, *offset));
        }
        let station_offsets =
            project.one_row_per_station(station_offsets, |_kept, _other| offsets_dropped += 1);
        for (sr, (offset_x, offset_y, offset_z)) in station_offsets {
            tx.execute(
                "INSERT INTO station_offsets (section_id, station_index, offset_x, offset_y, offset_z) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![sr.section_id, sr.station_index, offset_x, offset_y, offset_z],
            )?;
        }

        // Save station comments
        let mut station_comments = Vec::with_capacity(project.station_comments.len());
        for (key, comment) in &project.station_comments {
            let Some(sr) = project.resolve_key(key) else {
                tracing::warn!(
                    purpose = "skip orphan station comment on save",
                    "Dropping a station comment whose station no longer resolves"
                );
                continue;
            };
            station_comments.push((key, sr, comment.clone()));
        }
        let station_comments = project.one_row_per_station(station_comments, |kept, other| {
            comments_joined += 1;
            crate::models::join_station_comment(kept, other);
        });
        for (sr, comment) in station_comments {
            tx.execute(
                "INSERT INTO station_comments (section_id, station_index, comment) VALUES (?1, ?2, ?3)",
                params![sr.section_id, sr.station_index, comment],
            )?;
        }
        if offsets_dropped > 0 || comments_joined > 0 {
            tracing::warn!(
                offsets_dropped,
                comments_joined,
                purpose = "write one station offset and comment row per station (#576 RV13-6)",
                "Two keys named one station on save: kept the canonical key's offset and \
                 joined the comments"
            );
        }

        // Save loop closures. The `closure_id`, `strategy` and `is_aligned`
        // columns are the schema's business, not the save's: `init_schema`
        // creates them on a fresh file and `ensure_schema_version` migrates an
        // existing one on open (B23, #576). A save must never run DDL - the
        // SpeleoDB writers save on the LIVE file, and a copy of those ALTERs
        // here used to rewrite its schema mid-save.
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
