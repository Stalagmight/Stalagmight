use anyhow::{Context, Result};
use chrono::{NaiveDate, Utc};
use rusqlite::OptionalExtension;
use std::collections::HashMap;

use crate::models::{
    ClosureStrategy, GpsCoordinates, LoopClosure, Marker, MarkerDirection, MarkerType, Project,
    Section, SectionData, Shot, StationKey, StationMerge, SurveyDirection, ZeroDatum,
    FEATURE_ID_STRIDE,
};

use super::Database;

impl Database {
    /// Load a project from the database
    /// Load only the metadata needed to populate `ProjectMetadata` — no
    /// per-shot deserialization. Uses SQL COUNT/SUM aggregates and a
    /// single small read of `zero_datums` for GPS info. This replaces the
    /// "load entire project just to drop everything but metadata" path
    /// used by `list_recent_projects`, which was responsible for the
    /// multi-second freeze when returning to the global view with a
    /// large project in the recent list.
    pub fn load_metadata(
        &self,
        file_path: std::path::PathBuf,
    ) -> Result<crate::models::ProjectMetadata> {
        use crate::models::{GpsBounds, GpsCoordinates, Project, ProjectMetadata};
        use std::cmp::Ordering;
        use std::time::Instant;
        let start = Instant::now();

        // Tiny project header read.
        let (name, location, modified_at_raw): (String, Option<String>, String) = self
            .conn
            .query_row(
                "SELECT name, location, modified_at FROM project LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .context("Failed to read project metadata header")?;
        let t_header = start.elapsed();

        let modified_at = chrono::DateTime::parse_from_rfc3339(&modified_at_raw)
            .context("Failed to parse modified_at")?
            .with_timezone(&Utc);

        // Aggregate-only queries.
        let section_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM sections", [], |row| row.get(0))
            .unwrap_or(0);
        let t_sections = start.elapsed();
        let shot_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM shots", [], |row| row.get(0))
            .unwrap_or(0);
        let t_shot_count = start.elapsed();
        // COALESCE so an empty shots table returns 0.0 instead of NULL.
        let total_length: f64 = self
            .conn
            .query_row("SELECT COALESCE(SUM(length), 0.0) FROM shots", [], |row| {
                row.get(0)
            })
            .unwrap_or(0.0);
        let t_sum_length = start.elapsed();

        // GPS info: zero_datums is small (typically 1-10 rows) so we read
        // it in full and reuse the existing dateline-aware bounds logic.
        let mut zero_datums: Vec<GpsCoordinates> = Vec::new();
        if let Ok(mut stmt) = self
            .conn
            .prepare("SELECT latitude, longitude, depth_reference FROM zero_datums")
        {
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, f64>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            }) {
                for r in rows.flatten() {
                    // Skip rows that don't validate (matches load_project behavior).
                    if let Ok(gps) = GpsCoordinates::new(r.0, r.1, r.2) {
                        zero_datums.push(gps);
                    }
                }
            }
        }

        let (gps_center, gps_bounds) = if zero_datums.is_empty() {
            (None, None)
        } else {
            let first = &zero_datums[0];
            let center = Some((first.longitude, first.latitude));

            let mut south = f64::INFINITY;
            let mut north = f64::NEG_INFINITY;
            let mut longitudes = Vec::with_capacity(zero_datums.len());
            for gps in &zero_datums {
                south = south.min(gps.latitude);
                north = north.max(gps.latitude);
                longitudes.push(gps.longitude);
            }
            let (west, east) = Project::compute_longitude_bounds(&longitudes);
            // Avoid f64 NaN propagation if there was only one (already handled by helper).
            let south = if south.is_finite() { south } else { 0.0 };
            let north = if north.is_finite() { north } else { 0.0 };
            let _ = Ordering::Equal; // silence the import if compute_longitude_bounds is inlined
            (
                center,
                Some(GpsBounds {
                    north,
                    south,
                    east,
                    west,
                }),
            )
        };

        let t_gps = start.elapsed();

        // station_count = shot_count + section_count per Project::stats().
        let station_count = (shot_count as usize) + (section_count as usize);

