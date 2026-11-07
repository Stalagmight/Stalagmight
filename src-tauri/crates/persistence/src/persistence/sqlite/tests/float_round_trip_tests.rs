//! Special float values and full metadata through save and load.
//!
//! Lines 3740-3978 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;

// ============================================================================
// Special float values in save/load round-trips
// ============================================================================

#[test]
fn test_save_load_round_trip_extreme_coordinates() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("extreme_coords.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let mut project = Project::new("Extreme".to_string(), None);
    project.add_section(SectionData {
        name: Some("Deep".to_string()),
        color: "#ff0000".to_string(),
        shots: vec![ShotData {
            azimuth_in: 359.9,
            azimuth_out: 0.1,
            length: 999.99,
            depth_in: 0.0,
            depth_out: 999.99,
            pitch_in: Some(-89.9),
            pitch_out: Some(89.9),
            left: Some(0.01),
            right: Some(999.0),
            up: Some(0.0),
            down: Some(0.0),
            temperature: Some(-2.0),
            ..Default::default()
        }],
        ..Default::default()
    });

    db.save_project(&project).unwrap();
    let loaded = db.load_project().unwrap();

    let shot = &loaded.sections[0].shots[0];
    assert!((shot.azimuth_in - 359.9).abs() < 1e-6);
    assert!((shot.azimuth_out - 0.1).abs() < 1e-6);
    assert!((shot.length - 999.99).abs() < 1e-6);
    assert!((shot.depth_out - 999.99).abs() < 1e-6);
    assert!((shot.pitch_in.unwrap() - (-89.9)).abs() < 1e-6);
    assert!((shot.temperature.unwrap() - (-2.0)).abs() < 1e-6);
}

#[test]
fn test_save_load_round_trip_zero_values() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("zeros.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let mut project = Project::new("Zeros".to_string(), None);
    project.add_section(SectionData {
        name: None, // no name
        color: "#000000".to_string(),
        shots: vec![ShotData {
            uuid: None,
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 0.01, // smallest valid length
            depth_in: 0.0,
            depth_out: 0.0,
            pitch_in: None,
            pitch_out: None,
            left: None,
            right: None,
            up: None,
            down: None,
            comment: None,
            kind: ShotKind::Normal,
            marker_index: None,
            temperature: None,
        }],
        ..Default::default()
    });

    db.save_project(&project).unwrap();
    let loaded = db.load_project().unwrap();

    assert!(loaded.sections[0].name.is_none());
    let shot = &loaded.sections[0].shots[0];
    assert!(shot.pitch_in.is_none());
    assert!(shot.left.is_none());
    assert!(shot.comment.is_none());
    assert!(shot.temperature.is_none());
}

