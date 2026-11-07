//! Column-adding schema migrations and their idempotence.
//!
//! Lines 3550-3739 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;

// ============================================================================
// Schema migration tests
// ============================================================================

#[test]
fn test_migrate_schema_adds_direction_column() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("migrate_direction.stalx");

    // Create a minimal DB without the direction column
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        r"
        CREATE TABLE project (id INTEGER PRIMARY KEY, name TEXT, location TEXT, created_at TEXT, modified_at TEXT, schema_version INTEGER);
        CREATE TABLE sections (id INTEGER PRIMARY KEY, name TEXT, surveyor TEXT, date TEXT, color TEXT NOT NULL, created_at TEXT NOT NULL);
        CREATE TABLE shots (id INTEGER PRIMARY KEY, section_id INTEGER NOT NULL, sequence_order INTEGER NOT NULL, azimuth_in REAL, azimuth_out REAL, length REAL, depth_in REAL, depth_out REAL, pitch_in REAL, pitch_out REAL, left_distance REAL, right_distance REAL, up_distance REAL, down_distance REAL, comment TEXT, is_virtual INTEGER DEFAULT 0, marker_index INTEGER, temperature REAL);
        CREATE TABLE station_merges (id INTEGER PRIMARY KEY, station_a_section INTEGER, station_a_index INTEGER, station_b_section INTEGER, station_b_index INTEGER, merged_at TEXT);
        CREATE TABLE zero_datums (id INTEGER PRIMARY KEY, section_id INTEGER, station_index INTEGER, latitude REAL, longitude REAL, depth_reference REAL);
        CREATE TABLE section_offsets (section_id INTEGER PRIMARY KEY, offset_x REAL, offset_y REAL);
        CREATE TABLE loop_closures (id INTEGER PRIMARY KEY, station_a_section INTEGER, station_a_index INTEGER, station_b_section INTEGER, station_b_index INTEGER, path_length REAL, misclosure_distance REAL, error_percentage REAL, horizontal_misclosure REAL, vertical_misclosure REAL);
        INSERT INTO project VALUES (1, 'Test', NULL, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 2);
        INSERT INTO sections VALUES (0, 'Sec1', NULL, NULL, '#ff0000', '2024-01-01T00:00:00Z');
        ",
    ).unwrap();
    drop(conn);

    // Open with Database::open_existing which runs migrations
    let db = Database::open_existing(&db_path).unwrap();
    let project = db.load_project().unwrap();
    assert_eq!(project.sections.len(), 1);
    // Direction should default to 'in' after migration
    assert_eq!(project.sections[0].direction.to_string(), "in");
}

#[test]
fn test_migrate_schema_adds_quality_column() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("migrate_quality.stalx");

    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        r"
        CREATE TABLE project (id INTEGER PRIMARY KEY, name TEXT, location TEXT, created_at TEXT, modified_at TEXT, schema_version INTEGER);
        CREATE TABLE sections (id INTEGER PRIMARY KEY, name TEXT, surveyor TEXT, date TEXT, color TEXT NOT NULL, direction TEXT DEFAULT 'in', created_at TEXT NOT NULL);
        CREATE TABLE shots (id INTEGER PRIMARY KEY, section_id INTEGER NOT NULL, sequence_order INTEGER NOT NULL, azimuth_in REAL, azimuth_out REAL, length REAL, depth_in REAL, depth_out REAL, pitch_in REAL, pitch_out REAL, left_distance REAL, right_distance REAL, up_distance REAL, down_distance REAL, comment TEXT, is_virtual INTEGER DEFAULT 0, marker_index INTEGER, temperature REAL);
        CREATE TABLE station_merges (id INTEGER PRIMARY KEY, station_a_section INTEGER, station_a_index INTEGER, station_b_section INTEGER, station_b_index INTEGER, merged_at TEXT);
        CREATE TABLE zero_datums (id INTEGER PRIMARY KEY, section_id INTEGER, station_index INTEGER, latitude REAL, longitude REAL, depth_reference REAL);
        CREATE TABLE section_offsets (section_id INTEGER PRIMARY KEY, offset_x REAL, offset_y REAL);
        CREATE TABLE loop_closures (id INTEGER PRIMARY KEY, station_a_section INTEGER, station_a_index INTEGER, station_b_section INTEGER, station_b_index INTEGER, path_length REAL, misclosure_distance REAL, error_percentage REAL, horizontal_misclosure REAL, vertical_misclosure REAL);
        INSERT INTO project VALUES (1, 'QualityTest', NULL, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 2);
        INSERT INTO sections VALUES (0, 'Sec1', NULL, NULL, '#00ff00', 'out', '2024-01-01T00:00:00Z');
        ",
    ).unwrap();
    drop(conn);

    let db = Database::open_existing(&db_path).unwrap();
    let project = db.load_project().unwrap();
    // Quality should default to 1.0 after migration
    assert!((project.sections[0].quality - 1.0).abs() < f64::EPSILON);
}

