use anyhow::{Context, Result};
use chrono::{NaiveDate, Utc};
use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;

use crate::models::{
    AzimuthReference, ClosureStrategy, CorrectionSource, CreationStamp, GpsCoordinates,
    LoadedProject, LoopClosure, NorthReference, Project, Section, SectionData, SectionId, Shot,
    ShotKind, StationKey, StationMerge, SurveyDirection, UnrecognisedShotKind, ZeroDatum,
    FEATURE_ID_STRIDE, MAX_SECTION_ID,
};

use super::Database;

/// The most shots one section can number: its ids are `section_id * STRIDE +
/// position`, and the app caps a section one short of the stride
/// (`Project::add_shot`) so a free id always remains. A file holding more is
/// refused on load (A18(d), #576).
const MAX_SHOTS_PER_SECTION: usize = (FEATURE_ID_STRIDE - 1) as usize;

impl Database {
    /// Load a project from the database
    /// Load only the metadata needed to populate `ProjectMetadata` - no
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
                for (latitude, longitude, depth_reference) in rows.flatten() {
                    // [#576 B35(g), RV7b-4] The load's rule: a non-finite
                    // coordinate is dropped, and a FINITE one is kept as stored
                    // even when it is out of range (a depth reference beyond
                    // 11 km from a build before that bound), so a project the
                    // load opens with its datum still gets its card position and
                    // Global View marker. One more is left out HERE, because this
                    // read only positions a pin: a latitude outside [-90, 90],
                    // which no map can place (MapLibre throws on it).
                    let finite = latitude.is_finite()
                        && longitude.is_finite()
                        && depth_reference.is_finite();
                    if finite && (-90.0..=90.0).contains(&latitude) {
                        zero_datums.push(GpsCoordinates {
                            latitude,
                            longitude,
                            depth_reference,
                        });
                    }
                }
            }
        }

        let (gps_center, gps_bounds) = if zero_datums.is_empty() {
            (None, None)
        } else {
            // Positional on purpose, and not the frame rule
            // (`Project::frame_datum`, the first datum that RESOLVES): this reads
            // GPS columns out of the file for a project-list pin, with no sections
            // and no station refs loaded, so there is nothing here that could
            // resolve and nothing that reads LOCAL metres against it. The
            // divergence the frame rule exists to prevent - coordinates laid out
            // against one datum and interpreted against another - cannot arise
            // where no coordinate is involved.
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
        // is the bottleneck. Debug-level - fires for every recent
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
            is_remote,
            speleodb_project_id,
        })
    }

    pub fn load_project(&self) -> Result<Project> {
        self.load_project_with_report().map(|(project, _)| project)
    }

    /// [`Self::load_project`], plus what it could not read and filled with a
    /// default instead.
    ///
    /// [#576 B15] The per-project settings are the case that matters: a stored
    /// value that cannot be READ is not "not set". The project still loads, with
    /// the default in its place, and the report names the key - so the open path
    /// can refuse every save while it is unread, instead of writing the default
    /// over the stored value (`save_project` writes these settings back from the
    /// model).
    pub fn load_project_with_report(&self) -> Result<(Project, ProjectLoadReport)> {
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

        // [#576 NF-W18-1] Kept as stored, so a save writes it back byte for byte
        // (a `CreationStamp`, as a section's); an unreadable one refuses the open.
        let created_at = CreationStamp::read(created_at)?;
        let modified_at = chrono::DateTime::parse_from_rfc3339(&modified_at)?.with_timezone(&Utc);

        // What this load could not read, or reset because it could not use it.
        let mut report = ProjectLoadReport::default();

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

            // #24: `reverse_parity` is added by the schema migration, but a read-only open skips
            // that, so tolerate its absence the same way `quality` is tolerated. Both fall back
            // to a literal in the SELECT rather than a second pair of branches.
            let has_reverse_parity_column: bool = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('sections') WHERE name='reverse_parity'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map(|c| c > 0)
                .unwrap_or(false);

            let has_gauge_columns: bool = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('sections') WHERE name='gauge_shot_uuid'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map(|c| c > 0)
                .unwrap_or(false);

            // #438: same tolerance as `quality`/`reverse_parity` above - a
            // read-only open skips the migration, so the columns can be absent.
            let has_azimuth_columns: bool = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('sections') WHERE name='azimuth_north'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map(|c| c > 0)
                .unwrap_or(false);

            let quality_expr = if has_quality_column { "quality" } else { "1.0" };
            let parity_expr = if has_reverse_parity_column {
                "reverse_parity"
            } else {
                "0"
            };
            let (gauge_expr, gauge_nb_expr) = if has_gauge_columns {
                ("gauge_shot_uuid", "gauge_neighbour_uuid")
            } else {
                ("NULL", "NULL")
            };
            let azimuth_exprs = if has_azimuth_columns {
                "azimuth_north, azimuth_correction, azimuth_source, azimuth_epoch_year"
            } else {
                "NULL, NULL, NULL, NULL"
            };
            // #557: refuse BEFORE materializing. `ORDER BY id` puts over-bound
            // rows last, so the per-row check below would allocate the whole
            // table first - a hostile file with millions of rows makes the
            // refusal maximally expensive. `MAX(id)` is O(1) off the pk index,
            // and read as i64 it also gives a section id past even u32 the
            // same receipt-grade message instead of a bare type error.
            let max_id: Option<i64> =
                self.conn
                    .query_row("SELECT MAX(id) FROM sections", [], |row| row.get(0))?;
            if let Some(max_id) = max_id {
                anyhow::ensure!(
                    max_id <= i64::from(MAX_SECTION_ID),
                    "This file cannot be opened: it contains section id {max_id}, \
                     but section ids past {MAX_SECTION_ID} cannot hold shots. \
                     The file was written by an incompatible tool or is corrupted."
                );
            }

            let mut stmt = self.conn.prepare(&format!(
                "SELECT id, name, surveyor, date, color, direction, {quality_expr}, {parity_expr}, \
                 {gauge_expr}, {gauge_nb_expr}, {azimuth_exprs}, created_at FROM sections ORDER BY id"
            ))?;

            let mut section_rows = Vec::new();
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, f64>(6)?,
                    row.get::<_, i64>(7)? != 0,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<f64>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<i32>>(13)?,
                    // [#576 NF-W14BE-1] The creation stamp, in every schema
                    // since the first. Only TEXT this app can parse counts: a
                    // value of another type or shape loads as "never stamped"
                    // (the next save stamps it) rather than failing the open.
                    // [#576 RV14-2] Kept as the text it is, so the next save
                    // writes it back unchanged rather than the same moment in
                    // UTC form.
                    row.get_ref(14)?
                        .as_str()
                        .ok()
                        .and_then(CreationStamp::parse),
                ))
            })?;
            for row in rows {
                section_rows.push(row?);
            }

            // Build the empty sections first so we can address them by
            // ID when partitioning the bulk shot stream below.
            let mut section_by_id: HashMap<u32, usize> = HashMap::with_capacity(section_rows.len());
            for section_row in section_rows {
                let (
                    id,
                    name,
                    surveyor,
                    date_str,
                    color,
                    direction_str,
                    quality_raw,
                    reverse_parity,
                    gauge_shot_uuid,
                    gauge_neighbour_uuid,
                    azimuth_north,
                    azimuth_correction,
                    azimuth_source,
                    azimuth_epoch_year,
                    created_at,
                ) = section_row;
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

                // #438: the reference exists only if the file actually recorded a
                // north token. A NULL there is "nothing recorded" and stays
                // `None` (inherit the project default), which is the pre-#438
                // behaviour; `from_parts` re-imposes the type's invariants on
                // whatever the other three columns hold.
                let azimuth_reference = azimuth_north.map(|north| {
                    AzimuthReference::from_parts(
                        NorthReference::from_token(&north),
                        azimuth_correction.unwrap_or(0.0),
                        CorrectionSource::from_token(
                            azimuth_source.as_deref().unwrap_or("unknown"),
                            azimuth_epoch_year,
                        ),
                    )
                });

                // #557: refuse the whole open rather than construct a section
                // whose id cannot encode a shot id. `Section::new` computes
                // `id * FEATURE_ID_STRIDE` (debug panic, release wrap), the
                // shot encodes saturate every shot of such a section onto
                // u32::MAX, and the counter rebuild below would overflow -
                // #556 bounds what the app can MINT, this bounds what a file
                // (pre-#556, another tool, or hostile) can smuggle in.
                anyhow::ensure!(
                    id <= MAX_SECTION_ID,
                    "This file cannot be opened: it contains section id {id}, \
                     but section ids past {MAX_SECTION_ID} cannot hold shots. \
                     The file was written by an incompatible tool or is corrupted."
                );
                let mut section = Section::new(id, section_data);
                section.reverse_parity = reverse_parity;
                section.gauge_shot_uuid = gauge_shot_uuid;
                section.gauge_neighbour_uuid = gauge_neighbour_uuid;
                section.azimuth_reference = azimuth_reference;
                section.created_at = created_at;
                section_by_id.insert(id, sections.len());
                // Cannot overflow once ids are bounded above; saturating keeps
                // the counter rebuild total even if the bound ever moves.
                next_section_id = next_section_id.max(id.saturating_add(1));
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
                              comment, kind, marker_index, temperature, uuid
                       FROM shots ORDER BY section_id, sequence_order",
                )?;
                let shot_rows = shot_stmt.query_map([], |row| {
                    let section_id: u32 = row.get(0)?;
                    // Column 1 (`sequence_order`) only ORDERS the stream: the
                    // id is the shot's position in it, assigned below.
                    Ok((
                        section_id,
                        Shot {
                            id: 0,
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
                            // Out-of-u8 values (a corrupt or newer file can hold
                            // -1 or 256) must fail closed too. `unwrap_or(0)`
                            // sent them to `Normal` - swimmable - which routed
                            // straight past `from_u8`'s fail-closed rule, and
                            // the dialog would then send no edit because it
                            // assumes the in-memory value is already the safe
                            // one. Map every failed conversion to a discriminant
                            // `from_u8` rejects, so it lands on `Reference`.
                            // Deliberately total: ANY value this build cannot
                            // read becomes `Reference` rather than an error.
                            // `INTEGER` here is a type AFFINITY, not a
                            // constraint - the table is not STRICT, so a
                            // hand-edited or corrupt row can hold TEXT, REAL or
                            // a BLOB, and `row.get::<_, i64>` returns
                            // InvalidColumnType for those. Propagating that
                            // would make the whole project unopenable over one
                            // bad cell; out-of-i64 and wrong-type alike now land
                            // on a discriminant `from_u8` rejects, so the shot
                            // loads safely and reaches the resolution prompt.
                            kind: ShotKind::from_u8(
                                row.get::<_, i64>(14)
                                    .ok()
                                    .and_then(|v| u8::try_from(v).ok())
                                    .unwrap_or(u8::MAX),
                            ),
                            marker_index: row.get(15)?,
                            temperature: row.get(16)?,
                            // `uuid` column was added in schema v6; pre-v6
                            // rows return NULL here.  Treat NULL and empty
                            // string the same - the next save mints a
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
                    // save can't write NaN - SQLite coerces it to NULL, which the
                    // NOT NULL columns reject - so this guards inf, hand-edited DBs,
                    // and any future nullable geometry column.)
                    if shot.sanitize_non_finite() {
                        sanitized_shots += 1;
                    }
                    if let Some(&idx) = section_by_id.get(&section_id) {
                        let section = &mut sections[idx];
                        // The id is `section_id * STRIDE + POSITION` in the
                        // stored order (B35(f), A18(d), #576), not the stored
                        // rank. The app writes ranks 0..n, so for its own files
                        // the two agree; a foreign or hand-edited file can leave
                        // a gap (ranks 0, 2, 3 loaded as ids 0, 2, 3 with the
                        // counter on 3, so the next shot duplicated id 3), a rank
                        // past the window (into the next section's ids) or a
                        // negative one (`as u32` lands near `u32::MAX`). Station
                        // references are positional too, so numbering by
                        // position is what the rest of the load already assumes.
                        let position = section.shots.len();
                        // A position that does not fit the window would number
                        // this shot into the NEXT section's ids. The app caps a
                        // section at `FEATURE_ID_STRIDE - 1` shots, so refuse the
                        // open rather than load colliding ids, as #557 does for a
                        // section id past the encode bound.
                        anyhow::ensure!(
                            position < MAX_SHOTS_PER_SECTION,
                            "This file cannot be opened: section {section_id} holds more than \
                             {MAX_SHOTS_PER_SECTION} shots, the most one section can number."
                        );
                        // Cannot overflow: `section.id <= MAX_SECTION_ID`
                        // (refused above otherwise) and `position < STRIDE`.
                        shot.id = section.id * FEATURE_ID_STRIDE + position as u32;
                        section.shots.push(shot);
                    }
                    // (rows for unknown sections are silently dropped -
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
                report.repairs.shots = sanitized_shots;

                // Advance each section's next_shot_id past its loaded
                // shots so future appends don't collide with persisted IDs.
                for section in sections.iter_mut() {
                    section.next_shot_id = section
                        .id
                        .saturating_mul(FEATURE_ID_STRIDE)
                        .saturating_add(section.shots.len() as u32);
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
                // [#576 B35(g)] A NON-finite coordinate (+/-Inf: SQLite cannot
                // store NaN) would turn every coordinate this point positions into
                // NaN, so it is dropped here and counted for the open's notice. A
                // FINITE one is kept exactly as stored even when it is out of range
                // (latitude 95, a depth reference beyond 11 km from a build before
                // that bound): dropping it would move that part of the cave and
                // clamping would invent a position, so the integrity check flags it
                // (`invalid_zero_datum`) for the user to correct instead.
                if !(lat.is_finite() && lon.is_finite() && depth.is_finite()) {
                    tracing::warn!(
                        section_id,
                        station_index,
                        purpose = "block corruption at the source",
                        "Dropped a zero datum with a non-finite coordinate on load"
                    );
                    report.repairs.gps_points += 1;
                    continue;
                }
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
            report.repairs.section_offsets = sanitized_offsets;
        }

        // Load station offsets. [#576 B35(h)] Through the same finiteness guard as
        // shots and section offsets: they used to be inserted verbatim, and a
        // station offset is added straight into world coordinates.
        let mut station_offsets = self.load_station_offsets()?;
        let sanitized_station_offsets = Project::sanitize_station_offsets(&mut station_offsets);
        if sanitized_station_offsets > 0 {
            tracing::warn!(
                sanitized_station_offsets,
                purpose = "block corruption at the source",
                "Replaced non-finite (inf) station offset components with 0.0 on load"
            );
        }
        report.repairs.station_offsets = sanitized_station_offsets;

        // Load loop closures
        let loop_closures = self.load_loop_closures()?;

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

        // Load station comments
        let station_comments = self.load_station_comments()?;

        // Per-project validation thresholds (#57). The DB layer has no access to
        // the global config, so legacy projects (no row) fall back to the
        // hardcoded default here; the AppState open paths re-check for absence
        // and seed from the user's global config instead (see project_io.rs).
        let validation = report
            .setting("validation", self.load_project_validation())
            .unwrap_or_default();

        // Entrance elevation (#431). Unlike `validation`, a missing row has no
        // legacy-seeding fallback - `None` genuinely means "unknown".
        let surface_elevation =
            report.setting("surface_elevation", self.load_project_surface_elevation());
        let default_azimuth_reference = report.setting(
            "default_azimuth_reference",
            self.load_project_default_azimuth_reference(),
        );

        // Through the loader constructor (#576 A10): the lookup caches are
        // private to `Project`, which builds both of them up front so callers
        // get O(1) `get_section` / `resolve_key` right after load.
        let project = Project::from_loaded(LoadedProject {
            name,
            location,
            created_at,
            modified_at,
            sections,
            station_merges,
            zero_datums,
            loop_closures,
            section_offsets,
            station_offsets,
            station_comments,
            next_section_id,
            imported_file_hashes,
            imported_section_hashes,
            next_closure_id,
            validation,
            surface_elevation,
            default_azimuth_reference,
        });
        Ok((project, report))
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

    /// Load the SpeleoDB hand-off link from the single-row `speleo_link` table
    /// (schema v9, #189). Returns `None` for pre-v9 projects (table absent) or
    /// when no row is present.
    ///
    /// Deliberately `pub`: it is called
    /// cross-module from `core/state/speleodb.rs` (stamp-on-pull readback) and
    /// the #190/#191 tests. `base_commit_sha` is read as `Option<String>` so an
    /// on-disk NULL stays `None` (never coerced to `""`).
    pub fn load_speleo_link(&self) -> Result<Option<crate::models::SpeleoLink>> {
        // The error is PROPAGATED, not swallowed. `count(*)` over `sqlite_master`
        // returns a row for any readable database - including one that simply has no
        // `speleo_link` table (count 0, handled below) - so a failure here means the
        // FILE could not be read at all: not a database, a hot journal a read-only
        // connection cannot roll back (`SQLITE_READONLY_ROLLBACK`), a transient
        // `SQLITE_BUSY`. `unwrap_or(false)` rendered all of those as "no link row",
        // i.e. indistinguishable from a perfectly readable unmanaged `.stalx`, which
        // is what let the un-pushed-edits guard read an unreadable cached copy as
        // clean and pull the server copy over recoverable work. Callers that only
        // want "is this remote?" already treat `Err` as no (`matches!(.., Ok(Some(_)))`
        // / `.ok().flatten()`), so nothing else changes shape.
        let table_exists: bool = self.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='speleo_link'",
            [],
            |row| row.get::<_, i64>(0),
        )? > 0;

        if !table_exists {
            return Ok(None);
        }

        // The table can be NARROWER than today's schema, and this reader has to
        // cope: `open_for_metadata` runs NO migration (it is a read-only connection
        // opened for a handful of SELECTs), so a `.stalx` last written by a build at
        // schema v9/v10 still has the v9/v10 `speleo_link` shape while the crate is
        // at v13. Selecting `last_edit_at` (v10) / `last_verify_at` (v11)
        // unconditionally failed those files with `no such column`, and that error is
        // PROPAGATED (see above) - so a perfectly readable legacy cache copy was
        // reported as an unreadable FILE. That wedged it permanently: the guard
        // refused every pull / adopt / revision-restore for the project with a
        // factually wrong "has local edits that were never pushed", and the cache
        // scan reported "no local copy" for a project that has one.
        //
        // Probe the actual columns and substitute a literal NULL for the ones this
        // file predates - the same value a migrated-but-never-stamped row carries, so
        // the `Option<String>` reads below are unchanged. A genuinely unreadable FILE
        // still fails here (the pragma cannot be run either) and still propagates,
        // which is the fail-closed behaviour the guard depends on.
        let mut columns = self
            .conn
            .prepare("SELECT name FROM pragma_table_info('speleo_link')")?;
        let columns: std::collections::HashSet<String> = columns
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<_, _>>()?;
        // Named as expressions rather than as a dynamic column list so the SELECT
        // keeps a fixed arity and the indices below stay literal.
        let last_edit_at = if columns.contains("last_edit_at") {
            "last_edit_at"
        } else {
            "NULL"
        };
        let last_verify_at = if columns.contains("last_verify_at") {
            "last_verify_at"
        } else {
            "NULL"
        };
        let undo_mode = if columns.contains("undo_mode") {
            "undo_mode"
        } else {
            "NULL"
        };

        let row = self.conn.query_row(
            &format!(
                "SELECT endpoint, project_id, project_name, base_commit_sha, has_unpushed_edits, {last_edit_at}, {last_verify_at}, {undo_mode}
                 FROM speleo_link WHERE id = 1"
            ),
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
                    // [#303] An unrecognized string reads as `UndoMode::Unknown`, which
                    // resolves to the default rather than failing the load - the same
                    // tolerance the serde sentinel gives a hand-edited config.
                    undo_mode: row
                        .get::<_, Option<String>>(7)?
                        .as_deref()
                        .map(crate::models::UndoMode::from_db_str),
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
    /// projects that predate the feature - either no stored `validation` row, or
    /// a project_settings table that doesn't exist at all (open_existing() skips
    /// migrations and pre-#57 .stalx files predate the table). `None` lets the
    /// caller seed from the live global config (the "defaults for new projects")
    /// instead of the hardcoded ValidationConfig::default(); a present-but-
    /// unparseable row is treated as legacy too (`None`).
    /// Whether the v12->v13 migration (#27) left a one-shot chain
    /// classification pending. The migration seeds every legacy `is_reference`
    /// row as Jump, which is only a guess; the real verdict needs the project
    /// graph, so the flag defers it to the next load.
    pub fn is_kind_classification_pending(&self) -> bool {
        self.conn
            .query_row(
                "SELECT value FROM project_settings WHERE key = 'pending_kind_classification'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .unwrap_or(None)
            .is_some_and(|v| v == "1")
    }

    /// Clear the flag set by the v12->v13 migration once the classification has
    /// run, so it never runs a second time. Classification is import-time only
    /// by design (#27 q14): re-running it later could flip a kind under the user.
    ///
    /// Only correct once the reclassified kinds are durably on disk - see
    /// [`Self::persist_backfilled_shot_kinds`], which is the only caller that
    /// should ever pair a non-empty backfill with this.
    pub fn clear_pending_kind_classification(&self) -> Result<()> {
        self.conn.execute(
            "DELETE FROM project_settings WHERE key = 'pending_kind_classification'",
            [],
        )?;
        Ok(())
    }

    /// Shots whose stored `kind` this build does not recognise (#27).
    ///
    /// There is no `CHECK` constraint on the column, so a corrupt byte, a manual
    /// edit, or a file written by a newer build can hold anything. `from_u8`
    /// fails those closed to `Reference` so they cannot invent a swimmable leg,
    /// but silently substituting a kind would leave the user with a leg missing
    /// from gas and penetration and no idea why - so the load path collects them
    /// here and asks.
    ///
    /// Detected with a standalone query rather than during row decode: the loader
    /// builds `Shot`s through `from_u8`, which has already discarded the raw
    /// value by then, and threading a diagnostic out of `load_project` would
    /// change a signature every caller shares.
    ///
    /// `shot_id` is the in-memory id the frontend addresses shots by
    /// (`section_id * FEATURE_ID_STRIDE + position`, exactly as `load_project`
    /// numbers them), not the rowid, and `shot_number` is that position plus
    /// one. Position, not the stored `sequence_order`: a rank gap in a foreign
    /// file would otherwise name a different shot than the loader did.
    pub fn unrecognised_shot_kinds(&self) -> Result<Vec<UnrecognisedShotKind>> {
        // `kind` was added in v13; an older file legitimately has no such column.
        if self.conn.prepare("SELECT kind FROM shots LIMIT 0").is_err() {
            return Ok(Vec::new());
        }
        // `quote(...)`, not a raw read or a CAST: the column can legally hold any
        // storage class (see the loader's note), and reading a TEXT/BLOB cell as
        // i64 would error out the whole diagnostic - so the case most in need of
        // the prompt would be the one that never reached it. `CAST(blob AS
        // TEXT)` is no good either: it can yield invalid UTF-8. `quote` is total
        // and renders every class legibly (7, 'abc', X'DEADBEEF'), which is
        // exactly what the dialog wants to show.
        //
        // The position is numbered over EVERY shot of the section before the
        // filter applies (a window function runs after its own query's WHERE,
        // hence the subquery), the same `ORDER BY sequence_order` the loader
        // streams in.
        let mut stmt = self.conn.prepare(
            "SELECT section_id, position, raw_kind, name FROM (
                 SELECT s.section_id AS section_id,
                        ROW_NUMBER() OVER (
                            PARTITION BY s.section_id ORDER BY s.sequence_order
                        ) - 1 AS position,
                        quote(s.kind) AS raw_kind,
                        sec.name AS name,
                        s.kind NOT IN (0, 1, 2) AS unrecognised
                 FROM shots s
                 LEFT JOIN sections sec ON sec.id = s.section_id
             )
             WHERE unrecognised
             ORDER BY section_id, position",
        )?;
        let rows = stmt.query_map([], |row| {
            let section_id: SectionId = row.get(0)?;
            let position: u32 = row.get(1)?;
            Ok(UnrecognisedShotKind {
                section_id,
                // Saturating like the loader's shot encode: this diagnostic
                // runs its own query, so a shots row can name a section id no
                // sections row (and no #557 open-refusal) ever vetted.
                shot_id: section_id
                    .saturating_mul(FEATURE_ID_STRIDE)
                    .saturating_add(position),
                shot_number: position.saturating_add(1),
                section_name: row.get(3)?,
                raw_value: row.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Rewrite every unreadable `kind` to `Reference` on disk (#27).
    ///
    /// The loader already substitutes `Reference` in memory, so without this the
    /// two disagree: the app behaves correctly for the session, but the invalid
    /// discriminant survives, the project is never dirty (nothing was edited),
    /// and the same prompt reappears on every single open - while the dialog
    /// tells the user their choice is saved with the project.
    ///
    /// Writing the fail-closed value is not a decision taken from the user: it
    /// is the value already in force. Choosing anything else is still an
    /// ordinary undoable `SetShotKind` on top.
    pub fn repair_unrecognised_shot_kinds(&self) -> Result<usize> {
        if self.conn.prepare("SELECT kind FROM shots LIMIT 0").is_err() {
            return Ok(0);
        }
        // 2 == ShotKind::Reference, pinned like the migration's literals.
        let repaired = self
            .conn
            .execute("UPDATE shots SET kind = 2 WHERE kind NOT IN (0, 1, 2)", [])?;
        Ok(repaired)
    }

    /// Write the one-shot #27 backfill's verdicts straight to disk and clear the
    /// pending flag, both in one transaction.
    ///
    /// The backfill runs on load and mutates only the in-memory `Project`.
    /// Clearing the flag on its own (the first cut) destroyed the retry guard
    /// while the result was still volatile: open a migrated project, close it
    /// without editing, and the kinds were never written but the flag was gone -
    /// so every later open kept the migration's Jump seed and non-swimmable ties
    /// stayed counted as swum in gas, penetration and the ruler. Persisting here
    /// makes the backfill durable without needing a user-visible save; pairing
    /// it with the DELETE in one transaction means a crash mid-way leaves the
    /// flag set and the (idempotent) pass simply repeats.
    ///
    /// (The in-memory pass does still `touch()` the project, so `modified_at`
    /// moves on an open the user did not edit. That timestamp is not persisted
    /// by this write, and the frontend keeps `isDirty: false` after load, so no
    /// save is provoked.)
    ///
    /// Each verdict is `(section_id, position, kind)`, `position` being the
    /// shot's 0-based place in its section as `load_project` read it - NOT
    /// `Shot::id`, and not the stored `sequence_order`. `shots.id` is an
    /// auto-increment rowid the INSERT never sets, while the in-memory
    /// `Shot::id` is synthesised on load as `section_id * FEATURE_ID_STRIDE +
    /// position`; keying on `Shot::id` silently writes the wrong rows in the
    /// first section and matches nothing at all in the rest (the v7->v8
    /// migration converts between the same two spaces for `resolved_warnings`).
    /// The stored rank equals the position for every file this app writes, but
    /// a foreign file can have a gap, so each position is resolved to its row
    /// through the loader's own `ORDER BY sequence_order` (B35(f), #576).
    /// Keying on `uuid` would be wrong too: pre-v6 rows hold NULL on disk and
    /// `ensure_shot_uuids()` mints those in memory only, so it would persist
    /// against a transient identity.
    pub fn persist_backfilled_shot_kinds(
        &self,
        kinds: &[(SectionId, u32, ShotKind)],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut by_section: HashMap<SectionId, Vec<(u32, ShotKind)>> = HashMap::new();
            for (section_id, position, kind) in kinds {
                by_section
                    .entry(*section_id)
                    .or_default()
                    .push((*position, *kind));
            }
            let mut rows_in_order = tx
                .prepare("SELECT rowid FROM shots WHERE section_id = ?1 ORDER BY sequence_order")?;
            let mut update = tx.prepare("UPDATE shots SET kind = ?1 WHERE rowid = ?2")?;
            for (section_id, verdicts) in &by_section {
                let rowids = rows_in_order
                    .query_map([section_id], |row| row.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<i64>>>()?;
                for (position, kind) in verdicts {
                    if let Some(rowid) = rowids.get(*position as usize) {
                        update.execute(params![i32::from(kind.as_u8()), rowid])?;
                    }
                }
            }
        }
        tx.execute(
            "DELETE FROM project_settings WHERE key = 'pending_kind_classification'",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The stored text of one `project_settings` key.
    ///
    /// [#576 B15] "Not set" and "could not be read" are different answers, and only
    /// the first may become a default: the save writes these settings back from the
    /// project, so a read error taken for "not set" replaced the stored value with
    /// the default on the next save. A missing key - or no table at all, in a file
    /// written before it existed and opened read-only, which does not migrate - is
    /// "not set". Every other failure is an error. A value of the wrong SQLite type
    /// is unparseable, like a malformed one: warned about and read as not set.
    fn project_setting_text(&self, key: &str) -> Result<Option<String>> {
        let has_table: i64 = self.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='project_settings'",
            [],
            |row| row.get(0),
        )?;
        if has_table == 0 {
            return Ok(None);
        }
        let value = self
            .conn
            .query_row(
                "SELECT value FROM project_settings WHERE key = ?1",
                [key],
                |row| row.get::<_, rusqlite::types::Value>(0),
            )
            .optional()?;
        Ok(match value {
            None => None,
            Some(rusqlite::types::Value::Text(text)) => Some(text),
            Some(other) => {
                tracing::warn!(
                    key,
                    value_type = %other.data_type(),
                    "stored project setting is not text; reading it as not set"
                );
                None
            }
        })
    }

    pub fn load_project_validation(
        &self,
    ) -> Result<Option<crate::models::config::ValidationConfig>> {
        let json = self.project_setting_text("validation")?;
        Ok(json.and_then(|s| serde_json::from_str(&s).ok()))
    }

    /// Load the project-level entrance elevation (#431), same key/value table
    /// as `validation`. A missing key - either no stored row, or a
    /// `project_settings` table that doesn't exist at all (a project written
    /// before this feature) - reads as `None`, which is the correct "unknown"
    /// value for those files; no migration needed.
    pub fn load_project_surface_elevation(&self) -> Result<Option<f64>> {
        let text = self.project_setting_text("surface_elevation")?;
        Ok(text.and_then(|s| {
            // `"NaN"`, `"inf"` and `1e309` all parse happily; a corrupted or
            // hand-edited `.stalx` (same untrusted provenance as a `.tml`) can
            // carry one, and a non-finite value would defeat the no-clobber
            // comparison on import (every NaN compare is false) and land
            // verbatim in a TML export. Same idiom as
            // `parsers::tml::firstStartAbsoluteElevation`.
            let parsed = s.parse::<f64>().ok().filter(|v: &f64| v.is_finite());
            if parsed.is_none() {
                tracing::warn!(
                    raw = %s,
                    "stored surface_elevation is present but unparseable or non-finite; reading as unknown"
                );
            }
            parsed
        }))
    }

    /// Load the project-level default north reference (#438). A missing key -
    /// no row, or no `project_settings` table at all - reads as `None`, which
    /// resolves to a zero correction: exactly what every pre-#438 file means.
    /// A row that does not parse is treated the same way and warned about,
    /// rather than defaulting to some other rotation.
    pub fn load_project_default_azimuth_reference(&self) -> Result<Option<AzimuthReference>> {
        let json = self.project_setting_text("default_azimuth_reference")?;
        Ok(json.and_then(|s| {
            let parsed = serde_json::from_str::<AzimuthReference>(&s).ok().map(|r| {
                // Re-impose the type's invariants on a hand-edited row.
                AzimuthReference::from_parts(r.north, r.correction, r.source)
            });
            if parsed.is_none() {
                tracing::warn!(
                    raw = %s,
                    "stored default_azimuth_reference is present but unparseable; reading as none recorded"
                );
            }
            parsed
        }))
    }
}

/// What [`Database::load_project_with_report`] could not read and filled with a
/// default instead.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProjectLoadReport {
    /// [#576 B15] The `project_settings` keys (`validation`, `surface_elevation`,
    /// `default_azimuth_reference`) whose stored value exists but could not be
    /// read. Absent and merely unparseable values are not listed: those are "not
    /// set", as before.
    pub unreadable_settings: Vec<&'static str>,
    /// [#576 B35(h)] The values this load reset or dropped because it could not
    /// use them, per kind.
    pub repairs: LoadRepairs,
}

/// [#576 B35(h)] How many stored values a load reset or dropped because it could
/// not use them (non-finite numbers from a damaged or hand-edited file), per
/// kind. The next save writes the reset values, so the open reports them to the
/// user once instead of only logging them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct LoadRepairs {
    /// Shots with a non-finite measurement, reset to finite defaults.
    pub shots: usize,
    /// Section offsets with a non-finite component, reset to 0.
    pub section_offsets: usize,
    /// Station offsets with a non-finite component, reset to 0.
    pub station_offsets: usize,
    /// GPS tie points (zero datums) with a non-finite coordinate, dropped.
    pub gps_points: usize,
}

impl LoadRepairs {
    /// True when the load reset or dropped nothing.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl ProjectLoadReport {
    /// The setting `key` as read, or `None` - recorded as unreadable - when the
    /// read failed.
    fn setting<T>(&mut self, key: &'static str, read: Result<Option<T>>) -> Option<T> {
        match read {
            Ok(value) => value,
            Err(e) => {
                tracing::warn!(
                    key,
                    error = %e,
                    purpose = "keep stored project settings out of reach of a save",
                    "A stored project setting could not be read; using the default for this \
                     session"
                );
                self.unreadable_settings.push(key);
                None
            }
        }
    }
}
