//! [#576 B1-D2] The migration chain reaches today's schema from the oldest file
//! this build opens: a file with v0.0.3's own DDL is migrated and compared,
//! table by table - columns, indexes, foreign keys and triggers - against a
//! freshly created file.
//!
//! Moved out of `tests.rs` (#576 E8: the monolith is frozen); indexes, foreign
//! keys and triggers joined the comparison with #576 RV3-R-4.

use std::collections::BTreeMap;

use rusqlite::Connection;
use tempfile::TempDir;

use super::{Database, SCHEMA_VERSION};

/// [#576 B1-D2] The `init_schema` DDL of v0.0.3, the first public release and
/// the oldest schema this build opens (`OLDEST_SUPPORTED_SCHEMA_VERSION` = 2).
/// Copied verbatim from `git show v0.0.3:src-tauri/src/persistence/sqlite.rs`
/// (`fn init_schema`). Never edit it to make a test pass: it is the shape real
/// v0.0.3 files have on disk, and the migration chain has to reach today's
/// schema from exactly this.
const V0_0_3_SCHEMA_2_DDL: &str = r"
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
                is_virtual INTEGER NOT NULL DEFAULT 0,
                marker_index INTEGER,
                temperature REAL,
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

            -- Undo/redo stack
            CREATE TABLE IF NOT EXISTS undo_stack (
                id INTEGER PRIMARY KEY,
                sequence INTEGER NOT NULL,
                command_json TEXT NOT NULL,
                undo_data_json TEXT NOT NULL,
                executed_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS redo_stack (
                id INTEGER PRIMARY KEY,
                sequence INTEGER NOT NULL,
                command_json TEXT NOT NULL,
                undo_data_json TEXT NOT NULL,
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

            -- Resolved warnings (user acknowledged)
            CREATE TABLE IF NOT EXISTS resolved_warnings (
                id INTEGER PRIMARY KEY,
                section_id INTEGER NOT NULL,
                shot_id INTEGER NOT NULL,
                warning_code TEXT NOT NULL,
                resolved_at TEXT NOT NULL,
                resolved_comment TEXT,
                UNIQUE(section_id, shot_id, warning_code),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );

            -- Imported file hashes (for duplicate detection)
            CREATE TABLE IF NOT EXISTS imported_file_hashes (
                id INTEGER PRIMARY KEY,
                hash TEXT NOT NULL UNIQUE
            );

            -- Indexes
            CREATE INDEX IF NOT EXISTS idx_shots_section ON shots(section_id);
            CREATE INDEX IF NOT EXISTS idx_undo_sequence ON undo_stack(sequence);
";

/// `object -> (member -> shape)` for one schema: everything about it the
/// migration chain has to reproduce, keyed by name so the comparison does not
/// depend on ORDER.
type SchemaShape = BTreeMap<String, BTreeMap<String, String>>;

/// The rows of a single-text-column query, in order.
fn strings(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> Vec<String> {
    let mut statement = conn.prepare(sql).unwrap();
    let rows = statement.query_map(params, |r| r.get(0)).unwrap();
    rows.map(|r| r.unwrap()).collect()
}

/// Every user table of `conn` with (#576 RV3-R-4: not only its columns):
/// - `table T`: its columns as `pragma_table_info` reports them - `(declared
///   type, notnull, default, pk position)`, keyed by name, because an `ALTER
///   TABLE ADD COLUMN` appends while `init_schema` declares a column where it
///   reads best;
/// - `indexes on T`: every index, explicit or behind a UNIQUE / PRIMARY KEY
///   constraint, with its uniqueness, origin, partial flag and columns. A
///   constraint's automatic index is keyed by its columns, not its
///   SQLite-derived name;
/// - `foreign keys of T`: every reference with its ON UPDATE / ON DELETE;
/// - `triggers on T`: every trigger, with its full SQL.
fn schema_shape(conn: &Connection) -> SchemaShape {
    let tables = strings(
        conn,
        "SELECT name FROM sqlite_master WHERE type = 'table' \
         AND name NOT LIKE 'sqlite_%' ORDER BY name",
        [],
    );
    let mut shape = SchemaShape::new();
    for table in tables {
        let columns = conn
            .prepare("SELECT name, type, \"notnull\", dflt_value, pk FROM pragma_table_info(?1)")
            .unwrap()
            .query_map([&table], |r| {
                let name: String = r.get(0)?;
                let decl: String = r.get(1)?;
                let notnull: i64 = r.get(2)?;
                let default: Option<String> = r.get(3)?;
                let pk: i64 = r.get(4)?;
                Ok((
                    name,
                    format!("{decl} notnull={notnull} default={default:?} pk={pk}"),
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        shape.insert(format!("table {table}"), columns);

        let indexes = conn
            .prepare("SELECT name, \"unique\", origin, partial FROM pragma_index_list(?1)")
            .unwrap()
            .query_map([&table], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .map(|(name, unique, origin, partial)| {
                let columns = strings(
                    conn,
                    "SELECT coalesce(name, '<expr>') FROM pragma_index_info(?1) ORDER BY seqno",
                    [&name],
                )
                .join(",");
                let key = if name.starts_with("sqlite_autoindex_") {
                    format!("automatic({columns})")
                } else {
                    name
                };
                (
                    key,
                    format!("unique={unique} origin={origin} partial={partial} on ({columns})"),
                )
            })
            .collect();
        shape.insert(format!("indexes on {table}"), indexes);

        let foreign_keys = conn
            .prepare(
                "SELECT \"from\", \"table\", \"to\", on_update, on_delete \
                 FROM pragma_foreign_key_list(?1)",
            )
            .unwrap()
            .query_map([&table], |r| {
                let from: String = r.get(0)?;
                let parent: String = r.get(1)?;
                let to: Option<String> = r.get(2)?;
                let on_update: String = r.get(3)?;
                let on_delete: String = r.get(4)?;
                Ok((
                    format!("{from} -> {parent}({to:?})"),
                    format!("on update {on_update}, on delete {on_delete}"),
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        shape.insert(format!("foreign keys of {table}"), foreign_keys);

        let triggers = conn
            .prepare("SELECT name, sql FROM sqlite_master WHERE type = 'trigger' AND tbl_name = ?1")
            .unwrap()
            .query_map([&table], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        shape.insert(format!("triggers on {table}"), triggers);
    }
    shape
}

/// [#576 B1-D2] THE guard for "a column must be declared in five places": a file
/// at the oldest supported schema (v0.0.3's DDL, with a little synthetic data in
/// v0.0.3's own column shapes) is opened, and once the migration chain has run,
/// every table must have exactly the columns, indexes, foreign keys and triggers
/// a freshly created file has - and the migrated project must save in place and
/// load back unchanged.
///
/// A column added to `init_schema` without the matching `migrate_schema` step (or
/// a table only `init_schema` creates) shows up here as a difference, long before
/// a user's first save on an upgraded file fails with `no such column`. So does
/// an index a table rebuild drops (#576 RV3-R-4: the v2 -> v3 undo-table rebuild
/// lost `idx_undo_sequence`).
#[test]
fn migrate_schema_from_the_oldest_supported_file_matches_a_fresh_file() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("v0_0_3.stalx");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(V0_0_3_SCHEMA_2_DDL).unwrap();
        // Rows in v0.0.3's column shapes (its `save_project` INSERTs), stamped
        // the way v0.0.3 stamped every file it wrote.
        conn.execute_batch(&format!(
            "INSERT INTO project (id, name, location, created_at, modified_at, schema_version, \
                 next_closure_id)
             VALUES (1, 'CaveA', 'RegionA', '2026-02-06T10:00:00+00:00', \
                 '2026-02-06T11:00:00+00:00', 2, 1);
             INSERT INTO sections (id, name, surveyor, date, color, direction, quality, created_at)
             VALUES (1, 'SectionA', 'SurveyorA', '2026-02-01', '#ff0000', 'in', 1.0, \
                 '2026-02-06T10:00:00+00:00');
             INSERT INTO shots (id, section_id, sequence_order, azimuth_in, azimuth_out, length, \
                 depth_in, depth_out, pitch_in, pitch_out, left_distance, right_distance, \
                 up_distance, down_distance, comment, is_virtual, marker_index, temperature)
             VALUES
                 (1, 1, 0, 10.0, 10.0, 5.0, 2.0, 3.0, NULL, NULL, 1.0, 1.5, 0.5, 0.5, \
                     'ShotA1', 0, NULL, 24.5),
                 (2, 1, 1, 95.0, 95.0, 7.5, 3.0, 4.5, NULL, NULL, NULL, NULL, NULL, NULL, \
                     NULL, 0, 3, NULL);
             INSERT INTO zero_datums (id, section_id, station_index, latitude, longitude, \
                 depth_reference)
             VALUES (1, 1, 0, 20.5, -87.25, 0.0);
             INSERT INTO section_offsets (section_id, offset_x, offset_y) VALUES (1, 0.0, 0.0);
             INSERT INTO resolved_warnings (section_id, shot_id, warning_code, resolved_at, \
                 resolved_comment)
             VALUES (1, {shot_id}, 'SHORT_SHOT', '2026-02-06T10:30:00+00:00', 'checked');
             INSERT INTO display_settings (key, value) VALUES ('showVirtualShots', 'false');
             INSERT INTO undo_stack (sequence, command_json, undo_data_json, executed_at)
             VALUES (1, '{{\"type\":\"close_project\"}}', '{{}}', '2026-02-06T10:40:00+00:00');
             PRAGMA user_version = 2;",
            shot_id = crate::models::FEATURE_ID_STRIDE
        ))
        .unwrap();
    }

    let mut db = Database::open_existing(&path).expect("a v0.0.3 file must open");
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);

    let migrated = schema_shape(&db.conn);
    let fresh = schema_shape(&Database::in_memory().unwrap().conn);
    let mut differences = Vec::new();
    for (object, fresh_members) in &fresh {
        let Some(migrated_members) = migrated.get(object) else {
            differences.push(format!("{object}: missing from the migrated file"));
            continue;
        };
        for (member, shape) in fresh_members {
            match migrated_members.get(member) {
                None => differences.push(format!("{object}: {member} missing ({shape})")),
                Some(got) if got != shape => differences.push(format!(
                    "{object}: {member} migrated `{got}`, fresh `{shape}`"
                )),
                Some(_) => {}
            }
        }
        for member in migrated_members.keys() {
            if !fresh_members.contains_key(member) {
                differences.push(format!("{object}: {member} only in the migrated file"));
            }
        }
    }
    for object in migrated.keys() {
        if !fresh.contains_key(object) {
            differences.push(format!("{object}: only in the migrated file"));
        }
    }
    let count = |kind: &str| -> usize {
        fresh
            .iter()
            .filter(|(object, _)| object.starts_with(kind))
            .map(|(_, members)| members.len())
            .sum()
    };
    println!(
        "compared: {} tables, {} columns, {} indexes, {} foreign keys, {} triggers; \
         differences: {}",
        fresh.keys().filter(|o| o.starts_with("table ")).count(),
        count("table "),
        count("indexes on "),
        count("foreign keys of "),
        count("triggers on "),
        differences.len()
    );
    for difference in &differences {
        println!("  {difference}");
    }
    assert!(
        differences.is_empty(),
        "a file migrated from the oldest supported schema must have exactly the schema a \
         fresh file has:\n  {}",
        differences.join("\n  ")
    );

    // The data came through the chain...
    let project = db.load_project().unwrap();
    assert_eq!(project.name, "CaveA");
    assert_eq!(project.sections.len(), 1);
    let section = &project.sections[0];
    assert_eq!(section.name.as_deref(), Some("SectionA"));
    assert_eq!(section.surveyor.as_deref(), Some("SurveyorA"));
    assert_eq!(section.shots.len(), 2);
    assert!((section.shots[1].length - 7.5).abs() < 1e-9);
    assert_eq!(section.shots[0].comment.as_deref(), Some("ShotA1"));
    assert_eq!(project.zero_datums.len(), 1);
    assert!((project.zero_datums[0].gps.latitude - 20.5).abs() < 1e-9);

    // ...and the migrated connection can be written IN PLACE (the in-place
    // writers - push bookkeeping, link updates - use exactly this handle), then
    // read back to the same project, here and through a fresh open.
    db.save_project(&project)
        .expect("a migrated file must accept an in-place save");
    let saved = db.load_project().unwrap();
    assert_eq!(saved.sections[0].shots.len(), 2);
    assert!((saved.sections[0].shots[0].length - 5.0).abs() < 1e-9);
    assert_eq!(saved.sections[0].shots[1].marker_index, Some(3));
    drop(db);
    let reopened = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();
    assert_eq!(reopened.name, "CaveA");
    assert_eq!(reopened.sections[0].name.as_deref(), Some("SectionA"));
    assert_eq!(reopened.sections[0].shots.len(), 2);
    assert_eq!(reopened.zero_datums.len(), 1);
}

/// [#576 RV3-R-4] A file an EARLIER build already upgraded from v2 has no
/// `idx_undo_sequence` (that build's v2 -> v3 rebuild dropped it), and the v3
/// arm will never run on it again: its next upgrade puts the index back.
#[test]
fn an_upgrade_restores_the_undo_sequence_index_an_earlier_build_dropped() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("CaveA.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&crate::models::Project::new("CaveA".to_string(), None))
            .unwrap();
        db.conn
            .execute_batch(&format!(
                "DROP INDEX idx_undo_sequence; PRAGMA user_version = {};",
                SCHEMA_VERSION - 1
            ))
            .unwrap();
    }
    let has_index = |conn: &Connection| -> bool {
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'index' \
             AND name = 'idx_undo_sequence' AND tbl_name = 'undo_stack'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    };

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);
    println!(
        "idx_undo_sequence after the upgrade: {}",
        has_index(&db.conn)
    );
    assert!(has_index(&db.conn));
}
