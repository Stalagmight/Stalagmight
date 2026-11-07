//! Penetration distance, the cleanup_section_references cascade, shot displacement and closure ids.
//!
//! Lines 9066-9404 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

// ============================================================================
// Penetration distance tests
// ============================================================================

#[test]
fn penetration_no_zero_datum_returns_zero() {
    let project = Project::new("NoDatum".to_string(), None);
    let stations = project.compute_station_coordinates();
    assert_eq!(project.calculate_penetration(&stations), 0.0);
}

#[test]
fn penetration_single_section_with_datum() {
    let mut project = Project::new("Pen".to_string(), None);
    let sec_id = project
        .add_section(SectionData {
            // Northward shot: azimuth 0 means north
            shots: vec![shot(0.0, 180.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    let gps = GpsCoordinates::new(45.0, 10.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(sec_id, 0), gps)
        .unwrap();

    let stations = project.compute_station_coordinates();
    let pen = project.calculate_penetration(&stations);
    // Should be ~10m (one shot of 10m from the datum)
    assert!(pen > 9.0 && pen < 11.0, "penetration={pen}, expected ~10");
}

#[test]
fn penetration_datum_at_end_station() {
    let mut project = Project::new("PenEnd".to_string(), None);
    let sec_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 180.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Datum at end of section (station 1)
    let gps = GpsCoordinates::new(45.0, 10.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(sec_id, 1), gps)
        .unwrap();

    let stations = project.compute_station_coordinates();
    let pen = project.calculate_penetration(&stations);
    // First station is 10m from the datum (at end)
    assert!(pen > 9.0 && pen < 11.0, "penetration={pen}, expected ~10");
}

// ============================================================================
// cleanup_section_references cascade tests
// ============================================================================

#[test]
fn cleanup_removes_merges_referencing_section() {
    let mut project = Project::new("Cleanup".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    assert_eq!(project.station_merges.len(), 2);
    project.cleanup_section_references(b);
    // Both merges involve section b, so both should be removed
    assert_eq!(
        project.station_merges.len(),
        0,
        "all merges involving section b should be removed"
    );
}

#[test]
fn cleanup_removes_closures_referencing_section() {
    let mut project = Project::new("ClosureCleanup".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        path_length: 10.0,
        misclosure_distance: 0.1,
        error_percentage: 1.0,
        horizontal_misclosure: 0.05,
        vertical_misclosure: 0.08,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });
    project.loop_closures.push(LoopClosure {
        id: 2,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(a, 1).into(),
        path_length: 5.0,
        misclosure_distance: 0.05,
        error_percentage: 1.0,
        horizontal_misclosure: 0.03,
        vertical_misclosure: 0.02,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });

    project.cleanup_section_references(b);
    // Only the a↔b closure should be removed; a↔a stays
    assert_eq!(project.loop_closures.len(), 1);
    assert_eq!(project.loop_closures[0].id, 2);
}

#[test]
fn cleanup_removes_offsets_and_datums() {
    let mut project = Project::new("OffsetCleanup".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project.section_offsets.insert(a, (1.0, 2.0));
    project.section_offsets.insert(b, (3.0, 4.0));
    project
        .station_offsets
        .insert(StationRef::new(a, 0).into(), (1.0, 2.0, 3.0));
    project
        .station_offsets
        .insert(StationRef::new(b, 0).into(), (4.0, 5.0, 6.0));

    let gps = GpsCoordinates::new(45.0, 10.0, 0.0).unwrap();
    project.set_zero_datum(StationRef::new(a, 0), gps).unwrap();
    project.set_zero_datum(StationRef::new(b, 0), gps).unwrap();

    project.cleanup_section_references(a);

    assert!(
        !project.section_offsets.contains_key(&a),
        "section a offset should be removed"
    );
    assert!(
        project.section_offsets.contains_key(&b),
        "section b offset should remain"
    );
    assert_eq!(
        project.station_offsets.len(),
        1,
        "only b's station offset should remain"
    );
    assert_eq!(
        project.zero_datums.len(),
        1,
        "only b's zero datum should remain"
    );
    assert_eq!(project.zero_datums[0].station_ref.section_id, b);
}

#[test]
fn cleanup_preserves_unrelated_merges() {
    let mut project = Project::new("PreserveUnrelated".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    // Remove section b - only the a↔b merge goes away
    project.cleanup_section_references(b);
    assert_eq!(project.station_merges.len(), 1);
    assert_eq!(project.station_merges[0].station_b.section_id, c);
}

// ============================================================================
// shot_displacement edge cases
// ============================================================================

#[test]
fn shot_displacement_due_north() {
    let data = ShotData {
        azimuth_in: 0.0,
        azimuth_out: 180.0,
        length: 10.0,
        depth_in: 0.0,
        depth_out: 0.0,
        ..Default::default()
    };
    let (dx, dy) = Project::shot_displacement(&data, AzimuthReference::default());
    // Average azimuth = 90° → dx ≈ 10, dy ≈ 0
    assert!(dx.abs() > 9.0, "dx={dx}, expected ~10 for avg azimuth 90°");
    assert!(dy.abs() < 1.0, "dy={dy}, expected ~0");
}

#[test]
fn shot_displacement_vertical_shot_is_zero() {
    let data = ShotData {
        azimuth_in: 45.0,
        azimuth_out: 225.0,
        length: 10.0,
        depth_in: 0.0,
        depth_out: 10.0, // pure vertical
        ..Default::default()
    };
    let (dx, dy) = Project::shot_displacement(&data, AzimuthReference::default());
    assert!(dx.abs() < 1e-6, "vertical shot dx should be ~0, got {dx}");
    assert!(dy.abs() < 1e-6, "vertical shot dy should be ~0, got {dy}");
}

// ============================================================================
// assign_closure_ids
// ============================================================================

#[test]
fn assign_closure_ids_fills_zeros_and_updates_counter() {
    let mut project = Project::new("ClosureIDs".to_string(), None);
    project.loop_closures.push(LoopClosure {
        id: 0,
        station_a: StationRef::new(0, 0).into(),
        station_b: StationRef::new(0, 1).into(),
        path_length: 10.0,
        misclosure_distance: 0.1,
        error_percentage: 1.0,
        horizontal_misclosure: 0.05,
        vertical_misclosure: 0.08,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });
    project.loop_closures.push(LoopClosure {
        id: 5, // already assigned
        station_a: StationRef::new(0, 0).into(),
        station_b: StationRef::new(0, 1).into(),
        path_length: 10.0,
        misclosure_distance: 0.1,
        error_percentage: 1.0,
        horizontal_misclosure: 0.05,
        vertical_misclosure: 0.08,
        strategy: ClosureStrategy::Move,
        is_aligned: false,
    });
    project.loop_closures.push(LoopClosure {
        id: 0,
        station_a: StationRef::new(0, 0).into(),
        station_b: StationRef::new(0, 1).into(),
        path_length: 10.0,
        misclosure_distance: 0.1,
        error_percentage: 1.0,
        horizontal_misclosure: 0.05,
        vertical_misclosure: 0.08,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });

    project.assign_closure_ids();

    // First closure (was 0) gets id=6 (next after max existing=5)
    assert_eq!(project.loop_closures[0].id, 6);
    // Second closure keeps id=5. Its `Move` strategy used to FORCE `is_aligned`
    // on every open; it is MEASURED now (#469 item 2), and this project has no
    // sections at all - so neither endpoint resolves, there is no coincidence to
    // measure, and the claim stays false. (An open that re-asserted the flag
    // would put memory and disk permanently at odds about what the closure is.)
    assert_eq!(project.loop_closures[1].id, 5);
    assert!(
        !project.loop_closures[1].is_aligned,
        "assign_closure_ids must not re-assert an alignment claim it cannot measure"
    );
    // Third closure (was 0) gets id=7
    assert_eq!(project.loop_closures[2].id, 7);
    // next_closure_id should be 8
    assert_eq!(project.next_closure_id, 8);
}
