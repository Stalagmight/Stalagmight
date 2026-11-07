//! Display settings, resolved warnings and a complex project through save and load.
//!
//! Lines 2995-3237 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;

// === Display settings persistence ===

#[test]
fn test_display_settings_round_trip() {
    let db = Database::in_memory().unwrap();

    db.save_display_setting("show_grid", "true").unwrap();
    db.save_display_setting("grid_spacing", "10").unwrap();

    let settings = db.load_display_settings().unwrap();
    assert_eq!(settings.get("show_grid"), Some(&"true".to_string()));
    assert_eq!(settings.get("grid_spacing"), Some(&"10".to_string()));

    // Overwrite existing setting
    db.save_display_setting("show_grid", "false").unwrap();
    let updated = db.load_display_settings().unwrap();
    assert_eq!(updated.get("show_grid"), Some(&"false".to_string()));

    // Get individual setting
    let val = db.get_display_setting("grid_spacing").unwrap();
    assert_eq!(val, Some("10".to_string()));

    let missing = db.get_display_setting("nonexistent").unwrap();
    assert!(missing.is_none());
}

// === Resolved warnings persistence ===

#[test]
fn test_resolved_warnings_round_trip() {
    let mut db = Database::in_memory().unwrap();

    // Create a project with a section and shot so FK constraints are satisfied
    let mut project = Project::new("Warnings Test".to_string(), None);
    project.add_section(SectionData {
        shots: vec![ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 5.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    db.save_project(&project).unwrap();

    // Get the actual section and shot IDs from saved project
    let loaded_project = db.load_project().unwrap();
    let section_id = loaded_project.sections[0].id;
    let shot_id = loaded_project.sections[0].shots[0].id;

    let warning = ResolvedWarning {
        section_id,
        shot_id,
        warning_code: "azimuth_deviation".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: Some("Verified in field".to_string()),
        shot_uuid: None,
    };

    db.save_resolved_warning(&warning).unwrap();

    let loaded = db.load_resolved_warnings().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].section_id, section_id);
    assert_eq!(loaded[0].shot_id, shot_id);
    assert_eq!(loaded[0].warning_code, "azimuth_deviation");
    assert_eq!(
        loaded[0].resolved_comment.as_deref(),
        Some("Verified in field")
    );

    // Delete it
    db.delete_resolved_warning(section_id, shot_id, "azimuth_deviation", None)
        .unwrap();
    let remaining = db.load_resolved_warnings().unwrap();
    assert!(remaining.is_empty());
}

// === Save-load round-trip with all complex fields ===

#[test]
fn test_save_load_complex_project_round_trip() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("complex_roundtrip.stalx");

    let mut project = Project::new(
        "Complex Cave".to_string(),
        Some("Deep Location".to_string()),
    );

    // Add section with detailed shots
    let section_a = project
        .add_section(SectionData {
            name: Some("North Passage".to_string()),
            surveyor: Some("Alice".to_string()),
            date: NaiveDate::from_ymd_opt(2025, 6, 15),
            color: "#ff0000".to_string(),
            direction: Some(SurveyDirection::In),
            quality: 0.8,
            shots: vec![
                ShotData {
                    azimuth_in: 0.0,
                    azimuth_out: 0.0,
                    length: 10.0,
                    depth_in: 0.0,
                    depth_out: 2.0,
                    left: Some(1.0),
                    right: Some(2.0),
                    up: Some(3.0),
                    down: Some(0.5),
                    pitch_in: Some(5.0),
                    pitch_out: Some(-3.0),
                    comment: Some("Good visibility".to_string()),
                    ..Default::default()
                },
                ShotData {
                    azimuth_in: 45.0,
                    azimuth_out: 45.0,
                    length: 8.0,
                    depth_in: 2.0,
                    depth_out: 4.0,
                    ..Default::default()
                },
            ],
        })
        .id;

    let section_b = project
        .add_section(SectionData {
            name: Some("East Branch".to_string()),
            color: "#00ff00".to_string(),
            direction: Some(SurveyDirection::Out),
            quality: 1.0,
            shots: vec![ShotData {
                azimuth_in: 90.0,
                azimuth_out: 90.0,
                length: 5.0,
                depth_in: 0.0,
                depth_out: 1.0,
                ..Default::default()
            }],
            ..Default::default()
        })
        .id;

    // Add section offsets
    project.set_section_offset(section_a, (10.0, -5.0));

    // Add station merge
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 2).into(),
        station_b: StationRef::new(section_b, 0).into(),
        merged_at: Utc::now(),
    });

    // Add zero datum
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(section_a, 0).into(),
        gps: GpsCoordinates::new(45.5, -90.3, 0.0).unwrap(),
    });

    // Add loop closure
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(section_a, 1).into(),
        station_b: StationRef::new(section_b, 0).into(),
        strategy: ClosureStrategy::Move,
        is_aligned: false,
        path_length: 15.0,
        misclosure_distance: 0.3,
        horizontal_misclosure: 0.2,
        vertical_misclosure: 0.15,
        error_percentage: 2.0,
    });

    // Add station offset
    project
        .station_offsets
        .insert(StationRef::new(section_a, 1).into(), (0.1, -0.2, 0.05));

    // Save
    {
        let mut db = Database::open(&db_path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Load and verify everything
    let db = Database::open_existing(&db_path).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.name, "Complex Cave");
    assert_eq!(loaded.location, Some("Deep Location".to_string()));
    assert_eq!(loaded.sections.len(), 2);

    // Verify section A
    let s_a = loaded
        .sections
        .iter()
        .find(|s| s.name == Some("North Passage".to_string()))
        .unwrap();
    assert_eq!(s_a.direction, SurveyDirection::In);
    assert!((s_a.quality - 0.8).abs() < 1e-6);
    assert_eq!(s_a.shots.len(), 2);
    assert_eq!(s_a.shots[0].comment, Some("Good visibility".to_string()));
    assert_eq!(s_a.shots[0].left, Some(1.0));
    assert_eq!(s_a.shots[0].pitch_in, Some(5.0));

    // Verify section offsets
    let s_a_id = s_a.id;
    let offset = loaded.section_offsets().get(&s_a_id).unwrap();
    assert!((offset.0 - 10.0).abs() < 1e-10);
    assert!((offset.1 - (-5.0)).abs() < 1e-10);

    // Verify merges
    assert_eq!(loaded.station_merges.len(), 1);
    assert_eq!(
        loaded
            .resolve_key(&loaded.station_merges[0].station_a)
            .unwrap()
            .station_index,
        2
    );

    // Verify zero datums
    assert_eq!(loaded.zero_datums.len(), 1);
    assert!((loaded.zero_datums[0].gps.latitude - 45.5).abs() < 1e-6);

    // Verify loop closures
    assert_eq!(loaded.loop_closures.len(), 1);
    assert!((loaded.loop_closures[0].path_length - 15.0).abs() < 1e-6);
    assert_eq!(loaded.loop_closures[0].strategy, ClosureStrategy::Move);

    // Verify station offsets
    let st_offset = loaded
        .station_offset_for_ref(StationRef::new(s_a_id, 1))
        .unwrap();
    assert!((st_offset.0 - 0.1).abs() < 1e-10);
    assert!((st_offset.1 - (-0.2)).abs() < 1e-10);
    assert!((st_offset.2 - 0.05).abs() < 1e-10);
}
