//! The raw-table browser, after a save with two dangling station offsets.
//!
//! Lines 4723-4874 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;

#[test]
fn two_dangling_station_offsets_in_one_section_save_without_pk_collision() {
    // Regression for the save crash (review finding #7): two dangling station
    // offsets in one section both used to resolve to (section, 0) via
    // key_to_ref_lossy and collide on the (section_id, station_index) PRIMARY
    // KEY, aborting the entire save. The persistence layer now skips a key that
    // doesn't resolve, so the save succeeds and the dangling offsets are dropped
    // (not persisted as phantom station-0 rows).
    let mut db = Database::in_memory().unwrap();
    let mut project = Project::new("Dangling".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![ShotData {
                length: 5.0,
                ..Default::default()
            }],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project.station_offsets.insert(
        StationKey::new(a, StationAnchor::AfterShot("ghost-1".into())),
        (1.0, 0.0, 0.0),
    );
    project.station_offsets.insert(
        StationKey::new(a, StationAnchor::AfterShot("ghost-2".into())),
        (2.0, 0.0, 0.0),
    );

    db.save_project(&project)
        .expect("save must not PK-collide on two dangling offsets");

    let loaded = db.load_project().unwrap();
    assert!(
        loaded.station_offsets.is_empty(),
        "dangling offsets are dropped on save, not written as phantom station-0 rows"
    );
}

// === Raw table browsing ===

#[test]
fn test_list_raw_table_names_excludes_sqlite_internal() {
    let mut db = Database::in_memory().unwrap();
    let project = Project::new("Raw Cave".to_string(), None);
    db.save_project(&project).unwrap();

    let names = db.list_raw_table_names().unwrap();
    assert!(names.iter().any(|n| n == "project"));
    assert!(names.iter().any(|n| n == "sections"));
    assert!(names.iter().any(|n| n == "shots"));
    assert!(!names.iter().any(|n| n.starts_with("sqlite_")));
}

#[test]
fn test_read_raw_table_stringifies_values() {
    let db = Database::in_memory().unwrap();
    db.conn
        .execute_batch(
            "CREATE TABLE mixed (i INTEGER, r REAL, t TEXT, b BLOB, n TEXT);
             INSERT INTO mixed VALUES (42, 1.5, 'hello', x'0102030405', NULL);",
        )
        .unwrap();

    let data = db.read_raw_table("mixed", 100, 0).unwrap();
    assert_eq!(data.columns, vec!["i", "r", "t", "b", "n"]);
    assert_eq!(data.total_rows, 1);
    assert_eq!(
        data.rows,
        vec![vec![
            "42".to_string(),
            "1.5".to_string(),
            "hello".to_string(),
            "<blob 5 B>".to_string(),
            "NULL".to_string(),
        ]]
    );
}

#[test]
fn test_read_raw_table_rejects_unknown_names() {
    let mut db = Database::in_memory().unwrap();
    let project = Project::new("Inject Cave".to_string(), None);
    db.save_project(&project).unwrap();

    // Injection attempts and internal tables must be rejected by the
    // exact-member guard, never interpolated into SQL.
    for name in [
        "shots; DROP TABLE shots",
        "shots\" --",
        "sqlite_master",
        "no_such_table",
        "",
    ] {
        let err = db.read_raw_table(name, 100, 0).unwrap_err();
        assert!(
            err.to_string().contains("Unknown table"),
            "expected rejection for {name:?}, got: {err}"
        );
    }
    // The guard must not have damaged anything.
    assert!(db.read_raw_table("shots", 100, 0).is_ok());
}

#[test]
fn test_read_raw_table_limit_clamp_and_offset_paging() {
    let db = Database::in_memory().unwrap();
    db.conn
        .execute_batch(
            "CREATE TABLE big (v INTEGER);
             INSERT INTO big (v)
             WITH RECURSIVE cnt(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM cnt WHERE x < 10050)
             SELECT x FROM cnt;",
        )
        .unwrap();

    // Oversized limit is clamped to MAX_RAW_PAGE_ROWS.
    let page = db.read_raw_table("big", u32::MAX, 0).unwrap();
    assert_eq!(page.total_rows, 10_050);
    assert_eq!(page.rows.len(), MAX_RAW_PAGE_ROWS as usize);

    // Offset paging returns the remaining rows past the clamp boundary.
    let rest = db.read_raw_table("big", u32::MAX, 10_000).unwrap();
    assert_eq!(rest.total_rows, 10_050);
    assert_eq!(rest.rows.len(), 50);

    // Small pages line up back-to-back.
    let first = db.read_raw_table("big", 2, 0).unwrap();
    let second = db.read_raw_table("big", 2, 2).unwrap();
    assert_eq!(first.rows[0], vec!["1".to_string()]);
    assert_eq!(first.rows[1], vec!["2".to_string()]);
    assert_eq!(second.rows[0], vec!["3".to_string()]);
    assert_eq!(second.rows[1], vec!["4".to_string()]);
}

#[test]
fn test_format_blob_placeholder_units() {
    assert_eq!(format_blob_placeholder(0), "<blob 0 B>");
    assert_eq!(format_blob_placeholder(512), "<blob 512 B>");
    assert_eq!(format_blob_placeholder(1024), "<blob 1.0 KB>");
    assert_eq!(format_blob_placeholder(4300), "<blob 4.2 KB>");
    assert_eq!(format_blob_placeholder(1024 * 1024), "<blob 1.0 MB>");
    assert_eq!(
        format_blob_placeholder(3 * 1024 * 1024 + 512 * 1024),
        "<blob 3.5 MB>"
    );
    assert_eq!(
        format_blob_placeholder(2 * 1024 * 1024 * 1024),
        "<blob 2.0 GB>"
    );
}
