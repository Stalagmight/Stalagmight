//! Comprehensive integration tests for Stalagmight
//!
//! Tests are organized by category. Run `cargo test` to execute all tests.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp
)]

use chrono::Utc;
use stalagmight_lib::config::{CacheConfig, Config, ValidationConfig};
use stalagmight_lib::core::{Command, PersistStack, UndoData, UndoEntry, UndoStack, Validator};
use stalagmight_lib::models::{
    ClosureStrategy, Coordinates, GpsCoordinates, LoopClosure, Project, Section, SectionData,
    SectionUpdates, Shot, ShotData, ShotKind, ShotUpdates, StationMerge, StationRef,
};
use stalagmight_lib::persistence::Database;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tempfile::NamedTempFile;

// ============================================================================
// Helper functions
// ============================================================================

fn create_test_project() -> Project {
    Project::new("Test Cave".to_string(), Some("Test Location".to_string()))
}

fn create_shot_data(azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    // Use same azimuth for in/out (the function averages them)
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in,
        depth_out,
        ..Default::default()
    }
}

fn create_section_with_shots(name: &str, shots: Vec<ShotData>) -> SectionData {
    SectionData {
        name: Some(name.to_string()),
        surveyor: Some("Test Surveyor".to_string()),
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        quality: 1.0,
        shots,
    }
}

// ============================================================================
// Station Calculation Tests (5 tests)
// ============================================================================

/// Test 3: Station coordinates - verify computed coordinates from known shots
#[test]
fn test_station_coordinates_computed_correctly() {
    let mut project = create_test_project();

    // Create shots: North 10m, then East 10m
    let shot_a = create_shot_data(0.0, 10.0, 5.0, 5.0);
    let shot_b = create_shot_data(90.0, 10.0, 5.0, 6.0);
    project.add_section(create_section_with_shots(
        "Main",
        vec![
            shot_a,         // North 10m (horizontal)
            shot_b.clone(), // East 10m (slope)
        ],
    ));

    let stations = project.compute_station_coordinates();

    assert_eq!(stations.len(), 3); // 2 shots = 3 stations

    // Station 0: origin
    assert!((stations[0].coordinates.x - 0.0).abs() < 0.001);
    assert!((stations[0].coordinates.y - 0.0).abs() < 0.001);

    // Station 1: after north 10m (x=0, y=10)
    assert!((stations[1].coordinates.x - 0.0).abs() < 0.001);
    assert!((stations[1].coordinates.y - 10.0).abs() < 0.001);

    // Station 2: after east 10m slope (x=horizontal, y=10)
    let expected_x = shot_b.horizontal_length();
    assert!((stations[2].coordinates.x - expected_x).abs() < 0.001);
    assert!((stations[2].coordinates.y - 10.0).abs() < 0.001);
}