#[test]
fn test_save_load_round_trip_all_metadata() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("all_meta.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let mut project = Project::new("FullMeta".to_string(), Some("Underwater Cave".to_string()));
    project.add_section(SectionData {
        name: Some("Section A".to_string()),
        surveyor: Some("Diver One".to_string()),
        date: Some(NaiveDate::from_ymd_opt(2025, 12, 25).unwrap()),
        color: "#abcdef".to_string(),
        direction: Some(SurveyDirection::Out),
        quality: 0.75,
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 270.0,
            length: 5.0,
            depth_in: 10.0,
            depth_out: 12.0,
            comment: Some("Test shot".to_string()),
            kind: ShotKind::Reference,
            marker_index: Some(42),
            ..Default::default()
        }],
    });

    // Add station merge, zero datum, section offset, station offset
    let section_id = project.sections[0].id;
    project.add_section(SectionData {
        name: Some("Section B".to_string()),
        color: "#fedcba".to_string(),
        shots: vec![ShotData {
            azimuth_in: 45.0,
            azimuth_out: 225.0,
            length: 3.0,
            depth_in: 0.0,
            depth_out: 1.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let section_b_id = project.sections[1].id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_id, 1).into(),
        station_b: StationRef::new(section_b_id, 0).into(),
        merged_at: Utc::now(),
    });

    let gps = GpsCoordinates::new(48.8566, 2.3522, 5.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 0), gps)
        .unwrap();

    // set_zero_datum creates section offsets for all connected sections (A and B are merged),
    // so we don't add a manual section_offset - we just verify the ones set_zero_datum created.
    project
        .station_offsets
        .insert(StationRef::new(section_id, 1).into(), (1.0, 2.0, 3.0));
    project
        .imported_file_hashes
        .push("abc123def456".to_string());
    project.next_closure_id = 42;

    db.save_project(&project).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.name, "FullMeta");
    assert_eq!(loaded.location.as_deref(), Some("Underwater Cave"));
    assert_eq!(loaded.sections.len(), 2);
    assert_eq!(loaded.sections[0].surveyor.as_deref(), Some("Diver One"));
    assert_eq!(loaded.sections[0].direction.to_string(), "out");
    assert!((loaded.sections[0].quality - 0.75).abs() < 1e-6);
    assert_eq!(loaded.sections[0].shots[0].kind, ShotKind::Reference);
    assert_eq!(loaded.sections[0].shots[0].marker_index, Some(42));
    assert_eq!(loaded.station_merges.len(), 1);
    assert_eq!(loaded.zero_datums.len(), 1);
    assert!((loaded.zero_datums[0].gps.latitude - 48.8566).abs() < 1e-6);
    // The section offsets `set_zero_datum` produced survive the round trip
    // EXACTLY - value for value, not merely in number. Only the displaced
    // section carries one: `section_offsets` means "sections that are actually
    // displaced", so a section the datum leaves on exactly (0, 0) has no entry
    // to persist (#413 finding 2). This used to assert `len() == 2`, counting
    // that inert entry and checking no value at all.
    assert_eq!(loaded.section_offsets(), project.section_offsets());
    assert_eq!(loaded.section_offsets().len(), 1);
    assert_eq!(loaded.station_offsets.len(), 1);
    assert_eq!(loaded.imported_file_hashes.len(), 1);
    assert_eq!(loaded.imported_file_hashes[0], "abc123def456");
    assert_eq!(loaded.next_closure_id, 42);
}

#[test]
fn test_save_load_round_trip_loop_closures_with_strategy() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("closure_strategy.stalx");

    let mut db = Database::open(&db_path).unwrap();
    let mut project = Project::new("Closures".to_string(), None);
    project.add_section(SectionData {
        color: "#ff0000".to_string(),
        shots: vec![ShotData {
            length: 5.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(sec_id, 0).into(),
        station_b: StationRef::new(sec_id, 1).into(),
        path_length: 50.0,
        misclosure_distance: 0.1,
        error_percentage: 0.2,
        horizontal_misclosure: 0.05,
        vertical_misclosure: 0.08,
        strategy: ClosureStrategy::Move,
        is_aligned: true,
    });
    project.loop_closures.push(LoopClosure {
        id: 2,
        station_a: StationRef::new(sec_id, 0).into(),
        station_b: StationRef::new(sec_id, 1).into(),
        path_length: 100.0,
        misclosure_distance: 0.5,
        error_percentage: 0.5,
        horizontal_misclosure: 0.3,
        vertical_misclosure: 0.2,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });
    project.next_closure_id = 3;

    db.save_project(&project).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.loop_closures.len(), 2);
    let lc1 = &loaded.loop_closures[0];
    assert_eq!(lc1.id, 1);
    assert_eq!(lc1.strategy, ClosureStrategy::Move);
    assert!(lc1.is_aligned);
    assert!((lc1.path_length - 50.0).abs() < 1e-6);

    let lc2 = &loaded.loop_closures[1];
    assert_eq!(lc2.id, 2);
    assert_eq!(lc2.strategy, ClosureStrategy::Batch);
    assert!(!lc2.is_aligned);
    assert_eq!(loaded.next_closure_id, 3);
}