        // Trace where the time went, in ms, so we can spot which query
        // is the bottleneck. Debug-level — fires for every recent
        // project on every metadata refresh, so default releases skip
        // it.  Enable with `RUST_LOG=stalagmight_lib=debug` when you
        // need to see it.
        tracing::debug!(
            path = ?file_path,
            shots = shot_count,
            header_ms = t_header.as_micros() as f64 / 1000.0,
            sections_ms = (t_sections - t_header).as_micros() as f64 / 1000.0,
            shotcount_ms = (t_shot_count - t_sections).as_micros() as f64 / 1000.0,
            sumlength_ms = (t_sum_length - t_shot_count).as_micros() as f64 / 1000.0,
            gps_ms = (t_gps - t_sum_length).as_micros() as f64 / 1000.0,
            total_ms = t_gps.as_micros() as f64 / 1000.0,
            "load_metadata timings"
        );

        // Fast collab-on check: the recent-projects panel surfaces this
        // without opening the project, so we can't call the full
        // `load_collab_state` here (it deserialises the whole baseline).
        // Just probe the `enabled` flag from the JSON blob.
        let collab_enabled: bool = self
            .conn
            .query_row(
                "SELECT json_extract(state_json, '$.enabled') FROM collab_state WHERE id = 1",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten()
            .map(|v| v != 0)
            .unwrap_or(false);

        // The linked SpeleoDB project UUID (#201), so the recents/GlobalView list can
        // match this entry against a single lock scan. The `speleo_link` row is a
        // singleton (`CHECK(id = 1)`) with a NOT NULL project_id, so this doubles as
        // the SpeleoDB-remote probe (#189, D-B14). `.ok()` tolerates a missing table
        // on a read-only metadata open (open_for_metadata; SQLITE_OPEN_READ_ONLY runs
        // no migration, so a pre-v9 file simply has no table here) → None.
        let speleodb_project_id: Option<String> = self
            .conn
            .query_row("SELECT project_id FROM speleo_link WHERE id = 1", [], |r| {
                r.get::<_, String>(0)
            })
            .ok();

        // Derived SpeleoDB-remote flag: true iff the singleton link row exists.
        let is_remote: bool = speleodb_project_id.is_some();

        Ok(ProjectMetadata {
            name,
            location,
            file_path,
            modified_at,
            section_count: section_count as usize,
            total_length,
            station_count,
            gps_center,
            gps_bounds,
            collab_enabled,
            is_remote,
            speleodb_project_id,
        })
    }