#[test]
fn test_migrate_schema_adds_closure_columns() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("migrate_closure.stalx");

    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        r"
        CREATE TABLE project (id INTEGER PRIMARY KEY, name TEXT, location TEXT, created_at TEXT, modified_at TEXT, schema_version INTEGER);
        CREATE TABLE sections (id INTEGER PRIMARY KEY, name TEXT, surveyor TEXT, date TEXT, color TEXT NOT NULL, direction TEXT DEFAULT 'in', quality REAL DEFAULT 1.0, created_at TEXT NOT NULL);
        CREATE TABLE shots (id INTEGER PRIMARY KEY, section_id INTEGER NOT NULL, sequence_order INTEGER NOT NULL, azimuth_in REAL, azimuth_out REAL, length REAL, depth_in REAL, depth_out REAL, pitch_in REAL, pitch_out REAL, left_distance REAL, right_distance REAL, up_distance REAL, down_distance REAL, comment TEXT, is_virtual INTEGER DEFAULT 0, marker_index INTEGER, temperature REAL);
        CREATE TABLE station_merges (id INTEGER PRIMARY KEY, station_a_section INTEGER, station_a_index INTEGER, station_b_section INTEGER, station_b_index INTEGER, merged_at TEXT);
        CREATE TABLE zero_datums (id INTEGER PRIMARY KEY, section_id INTEGER, station_index INTEGER, latitude REAL, longitude REAL, depth_reference REAL);
        CREATE TABLE section_offsets (section_id INTEGER PRIMARY KEY, offset_x REAL, offset_y REAL);
        CREATE TABLE loop_closures (id INTEGER PRIMARY KEY, station_a_section INTEGER, station_a_index INTEGER, station_b_section INTEGER, station_b_index INTEGER, path_length REAL, misclosure_distance REAL, error_percentage REAL, horizontal_misclosure REAL, vertical_misclosure REAL);
        INSERT INTO project VALUES (1, 'ClosureTest', NULL, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 2);
        INSERT INTO sections VALUES (0, 'Sec1', NULL, NULL, '#ff0000', 'in', 1.0, '2024-01-01T00:00:00Z');
        INSERT INTO loop_closures VALUES (1, 0, 0, 0, 1, 100.0, 0.5, 0.5, 0.3, 0.2);
        ",
    ).unwrap();
    drop(conn);

    let db = Database::open_existing(&db_path).unwrap();
    // Verify migration added closure_id, strategy, and is_aligned columns
    let has_closure_id: bool = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('loop_closures') WHERE name='closure_id'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap();
    let has_strategy: bool = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('loop_closures') WHERE name='strategy'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap();
    let has_is_aligned: bool = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('loop_closures') WHERE name='is_aligned'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap();

    assert!(has_closure_id, "migration should add closure_id column");
    assert!(has_strategy, "migration should add strategy column");
    assert!(has_is_aligned, "migration should add is_aligned column");
}

/// B23 (#576): `save_project` writes rows; it never changes the schema.
///
/// The save transaction used to carry its own copy of the three `loop_closures`
/// column migrations above (`closure_id`, `strategy`, `is_aligned`) and ran them
/// on every save. Every open path already runs the real chain (`init_schema` for
/// a fresh temp file, `ensure_schema_version` for an existing one), so the copy
/// could only fire on a connection whose table lacked the columns under a
/// current stamp - and the SpeleoDB in-place writers save on the LIVE file, so
/// there it rewrote the user's schema in the middle of a save.
///
/// The forged shape is exactly that: the pre-`closure_id` table on a database
/// stamped current. A project with no closures saves fine without the columns,
/// so the assertion isolates the DDL side effect from the INSERT.
#[test]
fn save_project_never_alters_the_loop_closures_schema() {
    let mut db = Database::in_memory().unwrap();
    db.conn
        .execute_batch(
            "DROP TABLE loop_closures;
             CREATE TABLE loop_closures (id INTEGER PRIMARY KEY, station_a_section INTEGER, \
             station_a_index INTEGER, station_b_section INTEGER, station_b_index INTEGER, \
             path_length REAL, misclosure_distance REAL, error_percentage REAL, \
             horizontal_misclosure REAL, vertical_misclosure REAL);",
        )
        .unwrap();
    let loop_closures_ddl = |db: &Database| -> String {
        db.conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'loop_closures'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    let before = loop_closures_ddl(&db);

    db.save_project(&Project::new("CaveA".to_string(), None))
        .unwrap();

    let after = loop_closures_ddl(&db);
    println!("loop_closures DDL before save: {before}");
    println!("loop_closures DDL after save:  {after}");
    assert_eq!(
        after, before,
        "save_project ran DDL against the open database's loop_closures table"
    );
}

#[test]
fn test_migrate_schema_idempotent() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("idempotent.stalx");

    // Create and save a full project
    let mut db = Database::open(&db_path).unwrap();
    let project = Project::new("Idempotent".to_string(), None);
    db.save_project(&project).unwrap();
    drop(db);

    // Open it twice - migration runs both times, second should be a no-op
    let db1 = Database::open_existing(&db_path).unwrap();
    let p1 = db1.load_project().unwrap();
    drop(db1);

    let db2 = Database::open_existing(&db_path).unwrap();
    let p2 = db2.load_project().unwrap();
    drop(db2);

    assert_eq!(p1.name, p2.name);
    assert_eq!(p1.sections.len(), p2.sections.len());
}