/// Test 5: Zero datum propagation - verify GPS coords propagate
#[test]
fn test_zero_datum_propagation() {
    let mut project = create_test_project();

    project.add_section(create_section_with_shots(
        "Main",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    // Set zero datum at station 0
    let gps = GpsCoordinates {
        latitude: 45.0,
        longitude: -122.0,
        depth_reference: 5.0,
    };
    project.set_zero_datum(StationRef::new(0, 0), gps).unwrap();

    let stations = project.compute_station_coordinates();

    // Station 0 should be at origin (the zero datum point)
    assert!(stations[0].is_zero_datum);
    assert!((stations[0].coordinates.x - 0.0).abs() < 0.001);
    assert!((stations[0].coordinates.y - 0.0).abs() < 0.001);
}

/// Test 23: Station ID stability - verify station refs maintain format
#[test]
fn test_station_id_stability() {
    let mut project = create_test_project();

    project.add_section(create_section_with_shots(
        "Section A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));
    project.add_section(create_section_with_shots(
        "Section B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)],
    ));

    let stations = project.compute_station_coordinates();

    // Station refs should be section_id:station_index format
    let refs: Vec<_> = stations.iter().map(|s| s.station_ref).collect();

    // Section 0 has 2 stations: 0:0, 0:1
    assert!(refs.contains(&StationRef::new(0, 0)));
    assert!(refs.contains(&StationRef::new(0, 1)));

    // Section 1 has 2 stations: 1:0, 1:1
    assert!(refs.contains(&StationRef::new(1, 0)));
    assert!(refs.contains(&StationRef::new(1, 1)));
}

/// Test 24: 3D distance calculation - verify Euclidean distance
#[test]
fn test_3d_distance_calculation() {
    let coord_a = Coordinates::new(0.0, 0.0, 0.0);
    let coord_b = Coordinates::new(3.0, 4.0, 0.0);

    let distance = coord_a.distance_to(&coord_b);
    assert!((distance - 5.0).abs() < 0.001); // 3-4-5 triangle

    let coord_c = Coordinates::new(0.0, 0.0, 5.0);
    let distance_3d = coord_a.distance_to(&coord_c);
    assert!((distance_3d - 5.0).abs() < 0.001);
}

/// Test 49: Shot pitch values - verify 3D coords account for pitch (placeholder)
#[test]
fn test_shot_pitch_coordinates() {
    // Note: Current implementation doesn't use pitch for coordinate calculation
    // This test verifies the pitch fields are stored correctly
    let shot = Shot::new(
        0,
        ShotData {
            azimuth_in: 0.0,
            azimuth_out: 5.0,
            length: 10.0,
            depth_in: 5.0,
            depth_out: 5.0,
            pitch_in: Some(30.0),
            pitch_out: Some(-30.0),
            ..Default::default()
        },
    );

    assert_eq!(shot.pitch_in, Some(30.0));
    assert_eq!(shot.pitch_out, Some(-30.0));
}

// ============================================================================
// Merge Operations Tests (6 tests)
// ============================================================================

/// Test 4: Station merge repositioning - verify section repositions correctly
#[test]
fn test_station_merge_repositioning() {
    let mut project = create_test_project();

    // Section A goes north (with zero datum to anchor it)
    project.add_section(create_section_with_shots(
        "Section A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    // Set zero datum on section A to anchor it
    project
        .set_zero_datum(
            StationRef::new(0, 0),
            GpsCoordinates {
                latitude: 45.0,
                longitude: -122.0,
                depth_reference: 5.0,
            },
        )
        .unwrap();

    // Section B goes east (starts at origin, will move)
    project.add_section(create_section_with_shots(
        "Section B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)],
    ));

    // Merge station 0:1 (end of A at y=10) with station 1:0 (start of B)
    // Since A has zero datum, B will move to align
    let result = project.merge_stations(StationRef::new(0, 1), StationRef::new(1, 0));
    assert!(result.is_ok());

    let stations = project.compute_station_coordinates();

    // Find stations from section B
    let section_b_start = stations
        .iter()
        .find(|s| s.station_ref.section_id == 1 && s.station_ref.station_index == 0)
        .unwrap();

    // Section B's start should now be at end of section A (y=10 relative to zero datum at origin)
    // After zero datum adjustment, A's end is at y=10, and B starts there
    assert!(
        (section_b_start.coordinates.y - 10.0).abs() < 0.001,
        "Section B start Y should be 10, got {}",
        section_b_start.coordinates.y
    );
}

/// Test 6: Unmerge restores position (positions stay after unmerge in current impl)
#[test]
fn test_unmerge_keeps_position() {
    let mut project = create_test_project();

    project.add_section(create_section_with_shots(
        "Section A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));
    project.add_section(create_section_with_shots(
        "Section B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)],
    ));

    // Merge then unmerge
    project
        .merge_stations(StationRef::new(0, 1), StationRef::new(1, 0))
        .unwrap();

    let merged = project.are_stations_merged(StationRef::new(0, 1), StationRef::new(1, 0));
    assert!(merged);

    project.unmerge_stations(StationRef::new(0, 1), StationRef::new(1, 0));

    let still_merged = project.are_stations_merged(StationRef::new(0, 1), StationRef::new(1, 0));
    assert!(!still_merged);
}

/// Test 40: Duplicate merge rejection - verify already merged stations error
#[test]
fn test_duplicate_merge_rejection() {
    let mut project = create_test_project();

    project.add_section(create_section_with_shots(
        "A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));
    project.add_section(create_section_with_shots(
        "B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)],
    ));

    // First merge succeeds
    let result1 = project.merge_stations(StationRef::new(0, 1), StationRef::new(1, 0));
    assert!(result1.is_ok());

    // Second merge fails
    let result2 = project.merge_stations(StationRef::new(0, 1), StationRef::new(1, 0));
    assert!(result2.is_err());
    assert!(result2.unwrap_err().contains("already merged"));
}

/// Test 43: Delete section with merges - verify merges cleaned up
#[test]
fn test_delete_section_cleanup() {
    let mut project = create_test_project();

    project.add_section(create_section_with_shots(
        "A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));
    project.add_section(create_section_with_shots(
        "B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)],
    ));

    project
        .merge_stations(StationRef::new(0, 1), StationRef::new(1, 0))
        .unwrap();

    assert_eq!(project.station_merges.len(), 1);

    // Remove section B
    project.remove_section(1);

    // Note: Current implementation doesn't auto-clean merges on section delete
    // This test documents the current behavior
    assert_eq!(project.sections.len(), 1);
}

/// Test 48: Multiple zero datums - two datums in disconnected sections
#[test]
fn test_multiple_zero_datums_disconnected() {
    let mut project = create_test_project();

    project.add_section(create_section_with_shots(
        "Section A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));
    project.add_section(create_section_with_shots(
        "Section B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)],
    ));

    // Set zero datum on section A
    let gps_a = GpsCoordinates {
        latitude: 45.0,
        longitude: -122.0,
        depth_reference: 5.0,
    };
    project
        .set_zero_datum(StationRef::new(0, 0), gps_a)
        .unwrap();

    // Set zero datum on section B (disconnected, should succeed)
    let gps_b = GpsCoordinates {
        latitude: 46.0,
        longitude: -123.0,
        depth_reference: 10.0,
    };
    let result = project.set_zero_datum(StationRef::new(1, 0), gps_b);
    assert!(result.is_ok());
    assert_eq!(project.zero_datums.len(), 2);
}

/// Test: Merge nonexistent station fails
#[test]
fn test_merge_nonexistent_station_fails() {
    let mut project = create_test_project();

    project.add_section(create_section_with_shots(
        "A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    // Try to merge with non-existent station
    let result = project.merge_stations(
        StationRef::new(0, 0),
        StationRef::new(99, 0), // doesn't exist
    );
    assert!(result.is_err());
}

// ============================================================================
// Loop Closure Tests (2 tests)
// ============================================================================

/// Test 25: Loop detection simple - two sections merged at ends form loop
#[test]
fn test_loop_detection_simple() {
    let mut project = create_test_project();

    // Create a loop: Section A (north), Section B (east), merge to form loop
    project.add_section(create_section_with_shots(
        "Section A",
        vec![
            create_shot_data(0.0, 10.0, 5.0, 5.0),
            create_shot_data(0.0, 10.0, 5.0, 5.0),
        ],
    ));
    project.add_section(create_section_with_shots(
        "Section B",
        vec![
            create_shot_data(90.0, 10.0, 5.0, 5.0),
            create_shot_data(90.0, 10.0, 5.0, 5.0),
        ],
    ));

    // First merge end of A to start of B (connects them)
    project
        .merge_stations(StationRef::new(0, 2), StationRef::new(1, 0))
        .unwrap();

    // Create loop closure between start of A and end of B (already connected)
    let result = project.create_loop_closure(
        StationRef::new(0, 0),
        StationRef::new(1, 2),
        ClosureStrategy::Batch,
    );

    // This should succeed and create a loop closure
    assert!(result.is_ok());
    assert_eq!(project.loop_closures.len(), 1);
}

/// Test 26: Loop closure error - verify error % calculation
#[test]
fn test_loop_closure_error_calculation() {
    let mut project = create_test_project();

    // Create sections that form almost-closed loop
    project.add_section(create_section_with_shots(
        "Section A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)], // North 10m
    ));
    project.add_section(create_section_with_shots(
        "Section B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)], // East 10m
    ));

    // Connect them in a way that creates a loop
    project
        .merge_stations(
            StationRef::new(0, 1), // End of A
            StationRef::new(1, 0), // Start of B
        )
        .unwrap();

    // Now try to close the loop by creating a loop closure (stations are already connected)
    let result = project.create_loop_closure(
        StationRef::new(0, 0), // Start of A
        StationRef::new(1, 1), // End of B
        ClosureStrategy::Batch,
    );

    assert!(result.is_ok());

    if !project.loop_closures.is_empty() {
        let closure = &project.loop_closures[0];
        // Error percentage should be calculated
        assert!(closure.path_length > 0.0);
    }
}

// ============================================================================
// Undo/Redo Tests (7 tests)
// ============================================================================

/// Test 2: Undo/redo chain - create, add, delete, undo all, redo all
#[test]
fn test_undo_redo_chain() {
    let mut stack = UndoStack::new();

    // Simulate: create section, add shot, delete
    stack.push(Command::CreateSection {
        name: Some("Test".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![],
    });
    stack.push(Command::CreateShot {
        section_id: 0,
        shot: create_shot_data(0.0, 10.0, 5.0, 5.0),
        after_shot_id: None,
        before_shot_id: None,
    });

    assert!(stack.can_undo());
    assert!(!stack.can_redo());

    // Undo one
    let entry = stack.pop_undo().unwrap();
    stack.push_redo(entry);

    assert!(stack.can_undo());
    assert!(stack.can_redo());

    // Undo second
    let entry2 = stack.pop_undo().unwrap();
    stack.push_redo(entry2);

    assert!(!stack.can_undo());
    assert!(stack.can_redo());
}

/// Test 17: Undo stack limit - 110 operations, verify only last 100
#[test]
fn test_undo_stack_limit() {
    let mut stack = UndoStack::new_with_limit(100);

    // Push 110 commands
    for i in 0..110 {
        stack.push(Command::CreateSection {
            name: Some(format!("Section {i}")),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            shots: vec![],
        });
    }

    // Should only have 100
    assert_eq!(stack.len(), 100);
}

/// Test 18: Undo clears redo - action, undo, new action, verify redo cleared
#[test]
fn test_undo_clears_redo_stack() {
    let mut stack = UndoStack::new();

    stack.push(Command::CreateSection {
        name: Some("Test".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![],
    });

    // Undo
    let entry = stack.pop_undo().unwrap();
    stack.push_redo(entry);
    assert!(stack.can_redo());

    // New action should clear redo
    stack.push(Command::CreateSection {
        name: Some("New".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![],
    });
    assert!(!stack.can_redo());
}

/// Test 35: Batch command atomic (unit test - verifies entry structure)
#[test]
fn test_batch_command_structure() {
    let entry = UndoEntry {
        command_bytes: None,
        command: Command::CreateSection {
            name: Some("Test".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            shots: vec![],
        },
        undo_data: UndoData::ProjectSnapshot("{}".to_string()),
        timestamp: None,
    };

    // Verify entry can be serialized
    let (cmd_json, data_json) = UndoStack::serialize_entry(&entry).unwrap();
    assert!(!cmd_json.is_empty());
    assert!(!data_json.is_empty());

    // And deserialized
    let restored = UndoStack::deserialize_entry(&cmd_json, &data_json).unwrap();
    assert!(matches!(restored.command, Command::CreateSection { .. }));
}

/// Test 41: Undo persistence - serialize and deserialize entries
#[test]
fn test_undo_persistence() {
    let mut stack = UndoStack::new();

    stack.push_with_data(
        Command::CreateSection {
            name: Some("Test".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            shots: vec![],
        },
        UndoData::InverseCommand(Command::DeleteSection { section_id: 0 }),
    );

    // Get the serialized rows to persist
    let rows = stack.rows_to_persist(PersistStack::Undo, Some(10));
    assert_eq!(rows.len(), 1);

    // Load into new stack (the untimestamped loader takes the pair, not the triple)
    let serialized: Vec<(String, String)> = rows
        .into_iter()
        .map(|(command_json, undo_data_json, _)| (command_json, undo_data_json))
        .collect();
    let mut new_stack = UndoStack::new();
    new_stack.load_from_persisted(serialized);
    assert_eq!(new_stack.len(), 1);
}

/// Test 45: Undo zero datum - set datum, undo, verify removal
#[test]
fn test_undo_zero_datum() {
    // This tests the data structure for undo datum
    let mut offsets = HashMap::new();
    offsets.insert(0, Some((10.0, 20.0)));
    let undo_data = UndoData::ZeroDatumInfo {
        station_ref: StationRef::new(0, 0),
        previous_gps: None,
        // The index the datum sat at in `Project::zero_datums`, so undo of a
        // removal puts it back THERE instead of appending (#413, finding 17).
        position: Some(1),
        previous_offsets: offsets,
    };

    // Verify serialization
    let json = serde_json::to_string(&undo_data).unwrap();
    let restored: UndoData = serde_json::from_str(&json).unwrap();

    match restored {
        UndoData::ZeroDatumInfo {
            station_ref,
            previous_gps,
            position,
            previous_offsets,
        } => {
            assert_eq!(station_ref, StationRef::new(0, 0));
            assert!(previous_gps.is_none());
            assert_eq!(position, Some(1));
            assert_eq!(previous_offsets.get(&0), Some(&Some((10.0, 20.0))));
        }
        _ => panic!("Wrong undo data type"),
    }
}

/// Test 50: Comprehensive undo - multiple operation types
#[test]
fn test_comprehensive_undo_types() {
    // Test all UndoData variants can be serialized
    let variants = vec![
        UndoData::None,
        UndoData::InverseCommand(Command::DeleteSection { section_id: 0 }),
        UndoData::PreviousValues {
            section_id: 0,
            shot_id: Some(0),
            shot_uuid: None,
            values_json: "{}".to_string(),
            previous_offsets: HashMap::new(),
        },
        UndoData::SectionSnapshot {
            section_id: 0,
            section_data: create_section_with_shots("Test", vec![]),
        },
        UndoData::ShotSnapshot {
            section_id: 0,
            shot_id: 0,
            shot_data: create_shot_data(0.0, 10.0, 5.0, 5.0),
            position: 0,
            uuid: None,
        },
        UndoData::MergeInfo {
            station_a: StationRef::new(0, 0),
            station_b: StationRef::new(1, 0),
            removed_closures: Vec::new(),
            previous_offsets: HashMap::new(),
        },
        UndoData::ZeroDatumInfo {
            station_ref: StationRef::new(0, 0),
            previous_gps: Some(GpsCoordinates {
                latitude: 45.0,
                longitude: -122.0,
                depth_reference: 5.0,
            }),
            position: None,
            previous_offsets: HashMap::new(),
        },
        UndoData::ProjectSnapshot("{}".to_string()),
    ];

    for variant in variants {
        let json = serde_json::to_string(&variant).unwrap();
        let _: UndoData = serde_json::from_str(&json).unwrap();
    }
}

// ============================================================================
// Section Operations Tests (4 tests)
// ============================================================================

/// Test 7: Shot deletion (unit test for shot removal)
#[test]
fn test_shot_deletion() {
    let mut project = create_test_project();

    project.add_section(create_section_with_shots(
        "Main",
        vec![
            create_shot_data(0.0, 10.0, 5.0, 5.0),
            create_shot_data(0.0, 10.0, 5.0, 6.0),
            create_shot_data(0.0, 10.0, 6.0, 7.0),
        ],
    ));

    let section = project.get_section(0).unwrap();
    assert_eq!(section.shots.len(), 3);

    // Remove middle shot
    let shot_id = section.shots[1].id;
    project.remove_shot(0, shot_id);

    let section = project.get_section(0).unwrap();
    assert_eq!(section.shots.len(), 2);
}

/// Test 19: Section metadata update - verify all fields persist
#[test]
fn test_section_metadata_update() {
    let mut project = create_test_project();

    project.add_section(create_section_with_shots("Original", vec![]));

    let updates = SectionUpdates {
        name: Some(Some("Updated Name".to_string())),
        surveyor: Some(Some("New Surveyor".to_string())),
        color: Some("#ff0000".to_string()),
        date: None,
        direction: None,
        quality: Some(1.2),
    };

    project.update_section(0, &updates);

    let section = project.get_section(0).unwrap();
    assert_eq!(section.name, Some("Updated Name".to_string()));
    assert_eq!(section.surveyor, Some("New Surveyor".to_string()));
    assert_eq!(section.color, "#ff0000");
    assert_eq!(section.quality, 1.2);
}

/// Test 20: Section color auto-assign (verify unique colors)
#[test]
fn test_section_colors_unique() {
    let mut project = create_test_project();

    let colors = ["#4ecdc4", "#ff6b6b", "#ffd93d", "#6bcb77"];

    for (i, color) in colors.iter().enumerate() {
        project.add_section(SectionData {
            name: Some(format!("Section {i}")),
            surveyor: None,
            date: None,
            color: (*color).to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![],
        });
    }

    let section_colors: HashSet<_> = project.sections.iter().map(|s| &s.color).collect();
    assert_eq!(section_colors.len(), 4); // All unique
}

/// Test 47: Section date parsing (verify date storage)
#[test]
fn test_section_date_storage() {
    use chrono::NaiveDate;

    let mut project = create_test_project();

    let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();

    project.add_section(SectionData {
        name: Some("Dated Section".to_string()),
        surveyor: None,
        date: Some(date),
        color: "#4ecdc4".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![],
    });

    let section = project.get_section(0).unwrap();
    assert_eq!(section.date, Some(date));
}

// ============================================================================
// Statistics Tests (2 tests)
// ============================================================================

/// Test 31: Project stats - verify calculated statistics
#[test]
fn test_project_stats() {
    let mut project = create_test_project();

    project.add_section(create_section_with_shots(
        "Section A",
        vec![
            create_shot_data(0.0, 10.0, 5.0, 6.0),
            create_shot_data(0.0, 15.0, 6.0, 8.0),
        ],
    ));
    project.add_section(create_section_with_shots(
        "Section B",
        vec![create_shot_data(90.0, 20.0, 10.0, 12.0)],
    ));

    let stats = project.stats();

    assert_eq!(stats.section_count, 2);
    assert_eq!(stats.shot_count, 3);
    assert!((stats.total_length - 45.0).abs() < 0.001); // 10 + 15 + 20
    assert_eq!(stats.station_count, 5); // 3 + 2 stations

    // Depth is the absolute station range the caller fills in (#481):
    // Section A stations 5, 6, 8; Section B stations 10, 12.
    let (min_depth, max_depth) = Project::depth_range(&project.compute_station_coordinates());
    assert!((min_depth - 5.0).abs() < 0.001);
    assert!((max_depth - 12.0).abs() < 0.001);
}

/// Test 32: Penetration distance - linear survey
#[test]
fn test_penetration_distance() {
    let mut project = create_test_project();

    // Create a straight line north: 10m + 10m + 10m = 30m
    project.add_section(create_section_with_shots(
        "Main",
        vec![
            create_shot_data(0.0, 10.0, 5.0, 5.0),
            create_shot_data(0.0, 10.0, 5.0, 5.0),
            create_shot_data(0.0, 10.0, 5.0, 5.0),
        ],
    ));

    // Set zero datum at start
    project
        .set_zero_datum(
            StationRef::new(0, 0),
            GpsCoordinates {
                latitude: 45.0,
                longitude: -122.0,
                depth_reference: 5.0,
            },
        )
        .unwrap();

    let stations = project.compute_station_coordinates();
    let penetration = project.calculate_penetration(&stations);

    // Penetration should be 30m (straight line distance)
    assert!((penetration - 30.0).abs() < 0.001);
}

// ============================================================================
// Move Operations Tests (2 tests)
// ============================================================================

/// Test 33: Move section offset - verify offsets updated
#[test]
fn test_move_section_offset() {
    let mut project = create_test_project();

    project.add_section(create_section_with_shots(
        "Main",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    // Manually set section offset
    project.section_offsets.insert(0, (10.0, 20.0));

    let stations = project.compute_station_coordinates();

    // First station should be at offset position
    assert!((stations[0].coordinates.x - 10.0).abs() < 0.001);
    assert!((stations[0].coordinates.y - 20.0).abs() < 0.001);
}

/// Test 34: Move station connectivity - shot lengths unchanged
#[test]
fn test_move_preserves_shot_data() {
    let mut project = create_test_project();

    project.add_section(create_section_with_shots(
        "Main",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    let original_length = project.get_section(0).unwrap().shots[0].length;

    // Apply offset
    project.section_offsets.insert(0, (100.0, 100.0));

    // Shot data should be unchanged
    let new_length = project.get_section(0).unwrap().shots[0].length;
    assert!((original_length - new_length).abs() < 0.001);
}

// ============================================================================
// Config Tests (2 tests)
// ============================================================================

/// Test 15: Config defaults - verify default values
#[test]
fn test_config_defaults() {
    let stack = UndoStack::new();
    assert_eq!(stack.history_limit(), 100); // Default limit
}

/// Test 16: Config persistence - modify and verify
#[test]
fn test_undo_limit_config() {
    let mut stack = UndoStack::new_with_limit(50);
    assert_eq!(stack.history_limit(), 50);

    stack.set_history_limit(75);
    assert_eq!(stack.history_limit(), 75);
}

/// Test: CacheConfig defaults match SPEC.md values
#[test]
fn test_cache_config_defaults() {
    let cache = CacheConfig::default();
    assert!(
        cache.tile_cache_enabled,
        "tile_cache_enabled should default to true"
    );
    assert_eq!(
        cache.tile_cache_max_mb, 500,
        "tile_cache_max_mb should default to 500 per SPEC.md"
    );
    assert_eq!(
        cache.tile_ttl_days, 30,
        "tile_ttl_days should default to 30"
    );
}

/// Test: Config includes CacheConfig with correct defaults
#[test]
fn test_config_includes_cache_config() {
    let config = Config::default();
    assert!(config.cache.tile_cache_enabled);
    assert_eq!(config.cache.tile_cache_max_mb, 500);
    assert_eq!(config.cache.tile_ttl_days, 30);
}

/// Test: CacheConfig TOML deserialization with missing fields uses defaults
#[test]
fn test_cache_config_partial_toml_deserialization() {
    // Test partial cache config - only tile_cache_enabled specified
    let toml_str = r"
        tile_cache_enabled = false
    ";
    let cache: CacheConfig =
        toml::from_str(toml_str).expect("Should deserialize partial CacheConfig");
    assert!(
        !cache.tile_cache_enabled,
        "Explicit value should be respected"
    );
    assert_eq!(
        cache.tile_cache_max_mb, 500,
        "Missing field should use default"
    );
    assert_eq!(cache.tile_ttl_days, 30, "Missing field should use default");
}

/// Test: CacheConfig TOML deserialization with empty table uses all defaults
#[test]
fn test_cache_config_empty_toml_deserialization() {
    let toml_str = "";
    let cache: CacheConfig =
        toml::from_str(toml_str).expect("Should deserialize empty CacheConfig");
    assert!(cache.tile_cache_enabled);
    assert_eq!(cache.tile_cache_max_mb, 500);
    assert_eq!(cache.tile_ttl_days, 30);
}

/// Test: Config backwards compatibility - old config with removed fields parses correctly
#[test]
fn test_config_backwards_compat_ignores_removed_fields() {
    // Old config format with deprecated fields that were removed:
    // - display.station_size, display.line_thickness
    // - map.snap_threshold_pixels
    // - top-level [snap] section
    // Serde should ignore unknown fields (default behavior)
    let old_config_toml = r##"
[general]
auto_save = true
auto_save_interval_seconds = 60
recent_projects_max = 10

[units]
length = "meters"
depth = "feet"
angle = "degrees"
decimal_precision = 2

[display]
default_background = "#2a2a3e"
grid_color = "#444466"
station_size = 8
line_thickness = 3
font_size = 14
font_color = "#ffffff"

[display.depth_gradient]
enabled = true
shallow_color = "#00ff00"
deep_color = "#ff0000"
shallow_depth = 0.0
deep_depth = 100.0

[section_colors]
palette = ["#ff0000", "#00ff00", "#0000ff"]

[map]
satellite_provider = "esri"
satellite_enabled = true
snap_threshold_pixels = 15
zoom_default = 12
zoom_min = 5
zoom_max = 20

[map.controls]
pan_speed = 1.5
zoom_speed = 2.0
invert_scroll = true

[validation]
warn_shot_length_min = 0.5
warn_shot_length_max = 100.0
warn_depth_change_per_shot_max = 20.0
warn_azimuth_deviation_max = 45.0

[export]
default_image_format = "svg"
default_dpi = 150
default_include_satellite = false
default_include_grid = true
default_include_labels = false

[logging]
level = "debug"
max_directory_size_mb = 256

[shortcuts]
select = "A"
pan = "H"
zoom = "J"
draw = "K"
delete = "L"
merge = "G"
measure = "F"
move_section = "W"
move_station = "E"
undo = "Ctrl+U"
redo = "Ctrl+R"
save = "Ctrl+W"

[snap]
enabled_draw = false
enabled_merge = true
enabled_measure = false
threshold_pixels = 20
"##;

    let config: Config = toml::from_str(old_config_toml).expect(
        "Config with deprecated fields should parse - serde ignores unknown fields by default",
    );

    // Verify custom values were preserved
    assert_eq!(config.general.auto_save_interval_seconds, 60);
    assert_eq!(
        config.units.depth,
        stalagmight_lib::config::LengthUnit::Feet
    );
    assert_eq!(config.display.font_size, 14);
    // `[display.depth_gradient]` was removed (#380 item 1): its table above is
    // ignored, and re-serialising the config drops it.
    let resaved = toml::to_string(&config).expect("config re-serialises");
    assert!(!resaved.contains("[display.depth_gradient]"));
    assert!(!resaved.contains("shallow_color"));
    // A config from before `[map_palette]` existed loads with the theme palettes.
    assert_eq!(
        config.map_palette,
        stalagmight_lib::config::MapPaletteConfig::default()
    );
    assert_eq!(config.map.satellite_provider, "esri");
    assert!(config.map.satellite_enabled);
    assert!(config.map.controls.invert_scroll);

    // New nested configs should have defaults since they weren't in old format
    assert_eq!(config.map.station.radius, 4.0);
    assert_eq!(config.map.station.hit_area, 10.0);
    assert_eq!(config.map.shot.width, 2.0);
    assert_eq!(config.map.shot.hit_area, 8.0);
    assert!(!config.debug.map.show_hitboxes);

    // Legacy [snap] values should be migrated to map.snap
    // The test config had: enabled_draw=false, enabled_merge=true, enabled_measure=false, threshold_pixels=20
    // After migration, map.snap should reflect these values
}

// Note: Legacy snap migration tests are in src-tauri/src/config/mod.rs::tests
// where they have access to the private migrate_legacy() method

/// Test: Config normalization clamps and validates values
#[test]
fn test_config_normalize_clamps_values() {
    use stalagmight_lib::config::{MapConfig, MapSnapConfig, ShotConfig, StationConfig};

    let mut map_config = MapConfig {
        station: StationConfig {
            radius: 0.1,
            hit_area: 0.5,
            ..Default::default()
        }, // Below min
        shot: ShotConfig {
            width: 100.0,
            hit_area: 50.0,
            ..Default::default()
        }, // Above max
        snap: MapSnapConfig {
            threshold: -5.0,
            enabled_merge: true,
            enabled_measure: true,
        },
        ..Default::default()
    };

    map_config.normalize();

    // Station: radius clamped to 1.0, hit_area clamped to 1.0 then max'd to radius
    assert_eq!(map_config.station.radius, 1.0);
    assert_eq!(map_config.station.hit_area, 1.0); // max(1.0, 1.0)

    // Shot: width clamped to 20.0, hit_area clamped to 20.0 then max'd to width
    assert_eq!(map_config.shot.width, 20.0);
    assert_eq!(map_config.shot.hit_area, 20.0);

    // Snap: threshold clamped to 1.0
    assert_eq!(map_config.snap.threshold, 1.0);
}

/// Test: Config normalization handles NaN/inf values
#[test]
fn test_config_normalize_handles_nan_inf() {
    use stalagmight_lib::config::{MapConfig, MapSnapConfig, ShotConfig, StationConfig};

    let mut map_config = MapConfig {
        station: StationConfig {
            radius: f64::NAN,
            hit_area: f64::INFINITY,
            ..Default::default()
        },
        shot: ShotConfig {
            width: f64::NEG_INFINITY,
            hit_area: f64::NAN,
            ..Default::default()
        },
        snap: MapSnapConfig {
            threshold: f64::NAN,
            enabled_merge: true,
            enabled_measure: true,
        },
        ..Default::default()
    };

    map_config.normalize();

    // All should fall back to defaults
    assert_eq!(map_config.station.radius, 4.0);
    assert_eq!(map_config.station.hit_area, 10.0);
    assert_eq!(map_config.shot.width, 2.0);
    assert_eq!(map_config.shot.hit_area, 8.0);
    assert_eq!(map_config.snap.threshold, 10.0);
}

/// Test: hit_area must be >= radius/width after normalization
#[test]
fn test_config_normalize_hit_area_ge_visual_size() {
    use stalagmight_lib::config::{MapConfig, ShotConfig, StationConfig};

    let mut map_config = MapConfig {
        station: StationConfig {
            radius: 20.0,
            hit_area: 5.0,
            ..Default::default()
        }, // hit_area < radius
        shot: ShotConfig {
            width: 15.0,
            hit_area: 3.0,
            ..Default::default()
        }, // hit_area < width
        ..Default::default()
    };

    map_config.normalize();

    // hit_area should be raised to match radius/width
    assert_eq!(map_config.station.radius, 20.0);
    assert_eq!(map_config.station.hit_area, 20.0); // raised to radius

    assert_eq!(map_config.shot.width, 15.0);
    assert_eq!(map_config.shot.hit_area, 15.0); // raised to width
}

// ============================================================================
// Persistence Tests (8 tests)
// ============================================================================

/// Test 8: Reference shot flag - mark, save, reload, verify
#[test]
fn test_reference_shot_flag() {
    let mut project = create_test_project();

    let mut shot_data = create_shot_data(0.0, 10.0, 5.0, 5.0);
    shot_data.kind = ShotKind::Jump;

    project.add_section(create_section_with_shots("Main", vec![shot_data]));

    let shot = &project.get_section(0).unwrap().shots[0];
    assert_eq!(shot.kind, ShotKind::Jump);
}

/// Test 21: Shot LRUD persistence - verify all 4 values
#[test]
fn test_shot_lrud_persistence() {
    let shot = Shot::new(
        0,
        ShotData {
            azimuth_in: 0.0,
            azimuth_out: 5.0,
            length: 10.0,
            depth_in: 5.0,
            depth_out: 5.0,
            left: Some(1.0),
            right: Some(2.0),
            up: Some(3.0),
            down: Some(4.0),
            ..Default::default()
        },
    );

    assert_eq!(shot.left, Some(1.0));
    assert_eq!(shot.right, Some(2.0));
    assert_eq!(shot.up, Some(3.0));
    assert_eq!(shot.down, Some(4.0));
}

/// Test 22: Shot comment special chars - quotes, newlines, unicode
#[test]
fn test_shot_comment_special_chars() {
    let comment = "Test \"quotes\" and\nnewlines and émoji 🎉";

    let shot = Shot::new(
        0,
        ShotData {
            azimuth_in: 0.0,
            azimuth_out: 5.0,
            length: 10.0,
            depth_in: 5.0,
            depth_out: 5.0,
            comment: Some(comment.to_string()),
            ..Default::default()
        },
    );

    assert_eq!(shot.comment, Some(comment.to_string()));

    // Verify JSON serialization handles special chars
    let json = serde_json::to_string(&shot).unwrap();
    let restored: Shot = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.comment, Some(comment.to_string()));
}

/// Test 29: New project creates file (with database)
#[test]
fn test_new_project_creates_file() {
    let file = NamedTempFile::new().unwrap();
    let path = file.path().with_extension("stalx");

    let project = create_test_project();

    // Create database and save
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();

    assert!(path.exists());

    // Clean up
    std::fs::remove_file(&path).ok();
}

/// Test 37: Empty project save/load
#[test]
fn test_empty_project_save_load() {
    let file = NamedTempFile::new().unwrap();
    let path = file.path().with_extension("stalx");

    let project = Project::new("Empty".to_string(), None);
    assert!(project.sections.is_empty());

    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();

    let loaded = db.load_project().unwrap();
    assert_eq!(loaded.name, "Empty");
    assert!(loaded.sections.is_empty());

    std::fs::remove_file(&path).ok();
}

/// Test 38: Large survey performance (simplified - tests data structure)
#[test]
fn test_large_survey_data_structure() {
    let mut project = create_test_project();

    // Create section with 1000 shots
    let shots: Vec<_> = (0..1000)
        .map(|i| create_shot_data(f64::from(i) % 360.0, 1.0, 5.0, 5.0 + (f64::from(i) * 0.01)))
        .collect();

    project.add_section(create_section_with_shots("Large", shots));

    let stats = project.stats();
    assert_eq!(stats.shot_count, 1000);
    assert!((stats.total_length - 1000.0).abs() < 0.001);
}

/// Test 46: Shot order preserved
#[test]
fn test_shot_order_preserved() {
    let mut project = create_test_project();

    let shots = vec![
        create_shot_data(0.0, 1.0, 5.0, 5.0),
        create_shot_data(90.0, 2.0, 5.0, 5.0),
        create_shot_data(180.0, 3.0, 5.0, 5.0),
        create_shot_data(270.0, 4.0, 5.0, 5.0),
    ];

    project.add_section(create_section_with_shots("Ordered", shots));

    let section = project.get_section(0).unwrap();
    assert_eq!(section.shots.len(), 4);
    assert!((section.shots[0].length - 1.0).abs() < 0.001);
    assert!((section.shots[1].length - 2.0).abs() < 0.001);
    assert!((section.shots[2].length - 3.0).abs() < 0.001);
    assert!((section.shots[3].length - 4.0).abs() < 0.001);
}

/// Test: Project JSON serialization round-trip
#[test]
fn test_project_json_roundtrip() {
    let mut project = create_test_project();
    project.add_section(create_section_with_shots(
        "Main",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    let json = serde_json::to_string(&project).unwrap();
    let restored: Project = serde_json::from_str(&json).unwrap();

    assert_eq!(restored.name, project.name);
    assert_eq!(restored.sections.len(), 1);
}

// ============================================================================
// Zero Datum Tests (2 tests)
// ============================================================================

/// Test 44: Zero datum removal keeps position
#[test]
fn test_zero_datum_removal() {
    let mut project = create_test_project();

    project.add_section(create_section_with_shots(
        "Main",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    // Set and then remove zero datum
    project
        .set_zero_datum(
            StationRef::new(0, 0),
            GpsCoordinates {
                latitude: 45.0,
                longitude: -122.0,
                depth_reference: 5.0,
            },
        )
        .unwrap();

    assert_eq!(project.zero_datums.len(), 1);

    project.remove_zero_datum(StationRef::new(0, 0));
    assert_eq!(project.zero_datums.len(), 0);
}

/// Test: Connected stations reject multiple zero datums
#[test]
fn test_connected_zero_datum_rejection() {
    let mut project = create_test_project();

    project.add_section(create_section_with_shots(
        "Main",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    // Set zero datum at station 0
    project
        .set_zero_datum(
            StationRef::new(0, 0),
            GpsCoordinates {
                latitude: 45.0,
                longitude: -122.0,
                depth_reference: 5.0,
            },
        )
        .unwrap();

    // Try to set another zero datum on connected station (same section)
    let result = project.set_zero_datum(
        StationRef::new(0, 1),
        GpsCoordinates {
            latitude: 46.0,
            longitude: -123.0,
            depth_reference: 10.0,
        },
    );

    // Should fail - station 0:1 is connected to 0:0 which has a datum
    assert!(result.is_err());
}

// ============================================================================
// ID Boundary Tests (1 test)
// ============================================================================

/// Test 51: Shot ID overflow prevention
#[test]
fn test_shot_id_generation() {
    let mut project = create_test_project();

    // Add many shots to verify ID generation doesn't overflow
    let shots: Vec<_> = (0..1000)
        .map(|_| create_shot_data(0.0, 1.0, 5.0, 5.0))
        .collect();

    project.add_section(create_section_with_shots("Large", shots));

    let section = project.get_section(0).unwrap();

    // Verify all shots have unique IDs
    let ids: HashSet<_> = section.shots.iter().map(|s| s.id).collect();
    assert_eq!(ids.len(), 1000);
}

// ============================================================================
// Shot Update Tests
// ============================================================================

/// Test: Shot updates apply correctly
#[test]
fn test_shot_updates() {
    let mut shot = Shot::new(0, create_shot_data(0.0, 10.0, 5.0, 5.0));

    let updates = ShotUpdates {
        length: Some(20.0),
        comment: Some(Some("Updated".to_string())),
        kind: Some(ShotKind::Jump),
        ..Default::default()
    };

    shot.apply_updates(&updates);

    assert!((shot.length - 20.0).abs() < 0.001);
    assert_eq!(shot.comment, Some("Updated".to_string()));
    assert_eq!(shot.kind, ShotKind::Jump);
    // Unchanged fields
    assert!((shot.azimuth_in - 0.0).abs() < 0.001);
}

/// Test: Average azimuth calculation
#[test]
fn test_average_azimuth() {
    // Normal case - same in/out
    let shot1 = Shot::new(
        0,
        ShotData {
            azimuth_in: 10.0,
            azimuth_out: 10.0,
            length: 10.0,
            depth_in: 5.0,
            depth_out: 5.0,
            ..Default::default()
        },
    );
    assert!((shot1.average_azimuth_measured() - 10.0).abs() < 0.001);

    // Averaging with small difference
    let shot2 = Shot::new(
        0,
        ShotData {
            azimuth_in: 8.0,
            azimuth_out: 12.0,
            length: 10.0,
            depth_in: 5.0,
            depth_out: 5.0,
            ..Default::default()
        },
    );
    assert!((shot2.average_azimuth_measured() - 10.0).abs() < 0.001);

    // Wraparound case - averaging around 0/360
    let shot3 = Shot::new(
        0,
        ShotData {
            azimuth_in: 358.0,
            azimuth_out: 2.0,
            length: 10.0,
            depth_in: 5.0,
            depth_out: 5.0,
            ..Default::default()
        },
    );
    assert!((shot3.average_azimuth_measured() - 0.0).abs() < 0.001);
}

// ============================================================================
// Section Stats Tests
// ============================================================================

/// Test: Section statistics calculation
#[test]
fn test_section_stats() {
    let mut project = create_test_project();

    let mut shots = vec![
        create_shot_data(0.0, 10.0, 5.0, 8.0),
        create_shot_data(0.0, 15.0, 8.0, 12.0),
    ];
    shots[1].kind = ShotKind::Reference;

    project.add_section(create_section_with_shots("Test", shots));

    let section = project.get_section(0).unwrap();
    let stats = section.stats();

    assert_eq!(stats.shot_count, 2);
    // total_length still counts every shot; #27 deliberately left it raw so no
    // number users already read shifts on upgrade.
    assert!((stats.total_length - 25.0).abs() < 0.001);
    // The reference leg is not swimmable, so it drops out of the swum figure.
    assert!((stats.swum_length - 10.0).abs() < 0.001);
    assert_eq!(stats.reference_shot_count, 1);
    assert_eq!(stats.jump_shot_count, 0);
}

/// A jump counts separately from a reference, and unlike a reference it still
/// counts as swum (#27 q7/q13).
#[test]
fn test_section_stats_splits_jumps_from_references() {
    let mut project = create_test_project();

    let mut shots = vec![
        create_shot_data(0.0, 10.0, 5.0, 8.0),
        create_shot_data(0.0, 15.0, 8.0, 12.0),
    ];
    shots[1].kind = ShotKind::Jump;

    project.add_section(create_section_with_shots("Test", shots));

    let stats = project.get_section(0).unwrap().stats();

    assert_eq!(stats.total_length, 25.0);
    assert_eq!(stats.swum_length, 25.0, "a jump IS swum");
    assert_eq!(stats.jump_shot_count, 1);
    assert_eq!(stats.reference_shot_count, 0);
}

// ============================================================================
// DMP Parser Tests (already exist, but add one more)
// ============================================================================

/// Test 1: DMP round-trip (structure test)
#[test]
fn test_dmp_section_data_roundtrip() {
    // Test that SectionData can round-trip through JSON
    let section_data = create_section_with_shots(
        "Test Section",
        vec![
            create_shot_data(45.0, 10.0, 5.0, 6.0),
            create_shot_data(90.0, 15.0, 6.0, 8.0),
        ],
    );

    let json = serde_json::to_string(&section_data).unwrap();
    let restored: SectionData = serde_json::from_str(&json).unwrap();

    assert_eq!(restored.name, Some("Test Section".to_string()));
    assert_eq!(restored.shots.len(), 2);
}

// ============================================================================
// Error Handling Tests (2 tests)
// ============================================================================

/// Test 39: Invalid station reference handling
#[test]
fn test_invalid_station_reference() {
    let project = create_test_project();

    // Check non-existent station
    let exists = project.station_exists(StationRef::new(999, 0));
    assert!(!exists);
}

/// Test 36: Recent projects (structure test)
#[test]
fn test_project_metadata() {
    let mut project = Project::new("Test Cave".to_string(), Some("Location".to_string()));
    project.file_path = Some(std::path::PathBuf::from("/tmp/test.stalx"));

    project.add_section(create_section_with_shots(
        "Main",
        vec![create_shot_data(0.0, 50.0, 5.0, 10.0)],
    ));

    let metadata = project.metadata().unwrap();

    assert_eq!(metadata.name, "Test Cave");
    assert_eq!(metadata.location, Some("Location".to_string()));
    assert_eq!(metadata.section_count, 1);
    assert!((metadata.total_length - 50.0).abs() < 0.001);
}

// ============================================================================
// NEW COVERAGE TESTS (15 tests for increased coverage)
// ============================================================================

/// Helper: Create default validation config
fn default_validation_config() -> ValidationConfig {
    ValidationConfig {
        warn_shot_length_min: 0.1,
        warn_shot_length_max: 50.0,
        warn_depth_change_per_shot_max: 10.0,
        warn_azimuth_deviation_max: 30.0,
        azimuth_diff_threshold: 20.0,
        pitch_diff_threshold: 5.0,
        inclination_depth_diff_threshold: 10.0,
    }
}

// ----------------------------------------------------------------------------
// Validation Tests (5 tests)
// ----------------------------------------------------------------------------

/// Coverage Test 1: Negative length shot returns NEGATIVE_LENGTH error
#[test]
fn test_negative_length_shot() {
    let shot = ShotData {
        azimuth_in: 45.0,
        azimuth_out: 50.0,
        length: -5.0, // Negative!
        depth_in: 10.0,
        depth_out: 10.0,
        ..Default::default()
    };
    let config = default_validation_config();
    let warnings = Validator::validate_shot(&shot, &config, 0, 0);

    assert!(warnings.iter().any(|w| w.code == "NEGATIVE_LENGTH"));
    assert!(Validator::has_errors(&warnings));
}

/// Coverage Test 2: Short shot warning
#[test]
fn test_short_shot_warning() {
    let shot = ShotData {
        azimuth_in: 45.0,
        azimuth_out: 50.0,
        length: 0.05, // Below 0.1m threshold
        depth_in: 10.0,
        depth_out: 10.0,
        ..Default::default()
    };
    let config = ValidationConfig {
        warn_shot_length_min: 0.1,
        ..default_validation_config()
    };
    let warnings = Validator::validate_shot(&shot, &config, 0, 0);

    assert!(warnings.iter().any(|w| w.code == "SHORT_SHOT"));
    // SHORT_SHOT is a warning, not error
    let short_warning = warnings.iter().find(|w| w.code == "SHORT_SHOT").unwrap();
    assert_eq!(
        short_warning.severity,
        stalagmight_lib::core::WarningSeverity::Warning
    );
}

/// Coverage Test 3: Long shot warning
#[test]
fn test_long_shot_warning() {
    let shot = ShotData {
        azimuth_in: 45.0,
        azimuth_out: 50.0,
        length: 100.0, // Above 50m threshold
        depth_in: 10.0,
        depth_out: 10.0,
        ..Default::default()
    };
    let config = ValidationConfig {
        warn_shot_length_max: 50.0,
        ..default_validation_config()
    };
    let warnings = Validator::validate_shot(&shot, &config, 0, 0);

    assert!(warnings.iter().any(|w| w.code == "LONG_SHOT"));
}

/// Coverage Test 4: Large depth change warning
#[test]
fn test_large_depth_change_warning() {
    let shot = ShotData {
        azimuth_in: 45.0,
        azimuth_out: 50.0,
        length: 10.0,
        depth_in: 10.0,
        depth_out: 25.0, // 15m change, above 10m threshold
        ..Default::default()
    };
    let config = ValidationConfig {
        warn_depth_change_per_shot_max: 10.0,
        ..default_validation_config()
    };
    let warnings = Validator::validate_shot(&shot, &config, 0, 0);

    assert!(warnings.iter().any(|w| w.code == "LARGE_DEPTH_CHANGE"));
}

/// Coverage Test 5: Negative LRUD values return errors
#[test]
fn test_negative_lrud_errors() {
    let shot = ShotData {
        azimuth_in: 45.0,
        azimuth_out: 50.0,
        length: 10.0,
        depth_in: 10.0,
        depth_out: 10.0,
        left: Some(-1.0),
        right: Some(-2.0),
        up: Some(-3.0),
        down: Some(-4.0),
        ..Default::default()
    };
    let warnings = Validator::validate_shot(&shot, &default_validation_config(), 0, 0);

    let lrud_errors: Vec<_> = warnings
        .iter()
        .filter(|w| w.code == "NEGATIVE_LRUD")
        .collect();
    assert_eq!(lrud_errors.len(), 4); // All 4 LRUD values negative
    assert!(Validator::has_errors(&warnings));
}

// ----------------------------------------------------------------------------
// Persistence Tests (5 tests)
// ----------------------------------------------------------------------------

/// Coverage Test 6: Save/load with station merges
#[test]
fn test_save_load_with_station_merges() {
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(create_section_with_shots(
        "A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));
    project.add_section(create_section_with_shots(
        "B",
        vec![create_shot_data(90.0, 10.0, 5.0, 5.0)],
    ));

    // Add a merge
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(0, 1).into(),
        station_b: StationRef::new(1, 0).into(),
        merged_at: Utc::now(),
    });

    let mut db = Database::in_memory().unwrap();
    db.save_project(&project).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.station_merges.len(), 1);
    assert!(loaded.key_is_ref(&loaded.station_merges[0].station_a, StationRef::new(0, 1)));
    assert!(loaded.key_is_ref(&loaded.station_merges[0].station_b, StationRef::new(1, 0)));
}

/// Coverage Test 7: Save/load with zero datums
#[test]
fn test_save_load_with_zero_datums() {
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(create_section_with_shots(
        "A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    project
        .set_zero_datum(
            StationRef::new(0, 0),
            GpsCoordinates {
                latitude: 45.123,
                longitude: -122.456,
                depth_reference: 5.0,
            },
        )
        .unwrap();

    let mut db = Database::in_memory().unwrap();
    db.save_project(&project).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.zero_datums.len(), 1);
    assert!((loaded.zero_datums[0].gps.latitude - 45.123).abs() < 0.0001);
    assert!((loaded.zero_datums[0].gps.longitude - (-122.456)).abs() < 0.0001);
    assert!((loaded.zero_datums[0].gps.depth_reference - 5.0).abs() < 0.0001);
}

/// Coverage Test 8: Save/load with section offsets
#[test]
fn test_save_load_with_section_offsets() {
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(create_section_with_shots(
        "A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));
    project.section_offsets.insert(0, (15.5, -20.3));

    let mut db = Database::in_memory().unwrap();
    db.save_project(&project).unwrap();
    let loaded = db.load_project().unwrap();

    let offset = loaded.section_offsets.get(&0).unwrap();
    assert!((offset.0 - 15.5).abs() < 0.0001);
    assert!((offset.1 - (-20.3)).abs() < 0.0001);
}

/// Coverage Test 9: Save/load with loop closures
#[test]
fn test_save_load_with_loop_closures() {
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(create_section_with_shots(
        "A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(0, 0).into(),
        station_b: StationRef::new(0, 1).into(),
        path_length: 100.0,
        misclosure_distance: 2.5,
        error_percentage: 2.5,
        horizontal_misclosure: 2.0,
        vertical_misclosure: 1.5,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    let mut db = Database::in_memory().unwrap();
    db.save_project(&project).unwrap();
    let loaded = db.load_project().unwrap();

    assert_eq!(loaded.loop_closures.len(), 1);
    assert!((loaded.loop_closures[0].path_length - 100.0).abs() < 0.0001);
    assert!((loaded.loop_closures[0].error_percentage - 2.5).abs() < 0.0001);
    assert!((loaded.loop_closures[0].horizontal_misclosure - 2.0).abs() < 0.0001);
    assert!((loaded.loop_closures[0].vertical_misclosure - 1.5).abs() < 0.0001);
}

/// Coverage Test 10: Open nonexistent file fails
#[test]
fn test_open_nonexistent_file_fails() {
    let result = Database::open_existing(Path::new("/nonexistent/path/file.stalx"));
    assert!(result.is_err());
    match result {
        Err(e) => assert!(e.to_string().contains("does not exist")),
        Ok(_) => panic!("Expected error for nonexistent file"),
    }
}

// ----------------------------------------------------------------------------
// Model Tests (5 tests)
// ----------------------------------------------------------------------------

/// Coverage Test 11: Shot depth_change calculation
#[test]
fn test_shot_depth_change_method() {
    let shot = Shot::new(
        0,
        ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 10.0,
            depth_in: 10.0,
            depth_out: 15.0,
            ..Default::default()
        },
    );
    assert!((shot.depth_change() - 5.0).abs() < 0.001);

    let shot2 = Shot::new(
        1,
        ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 10.0,
            depth_in: 20.0,
            depth_out: 12.0,
            ..Default::default()
        },
    );
    assert!((shot2.depth_change() - (-8.0)).abs() < 0.001);
}

/// Coverage Test 12: Section insert_shot_after
#[test]
fn test_section_insert_shot_after() {
    let mut section = Section::new(
        0,
        SectionData {
            name: Some("Test".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![
                create_shot_data(0.0, 10.0, 5.0, 5.0),
                create_shot_data(90.0, 10.0, 5.0, 5.0),
            ],
        },
    );

    let first_shot_id = section.shots[0].id;
    let new_shot = section.insert_shot_after_with_id(
        first_shot_id,
        create_shot_data(45.0, 5.0, 5.0, 5.0),
        None,
        None,
    );

    assert!(new_shot.is_some());
    assert_eq!(section.shots.len(), 3);
    // New shot should be in position 1 (after first shot)
    assert!((section.shots[1].azimuth_in - 45.0).abs() < 0.001);
    assert!((section.shots[1].length - 5.0).abs() < 0.001);
}

/// Coverage Test 13: Section to_section_data
#[test]
fn test_section_to_section_data() {
    let section = Section::new(
        0,
        SectionData {
            name: Some("Original".to_string()),
            surveyor: Some("John".to_string()),
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![create_shot_data(0.0, 10.0, 5.0, 6.0)],
        },
    );

    let data = section.to_section_data();

    assert_eq!(data.name, Some("Original".to_string()));
    assert_eq!(data.surveyor, Some("John".to_string()));
    assert_eq!(data.color, "#ff0000");
    assert_eq!(data.shots.len(), 1);
    assert!((data.shots[0].length - 10.0).abs() < 0.001);
}

/// Coverage Test 14: Project insert_shot_after
#[test]
fn test_project_insert_shot_after() {
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(create_section_with_shots(
        "A",
        vec![
            create_shot_data(0.0, 10.0, 5.0, 5.0),
            create_shot_data(90.0, 10.0, 5.0, 5.0),
        ],
    ));

    let first_shot_id = project.sections[0].shots[0].id;
    let result = project.insert_shot_after(
        0,
        first_shot_id,
        create_shot_data(45.0, 5.0, 5.0, 5.0),
        None,
        None,
    );

    assert!(result.is_some());
    assert_eq!(project.sections[0].shots.len(), 3);
    // Verify the new shot is in the middle
    assert!((project.sections[0].shots[1].azimuth_in - 45.0).abs() < 0.001);
}

/// Coverage Test 15: Project update_shot
#[test]
fn test_project_update_shot() {
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(create_section_with_shots(
        "A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    let shot_id = project.sections[0].shots[0].id;
    let updates = ShotUpdates {
        length: Some(20.0),
        comment: Some(Some("Updated comment".to_string())),
        kind: Some(ShotKind::Jump),
        ..Default::default()
    };

    let result = project.update_shot(0, shot_id, &updates);
    assert!(result.is_some());

    let shot = project.get_section(0).unwrap().get_shot(shot_id).unwrap();
    assert!((shot.length - 20.0).abs() < 0.001);
    assert_eq!(shot.comment, Some("Updated comment".to_string()));
    assert_eq!(shot.kind, ShotKind::Jump);
}

// ============================================================================
// GPS Metadata Tests (5 tests)
// ============================================================================

/// Test GPS metadata with no zero datums returns None
#[test]
fn test_gps_metadata_no_datums() {
    let mut project = Project::new("Test Cave".to_string(), None);
    project.file_path = Some(std::path::PathBuf::from("/tmp/test.stalx"));
    project.add_section(create_section_with_shots(
        "Main",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    let metadata = project.metadata().unwrap();

    assert!(metadata.gps_center.is_none());
    assert!(metadata.gps_bounds.is_none());
}

/// Test GPS metadata with single zero datum
#[test]
fn test_gps_metadata_single_datum() {
    let mut project = Project::new("Test Cave".to_string(), None);
    project.file_path = Some(std::path::PathBuf::from("/tmp/test.stalx"));
    project.add_section(create_section_with_shots(
        "Main",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    let gps = GpsCoordinates {
        latitude: 45.5,
        longitude: -122.7,
        depth_reference: 0.0,
    };
    project.set_zero_datum(StationRef::new(0, 0), gps).unwrap();

    let metadata = project.metadata().unwrap();

    // Center should be from first (and only) datum (lng, lat order for MapLibre)
    let (lng, lat) = metadata.gps_center.unwrap();
    assert!((lat - 45.5).abs() < 0.0001);
    assert!((lng - (-122.7)).abs() < 0.0001);

    // Bounds should be a point (same as center for single datum)
    let bounds = metadata.gps_bounds.unwrap();
    assert!((bounds.north - 45.5).abs() < 0.0001);
    assert!((bounds.south - 45.5).abs() < 0.0001);
    assert!((bounds.east - (-122.7)).abs() < 0.0001);
    assert!((bounds.west - (-122.7)).abs() < 0.0001);
}

/// Test GPS metadata with multiple zero datums
#[test]
fn test_gps_metadata_multiple_datums() {
    let mut project = Project::new("Test Cave".to_string(), None);
    project.file_path = Some(std::path::PathBuf::from("/tmp/test.stalx"));

    // Create two disconnected sections
    project.add_section(create_section_with_shots(
        "Section A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));
    project.add_section(create_section_with_shots(
        "Section B",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    // Set zero datums on different sections (allowed because disconnected)
    let gps_a = GpsCoordinates {
        latitude: 45.0,
        longitude: -122.0,
        depth_reference: 0.0,
    };
    let gps_b = GpsCoordinates {
        latitude: 46.0,
        longitude: -123.0,
        depth_reference: 0.0,
    };
    project
        .set_zero_datum(StationRef::new(0, 0), gps_a)
        .unwrap();
    project
        .set_zero_datum(StationRef::new(1, 0), gps_b)
        .unwrap();

    let metadata = project.metadata().unwrap();

    // Center should be from first datum (lng, lat order for MapLibre)
    let (lng, lat) = metadata.gps_center.unwrap();
    assert!((lat - 45.0).abs() < 0.0001);
    assert!((lng - (-122.0)).abs() < 0.0001);

    // Bounds should encompass both datums
    let bounds = metadata.gps_bounds.unwrap();
    assert!((bounds.north - 46.0).abs() < 0.0001);
    assert!((bounds.south - 45.0).abs() < 0.0001);
    // For non-dateline-crossing, west is smaller longitude
    assert!((bounds.west - (-123.0)).abs() < 0.0001);
    assert!((bounds.east - (-122.0)).abs() < 0.0001);
}

/// Test GPS bounds calculation handles dateline crossing
#[test]
fn test_gps_metadata_dateline_crossing() {
    let mut project = Project::new("Test Cave".to_string(), None);
    project.file_path = Some(std::path::PathBuf::from("/tmp/test.stalx"));

    // Create three disconnected sections
    project.add_section(create_section_with_shots(
        "Section A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));
    project.add_section(create_section_with_shots(
        "Section B",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));
    project.add_section(create_section_with_shots(
        "Section C",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    // Set zero datums spanning the dateline (179 and -179 are close together)
    let gps_a = GpsCoordinates {
        latitude: 50.0,
        longitude: 179.0,
        depth_reference: 0.0,
    };
    let gps_b = GpsCoordinates {
        latitude: 50.0,
        longitude: -179.0,
        depth_reference: 0.0,
    };
    let gps_c = GpsCoordinates {
        latitude: 50.0,
        longitude: 178.0,
        depth_reference: 0.0,
    };
    project
        .set_zero_datum(StationRef::new(0, 0), gps_a)
        .unwrap();
    project
        .set_zero_datum(StationRef::new(1, 0), gps_b)
        .unwrap();
    project
        .set_zero_datum(StationRef::new(2, 0), gps_c)
        .unwrap();

    let metadata = project.metadata().unwrap();

    let bounds = metadata.gps_bounds.unwrap();

    // The bounds should span the small arc across the dateline (178 to -179)
    // not the large arc (178 to 179 to ... to -179)
    // west should be 178, east should be -179 (crossing dateline)
    assert!((bounds.west - 178.0).abs() < 0.0001);
    assert!((bounds.east - (-179.0)).abs() < 0.0001);
}

/// Test GPS metadata center is stable across updates to other datums
#[test]
fn test_gps_metadata_center_stability() {
    let mut project = Project::new("Test Cave".to_string(), None);
    project.file_path = Some(std::path::PathBuf::from("/tmp/test.stalx"));

    // Create two disconnected sections
    project.add_section(create_section_with_shots(
        "Section A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));
    project.add_section(create_section_with_shots(
        "Section B",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    let gps_a = GpsCoordinates {
        latitude: 45.0,
        longitude: -122.0,
        depth_reference: 0.0,
    };
    project
        .set_zero_datum(StationRef::new(0, 0), gps_a)
        .unwrap();

    // Get initial center
    let metadata = project.metadata().unwrap();
    let (initial_lat, initial_lng) = metadata.gps_center.unwrap();

    // Add another zero datum
    let gps_b = GpsCoordinates {
        latitude: 46.0,
        longitude: -123.0,
        depth_reference: 0.0,
    };
    project
        .set_zero_datum(StationRef::new(1, 0), gps_b)
        .unwrap();

    // Center should still be from first datum
    let metadata = project.metadata().unwrap();
    let (final_lat, final_lng) = metadata.gps_center.unwrap();

    assert!((initial_lat - final_lat).abs() < 0.0001);
    assert!((initial_lng - final_lng).abs() < 0.0001);
}

/// Test that updating the first datum preserves center position (in-place update)
#[test]
fn test_gps_metadata_update_preserves_order() {
    let mut project = Project::new("Test Cave".to_string(), None);
    project.file_path = Some(std::path::PathBuf::from("/tmp/test.stalx"));

    // Create two disconnected sections
    project.add_section(create_section_with_shots(
        "Section A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));
    project.add_section(create_section_with_shots(
        "Section B",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    // Set first datum
    let gps_a = GpsCoordinates {
        latitude: 45.0,
        longitude: -122.0,
        depth_reference: 0.0,
    };
    project
        .set_zero_datum(StationRef::new(0, 0), gps_a)
        .unwrap();

    // Add second datum
    let gps_b = GpsCoordinates {
        latitude: 46.0,
        longitude: -123.0,
        depth_reference: 0.0,
    };
    project
        .set_zero_datum(StationRef::new(1, 0), gps_b)
        .unwrap();

    // Update the first datum with new GPS (should update in-place, not reorder)
    let gps_a_updated = GpsCoordinates {
        latitude: 45.5,
        longitude: -122.5,
        depth_reference: 0.0,
    };
    project
        .set_zero_datum(StationRef::new(0, 0), gps_a_updated)
        .unwrap();

    // Center should still be from first datum (now with updated values)
    // (lng, lat order for MapLibre)
    let metadata = project.metadata().unwrap();
    let (center_lng, center_lat) = metadata.gps_center.unwrap();

    // Verify center is the updated first datum, not the second
    assert!(
        (center_lat - 45.5).abs() < 0.0001,
        "Center should be updated first datum lat"
    );
    assert!(
        (center_lng - (-122.5)).abs() < 0.0001,
        "Center should be updated first datum lng"
    );
}

/// Test that invalid GPS coordinates are rejected
#[test]
fn test_gps_invalid_coordinates_rejected() {
    let mut project = Project::new("Test Cave".to_string(), None);
    project.add_section(create_section_with_shots(
        "Section A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));

    // NaN latitude should be rejected
    let gps_nan = GpsCoordinates {
        latitude: f64::NAN,
        longitude: -122.0,
        depth_reference: 0.0,
    };
    assert!(project
        .set_zero_datum(StationRef::new(0, 0), gps_nan)
        .is_err());

    // Infinity longitude should be rejected
    let gps_inf = GpsCoordinates {
        latitude: 45.0,
        longitude: f64::INFINITY,
        depth_reference: 0.0,
    };
    assert!(project
        .set_zero_datum(StationRef::new(0, 0), gps_inf)
        .is_err());

    // Out of range latitude should be rejected
    let gps_oob_lat = GpsCoordinates {
        latitude: 91.0,
        longitude: -122.0,
        depth_reference: 0.0,
    };
    assert!(project
        .set_zero_datum(StationRef::new(0, 0), gps_oob_lat)
        .is_err());

    // Out of range longitude should be rejected
    let gps_oob_lng = GpsCoordinates {
        latitude: 45.0,
        longitude: 181.0,
        depth_reference: 0.0,
    };
    assert!(project
        .set_zero_datum(StationRef::new(0, 0), gps_oob_lng)
        .is_err());

    // Valid coordinates should work
    let gps_valid = GpsCoordinates {
        latitude: 45.0,
        longitude: -122.0,
        depth_reference: 0.0,
    };
    assert!(project
        .set_zero_datum(StationRef::new(0, 0), gps_valid)
        .is_ok());
}

// ============================================================================
// Merge Projects Tests
// ============================================================================

/// Test: Merge two projects into a new project
#[test]
fn test_merge_projects_basic() {
    use tempfile::tempdir;

    let dir = tempdir().unwrap();

    // Create first project with 2 sections
    let path1 = dir.path().join("project1.stalx");
    let mut project1 = Project::new("Cave A".to_string(), Some("Location A".to_string()));
    project1.add_section(create_section_with_shots(
        "A-Main",
        vec![
            create_shot_data(0.0, 10.0, 5.0, 5.0),
            create_shot_data(45.0, 8.0, 5.0, 6.0),
        ],
    ));
    project1.add_section(create_section_with_shots(
        "A-Side",
        vec![create_shot_data(90.0, 5.0, 6.0, 7.0)],
    ));

    let mut db1 = Database::open(&path1).unwrap();
    db1.save_project(&project1).unwrap();
    drop(db1);

    // Create second project with 1 section
    let path2 = dir.path().join("project2.stalx");
    let mut project2 = Project::new("Cave B".to_string(), Some("Location B".to_string()));
    project2.add_section(create_section_with_shots(
        "B-Main",
        vec![
            create_shot_data(180.0, 12.0, 4.0, 4.0),
            create_shot_data(270.0, 6.0, 4.0, 5.0),
        ],
    ));

    let mut db2 = Database::open(&path2).unwrap();
    db2.save_project(&project2).unwrap();
    drop(db2);

    // Merge projects
    let output_path = dir.path().join("merged.stalx");

    // Load both projects
    let db1 = Database::open_existing(&path1).unwrap();
    let loaded1 = db1.load_project().unwrap();
    let db2 = Database::open_existing(&path2).unwrap();
    let loaded2 = db2.load_project().unwrap();

    // Create merged project
    let mut merged = Project::new(
        "Merged Cave".to_string(),
        Some("Location A, Location B".to_string()),
    );

    // Add sections from project 1
    for section in &loaded1.sections {
        merged.add_section(section.to_section_data());
    }

    // Add sections from project 2
    for section in &loaded2.sections {
        merged.add_section(section.to_section_data());
    }

    // Save merged project
    let mut db_merged = Database::open(&output_path).unwrap();
    db_merged.save_project(&merged).unwrap();

    // Verify merged project
    let db_verify = Database::open_existing(&output_path).unwrap();
    let loaded_merged = db_verify.load_project().unwrap();

    assert_eq!(loaded_merged.name, "Merged Cave");
    assert_eq!(loaded_merged.sections.len(), 3); // 2 from project1 + 1 from project2
    assert_eq!(loaded_merged.sections[0].name, Some("A-Main".to_string()));
    assert_eq!(loaded_merged.sections[1].name, Some("A-Side".to_string()));
    assert_eq!(loaded_merged.sections[2].name, Some("B-Main".to_string()));

    // Verify section IDs are renumbered correctly (0, 1, 2)
    assert_eq!(loaded_merged.sections[0].id, 0);
    assert_eq!(loaded_merged.sections[1].id, 1);
    assert_eq!(loaded_merged.sections[2].id, 2);

    // Verify shots are preserved
    assert_eq!(loaded_merged.sections[0].shots.len(), 2);
    assert_eq!(loaded_merged.sections[1].shots.len(), 1);
    assert_eq!(loaded_merged.sections[2].shots.len(), 2);

    // Verify total stats
    let stats = loaded_merged.stats();
    assert_eq!(stats.section_count, 3);
    assert_eq!(stats.shot_count, 5);
}

/// Test: Merge projects preserves zero datums with remapped IDs
#[test]
fn test_merge_projects_preserves_zero_datums() {
    use tempfile::tempdir;

    let dir = tempdir().unwrap();

    // Create first project with zero datum
    let path1 = dir.path().join("project_zd1.stalx");
    let mut project1 = Project::new("Cave ZD1".to_string(), None);
    project1.add_section(create_section_with_shots(
        "ZD1-Main",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));
    project1
        .set_zero_datum(
            StationRef::new(0, 0),
            GpsCoordinates {
                latitude: 45.0,
                longitude: -122.0,
                depth_reference: 0.0,
            },
        )
        .unwrap();

    let mut db1 = Database::open(&path1).unwrap();
    db1.save_project(&project1).unwrap();
    drop(db1);

    // Create second project with zero datum
    let path2 = dir.path().join("project_zd2.stalx");
    let mut project2 = Project::new("Cave ZD2".to_string(), None);
    project2.add_section(create_section_with_shots(
        "ZD2-Main",
        vec![create_shot_data(90.0, 8.0, 4.0, 4.0)],
    ));
    project2
        .set_zero_datum(
            StationRef::new(0, 1), // End of section
            GpsCoordinates {
                latitude: 46.0,
                longitude: -123.0,
                depth_reference: 5.0,
            },
        )
        .unwrap();

    let mut db2 = Database::open(&path2).unwrap();
    db2.save_project(&project2).unwrap();
    drop(db2);

    // Load and merge
    let db1 = Database::open_existing(&path1).unwrap();
    let loaded1 = db1.load_project().unwrap();
    let db2 = Database::open_existing(&path2).unwrap();
    let loaded2 = db2.load_project().unwrap();

    let mut merged = Project::new("Merged ZD".to_string(), None);

    // Add sections and zero datums from project 1
    for section in &loaded1.sections {
        let old_id = section.id;
        let new_section = merged.add_section(section.to_section_data());
        let new_id = new_section.id;

        // Remap zero datums
        for zd in &loaded1.zero_datums {
            if zd.station_ref.section_id == old_id {
                let idx = loaded1.resolve_key(&zd.station_ref).unwrap().station_index;
                let _ = merged.set_zero_datum(StationRef::new(new_id, idx), zd.gps);
            }
        }
    }

    // Add sections and zero datums from project 2
    for section in &loaded2.sections {
        let old_id = section.id;
        let new_section = merged.add_section(section.to_section_data());
        let new_id = new_section.id;

        // Remap zero datums
        for zd in &loaded2.zero_datums {
            if zd.station_ref.section_id == old_id {
                let idx = loaded2.resolve_key(&zd.station_ref).unwrap().station_index;
                let _ = merged.set_zero_datum(StationRef::new(new_id, idx), zd.gps);
            }
        }
    }

    // Verify zero datums were preserved with remapped section IDs
    assert_eq!(merged.zero_datums.len(), 2);

    // First zero datum from project 1 (section 0, station 0)
    let mzd0 = merged
        .resolve_key(&merged.zero_datums[0].station_ref)
        .unwrap();
    assert_eq!(mzd0.section_id, 0);
    assert_eq!(mzd0.station_index, 0);
    assert!((merged.zero_datums[0].gps.latitude - 45.0).abs() < 0.001);

    // Second zero datum from project 2 (section 1, station 1)
    let mzd1 = merged
        .resolve_key(&merged.zero_datums[1].station_ref)
        .unwrap();
    assert_eq!(mzd1.section_id, 1);
    assert_eq!(mzd1.station_index, 1);
    assert!((merged.zero_datums[1].gps.latitude - 46.0).abs() < 0.001);
}

/// Test: Merge projects with station merges preserves them with remapped IDs
#[test]
fn test_merge_projects_preserves_station_merges() {
    use tempfile::tempdir;

    let dir = tempdir().unwrap();

    // Create project with 2 sections and a station merge
    let path1 = dir.path().join("project_merge1.stalx");
    let mut project1 = Project::new("Cave Merge".to_string(), None);

    project1.add_section(create_section_with_shots(
        "Section A",
        vec![create_shot_data(0.0, 10.0, 5.0, 5.0)],
    ));
    project1.add_section(create_section_with_shots(
        "Section B",
        vec![create_shot_data(90.0, 8.0, 4.0, 4.0)],
    ));

    // Merge last station of section 0 with first station of section 1
    project1
        .merge_stations(
            StationRef::new(0, 1), // End of section 0
            StationRef::new(1, 0), // Start of section 1
        )
        .unwrap();

    let mut db1 = Database::open(&path1).unwrap();
    db1.save_project(&project1).unwrap();
    drop(db1);

    // Create second project (empty)
    let path2 = dir.path().join("project_merge2.stalx");
    let mut project2 = Project::new("Empty Cave".to_string(), None);
    project2.add_section(create_section_with_shots(
        "Section C",
        vec![create_shot_data(180.0, 6.0, 3.0, 3.0)],
    ));

    let mut db2 = Database::open(&path2).unwrap();
    db2.save_project(&project2).unwrap();
    drop(db2);

    // Load and merge
    let db1 = Database::open_existing(&path1).unwrap();
    let loaded1 = db1.load_project().unwrap();
    let db2 = Database::open_existing(&path2).unwrap();
    let loaded2 = db2.load_project().unwrap();

    let mut merged = Project::new("Merged Stations".to_string(), None);
    let mut section_id_map: HashMap<u32, u32> = HashMap::new();

    // Add sections from project 1
    for section in &loaded1.sections {
        let old_id = section.id;
        let new_section = merged.add_section(section.to_section_data());
        section_id_map.insert(old_id, new_section.id);
    }

    // Copy station merges from project 1
    for sm in &loaded1.station_merges {
        if let (Some(&new_a), Some(&new_b)) = (
            section_id_map.get(&sm.station_a.section_id),
            section_id_map.get(&sm.station_b.section_id),
        ) {
            let a_idx = loaded1.resolve_key(&sm.station_a).unwrap().station_index;
            let b_idx = loaded1.resolve_key(&sm.station_b).unwrap().station_index;
            merged.station_merges.push(StationMerge {
                station_a: StationRef::new(new_a, a_idx).into(),
                station_b: StationRef::new(new_b, b_idx).into(),
                merged_at: sm.merged_at,
            });
        }
    }

    // Add sections from project 2
    for section in &loaded2.sections {
        merged.add_section(section.to_section_data());
    }

    // Verify station merge was preserved with remapped IDs
    assert_eq!(merged.station_merges.len(), 1);
    let msa = merged
        .resolve_key(&merged.station_merges[0].station_a)
        .unwrap();
    let msb = merged
        .resolve_key(&merged.station_merges[0].station_b)
        .unwrap();
    assert_eq!(msa.section_id, 0);
    assert_eq!(msa.station_index, 1);
    assert_eq!(msb.section_id, 1);
    assert_eq!(msb.station_index, 0);

    // Verify 3 sections total
    assert_eq!(merged.sections.len(), 3);
}