    pub fn load_project(&self) -> Result<Project> {
        tracing::debug!("Loading project from database");

        // First check if project table has data and log what's there
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM project", [], |row| row.get(0))
            .unwrap_or(0);
        tracing::debug!("Project table row count: {}", count);

        if count == 0 {
            anyhow::bail!("Project table is empty - file may be corrupted");
        }

        // Load project metadata - get the first row (don't require id = 1)
        // Read next_closure_id if the column exists (added for monotonic ID persistence)
        let has_next_closure_id_col: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('project') WHERE name='next_closure_id'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        let (name, location, created_at, modified_at, persisted_next_closure_id): (
            String,
            Option<String>,
            String,
            String,
            Option<u64>,
        ) = if has_next_closure_id_col {
            self.conn.query_row(
                    "SELECT name, location, created_at, modified_at, next_closure_id FROM project LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, Some(row.get::<_, i64>(4).unwrap_or(1).max(1) as u64))),
                ).context("Failed to read project metadata")?
        } else {
            let (n, l, c, m) = self
                .conn
                .query_row(
                    "SELECT name, location, created_at, modified_at FROM project LIMIT 1",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .context("Failed to read project metadata")?;
            (n, l, c, m, None)
        };

        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc);
        let modified_at = chrono::DateTime::parse_from_rfc3339(&modified_at)?.with_timezone(&Utc);

        // Read next_marker_id if the column exists (added in schema v12)
        let has_next_marker_id_col: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('project') WHERE name='next_marker_id'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        let persisted_next_marker_id: Option<u64> = if has_next_marker_id_col {
            self.conn
                .query_row("SELECT next_marker_id FROM project LIMIT 1", [], |row| {
                    Ok(row.get::<_, i64>(0).unwrap_or(1).max(1) as u64)
                })
                .ok()
        } else {
            None
        };

        // Load sections
        let mut sections: Vec<Section> = Vec::new();
        let mut next_section_id = 0u32;

        {
            let has_quality_column: bool = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('sections') WHERE name='quality'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map(|c| c > 0)
                .unwrap_or(false);

            let mut stmt = if has_quality_column {
                self.conn.prepare(
                    "SELECT id, name, surveyor, date, color, direction, quality FROM sections ORDER BY id",
                )?
            } else {
                self.conn.prepare(
                    "SELECT id, name, surveyor, date, color, direction FROM sections ORDER BY id",
                )?
            };

            let mut section_rows = Vec::new();
            if has_quality_column {
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, f64>(6)?,
                    ))
                })?;
                for row in rows {
                    section_rows.push(row?);
                }
            } else {
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        1.0,
                    ))
                })?;
                for row in rows {
                    section_rows.push(row?);
                }
            }

            // Build the empty sections first so we can address them by
            // ID when partitioning the bulk shot stream below.
            let mut section_by_id: HashMap<u32, usize> = HashMap::with_capacity(section_rows.len());
            for section_row in section_rows {
                let (id, name, surveyor, date_str, color, direction_str, quality_raw) = section_row;
                let date = date_str.and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok());
                let direction = direction_str
                    .and_then(|s| s.parse::<SurveyDirection>().ok())
                    .unwrap_or_default();
                let quality = if quality_raw.is_finite() && quality_raw > 0.0 {
                    quality_raw
                } else {
                    1.0
                };

                let section_data = SectionData {
                    name,
                    surveyor,
                    date,
                    color,
                    direction: Some(direction),
                    quality,
                    shots: Vec::new(),
                };

                let section = Section::new(id, section_data);
                section_by_id.insert(id, sections.len());
                next_section_id = next_section_id.max(id + 1);
                sections.push(section);
            }

            // Load ALL shots in one SELECT and partition by section_id.
            // The legacy code prepared a per-section statement inside the
            // loop above, executing 14k+ separate queries on large files;
            // each query had to parse + plan + execute, so cost was
            // dominated by SQLite overhead rather than row I/O.  A single
            // bulk SELECT cuts roughly 350 ms off a 14k-section load and
            // is friendly to the query planner: it can stream rows
            // sequentially without re-preparing.
            {
                let mut shot_stmt = self.conn.prepare(
                    r"SELECT section_id, sequence_order, azimuth_in, azimuth_out, length, depth_in, depth_out,
                              pitch_in, pitch_out, left_distance, right_distance, up_distance, down_distance,
                              comment, is_reference, marker_index, temperature, uuid
                       FROM shots ORDER BY section_id, sequence_order",
                )?;
                let shot_rows = shot_stmt.query_map([], |row| {
                    let section_id: u32 = row.get(0)?;
                    let seq: i64 = row.get(1)?;
                    Ok((
                        section_id,
                        Shot {
                            id: section_id * FEATURE_ID_STRIDE + (seq as u32),
                            azimuth_in: row.get(2)?,
                            azimuth_out: row.get(3)?,
                            length: row.get(4)?,
                            depth_in: row.get(5)?,
                            depth_out: row.get(6)?,
                            pitch_in: row.get(7)?,
                            pitch_out: row.get(8)?,
                            left: row.get(9)?,
                            right: row.get(10)?,
                            up: row.get(11)?,
                            down: row.get(12)?,
                            comment: row.get(13)?,
                            is_reference: row.get::<_, i32>(14)? != 0,
                            marker_index: row.get(15)?,
                            temperature: row.get(16)?,
                            // `uuid` column was added in schema v6; pre-v6
                            // rows return NULL here.  Treat NULL and empty
                            // string the same — the next save mints a
                            // fresh UUID and persists it.
                            uuid: row.get::<_, Option<String>>(17)?.filter(|s| !s.is_empty()),
                        },
                    ))
                })?;

                let mut sanitized_shots = 0usize;
                for row in shot_rows {
                    let (section_id, mut shot) = row?;
                    // Neutralize geometry corrupt-on-disk at the load boundary: a
                    // single non-finite (NaN/inf) value would otherwise propagate
                    // through the offset cascade and break the whole survey. The
                    // degenerate-but-finite result is localized and visible. (Our own
                    // save can't write NaN — SQLite coerces it to NULL, which the
                    // NOT NULL columns reject — so this guards inf, hand-edited DBs,
                    // and any future nullable geometry column.)
                    if shot.sanitize_non_finite() {
                        sanitized_shots += 1;
                    }
                    if let Some(&idx) = section_by_id.get(&section_id) {
                        sections[idx].shots.push(shot);
                    }
                    // (rows for unknown sections are silently dropped —
                    // matches the legacy `WHERE section_id = ?` filter
                    // which never returned orphan rows in the first place.)
                }
                if sanitized_shots > 0 {
                    tracing::warn!(
                        sanitized_shots,
                        purpose = "block corruption at the source",
                        "Replaced non-finite (NaN/inf) shot geometry with finite defaults on load"
                    );
                }

                // Advance each section's next_shot_id past its loaded
                // shots so future appends don't collide with persisted IDs.
                for section in sections.iter_mut() {
                    section.next_shot_id =
                        section.id * FEATURE_ID_STRIDE + section.shots.len() as u32;
                }
            }
        }

        // Load station merges
        let mut station_merges: Vec<StationMerge> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT station_a_section, station_a_index, station_b_section, station_b_index, merged_at FROM station_merges",
            )?;
            let merge_rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;

            for merge_row in merge_rows {
                let (a_sec, a_idx, b_sec, b_idx, merged_at_str) = merge_row?;
                let merged_at =
                    chrono::DateTime::parse_from_rfc3339(&merged_at_str)?.with_timezone(&Utc);
                // Persisted positionally; load as transitional Index anchors,
                // which `reanchor_station_refs` (run on the load path) canonicalizes
                // to stable shot-UUID anchors once shots are present.
                station_merges.push(StationMerge {
                    station_a: StationKey::positional(a_sec, a_idx),
                    station_b: StationKey::positional(b_sec, b_idx),
                    merged_at,
                });
            }
        }

        // Load zero datums
        let mut zero_datums: Vec<ZeroDatum> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT section_id, station_index, latitude, longitude, depth_reference FROM zero_datums",
            )?;
            let zd_rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, f64>(4)?,
                ))
            })?;

            for zd_row in zd_rows {
                let (section_id, station_index, lat, lon, depth) = zd_row?;
                zero_datums.push(ZeroDatum {
                    station_ref: StationKey::positional(section_id, station_index),
                    gps: GpsCoordinates {
                        latitude: lat,
                        longitude: lon,
                        depth_reference: depth,
                    },
                });
            }
        }

        // Load section offsets
        let mut section_offsets: HashMap<u32, (f64, f64)> = HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT section_id, offset_x, offset_y FROM section_offsets")?;
            let offset_rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            })?;

            let mut sanitized_offsets = 0usize;
            for offset_row in offset_rows {
                let (section_id, mut offset_x, mut offset_y) = offset_row?;
                // Sanitize non-finite (NaN/inf) section offsets at the load boundary,
                // mirroring the per-shot sanitize above. section_offsets is the one
                // geometry field with no finite guard, and resolve_offsets never
                // adjusts a component ANCHOR's own offset, so a non-finite anchor/
                // floating offset from a corrupt or hand-edited DB would survive the
                // load and poison every world coordinate for that component. Coerce
                // to 0.0 so the subsequent re-solve can re-derive a finite position.
                if !offset_x.is_finite() || !offset_y.is_finite() {
                    if !offset_x.is_finite() {
                        offset_x = 0.0;
                    }
                    if !offset_y.is_finite() {
                        offset_y = 0.0;
                    }
                    sanitized_offsets += 1;
                }
                section_offsets.insert(section_id, (offset_x, offset_y));
            }
            if sanitized_offsets > 0 {
                tracing::warn!(
                    sanitized_offsets,
                    purpose = "block corruption at the source",
                    "Replaced non-finite (NaN/inf) section offset with 0.0 on load"
                );
            }
        }

        // Load station offsets
        let station_offsets = self.load_station_offsets()?;

        // Load loop closures
        let loop_closures = self.load_loop_closures()?;

        // Build UUID → current shot_id map so load_markers can resolve stable refs.
        // Shot IDs are positionally reconstructed on every load, so the UUID is the
        // only identity that survives shot deletions (see write.rs: shot_uuid).
        let uuid_to_shot_id: HashMap<String, u32> = sections
            .iter()
            .flat_map(|s| {
                s.shots
                    .iter()
                    .filter_map(|shot| shot.uuid.as_ref().map(|uuid| (uuid.clone(), shot.id)))
            })
            .collect();

        // Load markers
        let markers = self.load_markers(&uuid_to_shot_id)?;

        // Load imported file hashes
        let imported_file_hashes = self.load_imported_file_hashes()?;

        // Load imported section hashes
        let imported_section_hashes = self.load_imported_section_hashes()?;

        // Derive next_closure_id: use persisted value if available, otherwise fall back to max(existing) + 1.
        // Always ensure it's at least max(existing closures) + 1 for safety.
        let max_existing = loop_closures.iter().map(|lc| lc.id).max().unwrap_or(0);
        let next_closure_id = match persisted_next_closure_id {
            Some(persisted) => persisted.max(max_existing + 1),
            None => max_existing + 1,
        };

        // Derive next_marker_id: use persisted value if available, otherwise fall back to max(existing) + 1.
        let max_existing_marker = markers.iter().map(|m| m.id).max().unwrap_or(0);
        let next_marker_id = match persisted_next_marker_id {
            Some(persisted) => persisted.max(max_existing_marker + 1),
            None => max_existing_marker + 1,
        };

        // Load station comments
        let station_comments = self.load_station_comments()?;

        // Load collaboration state (None for pre-v5 projects or when off)
        let collab = self.load_collab_state()?;

        // Per-project validation thresholds (#57). The DB layer has no access to
        // the global config, so legacy projects (no row) fall back to the
        // hardcoded default here; the AppState open paths re-check for absence
        // and seed from the user's global config instead (see project_io.rs).
        let validation = self.load_project_validation()?.unwrap_or_default();

        // Pre-populate the section_id → index lookup so callers get
        // O(1) get_section without needing rebuild_section_index().
        let mut section_index_cache = HashMap::with_capacity(sections.len());
        for (i, s) in sections.iter().enumerate() {
            section_index_cache.insert(s.id, i);
        }
        // Same for merge adjacency — the first compute_station_coordinates
        // call hammers get_merged_section_ids and would otherwise pay the
        // O(N) linear fallback on every BFS step.
        let mut merge_adjacency_cache: HashMap<u32, Vec<u32>> = HashMap::new();
        for m in &station_merges {
            merge_adjacency_cache
                .entry(m.station_a.section_id)
                .or_default()
                .push(m.station_b.section_id);
            merge_adjacency_cache
                .entry(m.station_b.section_id)
                .or_default()
                .push(m.station_a.section_id);
        }
        // Pre-build the shot-UUID index too, so `resolve_key`'s `AfterShot` arm
        // is O(1) right after load (mirrors the two caches above).
        let mut shot_uuid_index = HashMap::new();
        for s in &sections {
            for (i, shot) in s.shots.iter().enumerate() {
                if let Some(u) = &shot.uuid {
                    // First occurrence wins, matching `add_section` /
                    // `rebuild_shot_uuid_index` / the `resolve_key` cold scan; a
                    // plain `.insert` would be last-write-wins and silently break
                    // that invariant if a duplicate UUID ever slipped through.
                    shot_uuid_index.entry((s.id, u.clone())).or_insert(i as u32);
                }
            }
        }
        Ok(Project {
            name,
            location,
            created_at,
            modified_at,
            sections,
            station_merges,
            zero_datums,
            loop_closures,
            markers,
            next_marker_id,
            section_offsets,
            station_offsets,
            station_comments,
            file_path: None,
            next_section_id,
            imported_file_hashes,
            imported_section_hashes,
            next_closure_id,
            collab,
            validation,
            cascade_deferred: false,
            section_index_cache,
            merge_adjacency_cache,
            shot_uuid_index,
        })
    }

    /// Load loop closures from database
    fn load_loop_closures(&self) -> Result<Vec<LoopClosure>> {
        let mut closures = Vec::new();

        // Check if table exists (for backwards compatibility with older files)
        let table_exists: bool = self
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='loop_closures'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !table_exists {
            return Ok(closures);
        }

        // Check which optional columns exist
        let has_closure_id: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('loop_closures') WHERE name='closure_id'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);
        let has_strategy: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('loop_closures') WHERE name='strategy'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);
        let has_is_aligned: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('loop_closures') WHERE name='is_aligned'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        // Build SELECT dynamically based on available columns
        let mut extra_cols = String::new();
        if has_closure_id {
            extra_cols.push_str(", closure_id");
        }
        if has_strategy {
            extra_cols.push_str(", strategy");
        }
        if has_is_aligned {
            extra_cols.push_str(", is_aligned");
        }
        let query = format!(
            "SELECT station_a_section, station_a_index, station_b_section, station_b_index, \
             path_length, misclosure_distance, error_percentage, horizontal_misclosure, vertical_misclosure{extra_cols} \
             FROM loop_closures"
        );

        let mut stmt = self.conn.prepare(&query)?;
        let rows = stmt.query_map([], |row| {
            let mut col = 9usize; // next column index after the 9 base columns
            let id = if has_closure_id {
                let v = row.get::<_, i64>(col).unwrap_or(0).max(0) as u64;
                col += 1;
                v
            } else {
                0
            };
            let strategy = if has_strategy {
                let s: String = row.get::<_, String>(col).unwrap_or_default();
                col += 1;
                ClosureStrategy::from_str_lossy(&s)
            } else {
                ClosureStrategy::default()
            };
            let is_aligned = if has_is_aligned {
                let v = row.get::<_, i64>(col).unwrap_or(0);
                v != 0
            } else {
                false
            };
            Ok(LoopClosure {
                id,
                station_a: StationKey::positional(row.get(0)?, row.get(1)?),
                station_b: StationKey::positional(row.get(2)?, row.get(3)?),
                path_length: row.get(4)?,
                misclosure_distance: row.get(5)?,
                error_percentage: row.get(6)?,
                horizontal_misclosure: row.get(7)?,
                vertical_misclosure: row.get(8)?,
                strategy,
                is_aligned,
            })
        })?;

        for row in rows {
            closures.push(row?);
        }

        Ok(closures)
    }

    /// Load survey markers from database
    fn load_markers(&self, uuid_to_shot_id: &HashMap<String, u32>) -> Result<Vec<Marker>> {
        let mut markers = Vec::new();

        let table_exists: bool = self
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='markers'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !table_exists {
            return Ok(markers);
        }

        let mut stmt = self.conn.prepare(
            "SELECT id, section_id, shot_id, shot_uuid, position, marker_type, color, text, direction \
             FROM markers",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, u32>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })?;

        for row in rows {
            let (
                id_i64,
                section_id,
                stored_shot_id,
                shot_uuid,
                position,
                marker_type_str,
                color,
                text,
                direction_str,
            ) = row?;
            // Prefer UUID-resolved shot_id (stable across shot deletions) over
            // the stored positional shot_id (legacy markers or files without UUIDs).
            let shot_id = shot_uuid
                .as_deref()
                .and_then(|uuid| uuid_to_shot_id.get(uuid).copied())
                .unwrap_or(stored_shot_id);
            let marker_type = match marker_type_str.as_str() {
                "Arrow" => MarkerType::Arrow,
                _ => MarkerType::Ndm,
            };
            let direction = direction_str.as_deref().and_then(|s| match s {
                "TowardStart" => Some(MarkerDirection::TowardStart),
                "TowardEnd" => Some(MarkerDirection::TowardEnd),
                _ => None,
            });
            markers.push(Marker {
                id: id_i64.max(0) as u64,
                section_id,
                shot_id,
                position: position as f32,
                marker_type,
                color,
                text,
                direction,
            });
        }

        Ok(markers)
    }

    /// Load station offsets from database
    fn load_station_offsets(&self) -> Result<HashMap<StationKey, (f64, f64, f64)>> {
        let mut offsets = HashMap::new();

        // Check if table exists (for backwards compatibility with older files)
        let table_exists: bool = self
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='station_offsets'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !table_exists {
            return Ok(offsets);
        }

        let mut stmt = self.conn.prepare(
            "SELECT section_id, station_index, offset_x, offset_y, offset_z FROM station_offsets",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, f64>(4)?,
            ))
        })?;

        for row in rows {
            let (section_id, station_index, offset_x, offset_y, offset_z) = row?;
            offsets.insert(
                StationKey::positional(section_id, station_index),
                (offset_x, offset_y, offset_z),
            );
        }

        Ok(offsets)
    }

    fn load_station_comments(&self) -> Result<HashMap<StationKey, String>> {
        let mut comments = HashMap::new();

        // Check if table exists (for backwards compatibility with older files)
        let table_exists: bool = self
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='station_comments'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !table_exists {
            return Ok(comments);
        }

        let mut stmt = self
            .conn
            .prepare("SELECT section_id, station_index, comment FROM station_comments")?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;

        for row in rows {
            let (section_id, station_index, comment) = row?;
            comments.insert(StationKey::positional(section_id, station_index), comment);
        }

        Ok(comments)
    }

    /// Load collaboration state from the single-row `collab_state` table.
    /// Returns `None` for pre-v5 projects (table absent) or when no row is present.
    fn load_collab_state(&self) -> Result<Option<crate::models::CollabState>> {
        let table_exists: bool = self
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='collab_state'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !table_exists {
            return Ok(None);
        }

        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT state_json FROM collab_state WHERE id = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok();

        match json {
            Some(j) => Ok(Some(
                serde_json::from_str(&j).context("Failed to deserialize collab state")?,
            )),
            None => Ok(None),
        }
    }

    /// Load the SpeleoDB hand-off link from the single-row `speleo_link` table
    /// (schema v9, #189). Returns `None` for pre-v9 projects (table absent) or
    /// when no row is present.
    ///
    /// Deliberately `pub` (unlike the private `load_collab_state`): it is called
    /// cross-module from `core/state/speleodb.rs` (stamp-on-pull readback) and
    /// the #190/#191 tests. `base_commit_sha` is read as `Option<String>` so an
    /// on-disk NULL stays `None` (never coerced to `""`).
    pub fn load_speleo_link(&self) -> Result<Option<crate::models::SpeleoLink>> {
        let table_exists: bool = self
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='speleo_link'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .unwrap_or(false);

        if !table_exists {
            return Ok(None);
        }

        let row = self.conn.query_row(
            "SELECT endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits, last_edit_at, last_verify_at
             FROM speleo_link WHERE id = 1",
            [],
            |row| {
                Ok(crate::models::SpeleoLink {
                    endpoint: row.get::<_, String>(0)?,
                    project_id: row.get::<_, String>(1)?,
                    project_name: row.get::<_, String>(2)?,
                    base_commit_sha: row.get::<_, Option<String>>(3)?,
                    has_unpushed_edits: row.get::<_, i64>(4)? != 0,
                    last_edit_at: row.get::<_, Option<String>>(5)?,
                    last_verify_at: row.get::<_, Option<String>>(6)?,
                })
            },
        );

        match row {
            Ok(link) => Ok(Some(link)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Load imported file hashes from database
    fn load_imported_file_hashes(&self) -> Result<Vec<String>> {
        let mut hashes = Vec::new();

        // Check if table exists (for backwards compatibility with older files)
        let table_exists: bool = self.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='imported_file_hashes'",
            [],
            |row| row.get::<_, i64>(0),
        ).map(|c| c > 0).unwrap_or(false);

        if !table_exists {
            return Ok(hashes);
        }

        let mut stmt = self.conn.prepare("SELECT hash FROM imported_file_hashes")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;

        for row in rows {
            hashes.push(row?);
        }

        Ok(hashes)
    }

    /// Load imported section hashes from database
    fn load_imported_section_hashes(&self) -> Result<Vec<String>> {
        let mut hashes = Vec::new();

        // Check if table exists (for backwards compatibility with older files)
        let table_exists: bool = self.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='imported_section_hashes'",
            [],
            |row| row.get::<_, i64>(0),
        ).map(|c| c > 0).unwrap_or(false);

        if !table_exists {
            return Ok(hashes);
        }

        let mut stmt = self
            .conn
            .prepare("SELECT hash FROM imported_section_hashes")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;

        for row in rows {
            hashes.push(row?);
        }

        Ok(hashes)
    }

    /// Load the per-project validation config (#57). Returns `None` for legacy
    /// projects that predate the feature — either no stored `validation` row, or
    /// a project_settings table that doesn't exist at all (open_existing() skips
    /// migrations and pre-#57 .stalx files predate the table). `None` lets the
    /// caller seed from the live global config (the "defaults for new projects")
    /// instead of the hardcoded ValidationConfig::default(); a present-but-
    /// unparseable row is treated as legacy too (`None`).
    pub fn load_project_validation(&self) -> Result<Option<crate::config::ValidationConfig>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM project_settings WHERE key = 'validation'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None);
        Ok(json.and_then(|s| serde_json::from_str(&s).ok()))
    }
}
