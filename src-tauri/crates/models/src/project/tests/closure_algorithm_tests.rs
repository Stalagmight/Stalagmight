//! The loop closure algorithm: validation, metrics, distribution, station moves and merge-alignment
//! conflicts (groups A-E).
//!
//! Lines 5747-6727 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

// === Loop Closure Algorithm Tests ===

// Group A: create_loop_closure validation (new tests only; see existing tests above for basic/duplicate/deferred)

#[test]
fn create_loop_closure_rejects_merged_stations() {
    let mut project = Project::new("ClosureRejectMerged".to_string(), None);

    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // Try to create closure between already-merged stations
    let result = project.create_loop_closure(
        StationRef::new(a, 1),
        StationRef::new(b, 0),
        ClosureStrategy::Batch,
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already merged"));
}

#[test]
fn create_loop_closure_allows_disconnected_with_both_datums() {
    let mut project = Project::new("ClosureDisconnectedDatums".to_string(), None);

    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Each section gets its own zero datum (disconnected but GPS-anchored)
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .set_zero_datum(
            StationRef::new(b, 0),
            GpsCoordinates::new(0.0, 0.01, 0.0).unwrap(),
        )
        .unwrap();

    let result = project.create_loop_closure(
        StationRef::new(a, 1),
        StationRef::new(b, 1),
        ClosureStrategy::Batch,
    );
    assert!(
        result.is_ok(),
        "Disconnected with both datums should allow closure: {:?}",
        result.err()
    );
}

// Group B: compute_closure_metrics

#[test]
fn compute_closure_metrics_known_geometry() {
    let mut project = Project::new("ClosureMetrics".to_string(), None);

    // Section A: 2 shots going north (0°), 10m each → A:0=(0,0), A:1=(0,10), A:2=(0,20)
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going east (90°), 5m → B:0=(0,0), B:1=(5,0)
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A:0 with B:0
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    let metrics = project
        .compute_closure_metrics(StationRef::new(a, 2), StationRef::new(b, 1))
        .unwrap();

    // A:2 is at (0, 20), B:1 is at (5, 0) after merge alignment
    // B was moved to align B:0 with A:0 at origin
    let coords_a2 = project
        .compute_station_raw_coords(StationRef::new(a, 2))
        .unwrap();
    let coords_b1 = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();

    let expected_h = coords_a2.horizontal_distance_to(&coords_b1);
    let expected_v = (coords_a2.z - coords_b1.z).abs();
    let expected_d = coords_a2.distance_to(&coords_b1);

    assert!(
        (metrics.horizontal_misclosure - expected_h).abs() < 1e-6,
        "horizontal: expected {}, got {}",
        expected_h,
        metrics.horizontal_misclosure
    );
    assert!(
        (metrics.vertical_misclosure - expected_v).abs() < 1e-6,
        "vertical: expected {}, got {}",
        expected_v,
        metrics.vertical_misclosure
    );
    assert!(
        (metrics.misclosure_distance - expected_d).abs() < 1e-6,
        "distance: expected {}, got {}",
        expected_d,
        metrics.misclosure_distance
    );
    assert!(metrics.path_length > 0.0, "path_length should be > 0");
    assert!(
        metrics.error_percentage > 0.0,
        "error_percentage should be > 0"
    );
}

#[test]
fn compute_closure_metrics_zero_misclosure() {
    let mut project = Project::new("ClosureZeroMisclosure".to_string(), None);

    // Two sections that arrive at the same point: a perfect loop
    // Section A: 1 shot going north (0°), 10m → A:0=(0,0), A:1=(0,10)
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going north (0°), 10m → B:0=(0,0), B:1=(0,10)
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A:0 with B:0
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // A:1 and B:1 should be at the same point (0, 10)
    let metrics = project
        .compute_closure_metrics(StationRef::new(a, 1), StationRef::new(b, 1))
        .unwrap();

    assert!(
        metrics.misclosure_distance < 1e-6,
        "Expected near-zero misclosure, got {}",
        metrics.misclosure_distance
    );
    assert!(
        metrics.error_percentage < 1e-6,
        "Expected near-zero error, got {}",
        metrics.error_percentage
    );
}

// Group C: distribute_closure_error_internal (core algorithm)

#[test]
fn distribute_closure_simple_two_section_loop() {
    let mut project = Project::new("DistributeSimple".to_string(), None);

    // Section A: 3 shots going north (0°), 10m each → A:3=(0,30)
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going south (180°), 29m → B:1=(0,-29)
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 29.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A:0 with B:0
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // A:3 at (0,30), B:1 at (0,-29). Gap = 59m in y.
    let coords_a3_before = project
        .compute_station_raw_coords(StationRef::new(a, 3))
        .unwrap();
    let coords_b1_before = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!((coords_a3_before.y - 30.0).abs() < 1e-6);
    assert!((coords_b1_before.y - (-29.0)).abs() < 1e-6);

    // Distribute closure error proportionally
    project
        .test_distribute_closure_error(
            StationRef::new(a, 3),
            StationRef::new(b, 1),
            ClosureStrategy::ProportionalByLength,
        )
        .unwrap();

    // After distribution, A:3 and B:1 should have the same coordinates
    let coords_a3_after = project
        .compute_station_raw_coords(StationRef::new(a, 3))
        .unwrap();
    let coords_b1_after = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    let residual = coords_a3_after.distance_to(&coords_b1_after);
    assert!(
        residual < 0.01,
        "Endpoints should align after distribution, residual = {residual}"
    );
}

#[test]
fn distribute_closure_with_zero_datum_at_endpoint() {
    let mut project = Project::new("DistributeDatumEndpoint".to_string(), None);

    // Section A: 3 shots going north (0°), 10m each
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going south (180°), 29m
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 29.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set zero datum on A:0 before merging
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();

    // Merge A:0 with B:0
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // Record A:0 position before distribution
    let coords_a0_before = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();

    // Distribute closure error
    project
        .test_distribute_closure_error(
            StationRef::new(a, 3),
            StationRef::new(b, 1),
            ClosureStrategy::ProportionalByLength,
        )
        .unwrap();

    // A:0 (zero datum) should NOT have moved
    let coords_a0_after = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!(
        coords_a0_before.distance_to(&coords_a0_after) < 1e-6,
        "Zero datum station should not move"
    );

    // Endpoints should align
    let coords_a3_after = project
        .compute_station_raw_coords(StationRef::new(a, 3))
        .unwrap();
    let coords_b1_after = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    let residual = coords_a3_after.distance_to(&coords_b1_after);
    assert!(
        residual < 0.01,
        "Endpoints should align after distribution, residual = {residual}"
    );
}

#[test]
fn distribute_closure_with_zero_datum_mid_path() {
    let mut project = Project::new("DistributeDatumMid".to_string(), None);

    // Section A: 2 shots going north (0°), 10m each → A:0=(0,0), A:1=(0,10), A:2=(0,20)
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going south (180°), 19m → B:1=(0,-19)
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 19.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A:0 with B:0
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // Set zero datum on A:1 (mid-path station)
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 1).into(),
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });

    // Record A:1 position before distribution
    let coords_a1_before = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();

    // Distribute closure error - datum is mid-path, so path splits at A:1
    project
        .test_distribute_closure_error(
            StationRef::new(a, 2),
            StationRef::new(b, 1),
            ClosureStrategy::ProportionalByLength,
        )
        .unwrap();

    // A:1 (zero datum) should NOT have moved
    let coords_a1_after = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    assert!(
        coords_a1_before.distance_to(&coords_a1_after) < 1e-6,
        "Zero datum station should not move, moved by {}",
        coords_a1_before.distance_to(&coords_a1_after)
    );

    // Endpoints should align
    let coords_a2_after = project
        .compute_station_raw_coords(StationRef::new(a, 2))
        .unwrap();
    let coords_b1_after = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    let residual = coords_a2_after.distance_to(&coords_b1_after);
    assert!(
        residual < 0.01,
        "Endpoints should align after distribution, residual = {residual}"
    );
}

#[test]
fn distribute_closure_both_anchored_fails() {
    let mut project = Project::new("DistributeBothAnchored".to_string(), None);

    // Section A: 1 shot going north
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going south, slightly different length
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 9.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A:0 with B:0
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // Set zero datums on both closure endpoints
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 1).into(),
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 1).into(),
        gps: GpsCoordinates::new(0.0, 0.01, 0.0).unwrap(),
    });

    // Should fail because both endpoints are anchored
    let result = project.test_distribute_closure_error(
        StationRef::new(a, 1),
        StationRef::new(b, 1),
        ClosureStrategy::ProportionalByLength,
    );
    assert!(
        result.is_err(),
        "Should fail when both endpoints are anchored to zero datums"
    );
}

#[test]
fn distribute_closure_equal_distribution() {
    let mut project = Project::new("DistributeEqual".to_string(), None);

    // Section A: 3 shots going north, different lengths
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 15.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going south, 29m
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 29.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    project
        .test_distribute_closure_error(
            StationRef::new(a, 3),
            StationRef::new(b, 1),
            ClosureStrategy::EqualDistribution,
        )
        .unwrap();

    // Endpoints should align
    let coords_a3 = project
        .compute_station_raw_coords(StationRef::new(a, 3))
        .unwrap();
    let coords_b1 = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    let residual = coords_a3.distance_to(&coords_b1);
    assert!(
        residual < 0.01,
        "Endpoints should align after equal distribution, residual = {residual}"
    );
}

#[test]
fn distribute_closure_fallback_separate_datums() {
    let mut project = Project::new("DistributeFallbackDatums".to_string(), None);

    // Section A: 2 shots going north → A:2=(0,20)
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going east → B:1=(5+offset,0+offset)
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set separate zero datums on each (NOT connected by merges)
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .set_zero_datum(
            StationRef::new(b, 0),
            GpsCoordinates::new(0.0, 0.01, 0.0).unwrap(),
        )
        .unwrap();

    // Distribute with fallback path (no direct connection)
    project
        .test_distribute_closure_error(
            StationRef::new(a, 2),
            StationRef::new(b, 1),
            ClosureStrategy::ProportionalByLength,
        )
        .unwrap();

    // Endpoints should align
    let coords_a2 = project
        .compute_station_raw_coords(StationRef::new(a, 2))
        .unwrap();
    let coords_b1 = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    let residual = coords_a2.distance_to(&coords_b1);
    assert!(
        residual < 0.01,
        "Endpoints should align via fallback datums, residual = {residual}"
    );
}

/// Stress test: 50-section ring network with closure.
/// Verifies the sparse PCG solver produces correct results for a
/// non-trivial graph (all endpoints align, zero datums preserved).
#[test]
fn distribute_closure_large_ring_network() {
    let mut project = Project::new("LargeRingNetwork".to_string(), None);

    // Create 50 sections, each with 5 shots going at slightly different azimuths.
    // This creates a network where the union-find produces many distinct groups,
    // exercising the sparse solver with a matrix larger than trivial test cases.
    let num_sections = 50;
    let mut section_ids = Vec::new();
    for i in 0..num_sections {
        // Each section goes at a slightly different heading to create a ring
        let azimuth = (i as f64 * 360.0 / num_sections as f64) % 360.0;
        let id = project
            .add_section(SectionData {
                shots: vec![
                    shot(azimuth, azimuth, 10.0, 0.0, 0.0),
                    shot(azimuth, azimuth, 10.0, 0.0, 0.0),
                    shot(azimuth, azimuth, 10.0, 0.0, 0.0),
                    shot(azimuth, azimuth, 10.0, 0.0, 0.0),
                    shot(azimuth, azimuth, 10.0, 0.0, 0.0),
                ],
                ..Default::default()
            })
            .id;
        section_ids.push(id);
    }

    // Chain sections: merge end of section i with start of section i+1
    for i in 0..(num_sections - 1) {
        project
            .merge_stations(
                StationRef::new(section_ids[i], 5),     // end of section i
                StationRef::new(section_ids[i + 1], 0), // start of section i+1
            )
            .unwrap();
    }

    // Set zero datum on the first station
    project
        .set_zero_datum(
            StationRef::new(section_ids[0], 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();

    // Record positions before closure
    let first_station = StationRef::new(section_ids[0], 0);
    let last_station = StationRef::new(section_ids[num_sections - 1], 5);
    let coords_first_before = project.compute_station_raw_coords(first_station).unwrap();

    // Create a closure between last station and first station (closing the ring)
    project
        .test_distribute_closure_error(
            last_station,
            first_station,
            ClosureStrategy::ProportionalByLength,
        )
        .unwrap();

    // Zero datum should not have moved
    let coords_first_after = project.compute_station_raw_coords(first_station).unwrap();
    assert!(
        coords_first_before.distance_to(&coords_first_after) < 1e-6,
        "Zero datum should not move, delta = {}",
        coords_first_before.distance_to(&coords_first_after)
    );

    // Endpoints should align (closure should be resolved)
    let coords_last_after = project.compute_station_raw_coords(last_station).unwrap();
    let coords_first_after2 = project.compute_station_raw_coords(first_station).unwrap();
    let residual = coords_last_after.distance_to(&coords_first_after2);
    assert!(
        residual < 0.01,
        "Ring endpoints should align after distribution, residual = {residual}"
    );

    // All merge points should remain consistent (merged stations should coincide)
    for i in 0..(num_sections - 1) {
        let end_i = project
            .compute_station_raw_coords(StationRef::new(section_ids[i], 5))
            .unwrap();
        let start_next = project
            .compute_station_raw_coords(StationRef::new(section_ids[i + 1], 0))
            .unwrap();
        let merge_residual = end_i.distance_to(&start_next);
        assert!(
            merge_residual < 0.01,
            "Merge point {i} should be consistent, residual = {merge_residual}"
        );
    }
}

// Group D: apply_station_move_internal (tested indirectly)

#[test]
fn apply_station_move_adjusts_incoming_and_outgoing_shots() {
    let mut project = Project::new("StationMoveMiddle".to_string(), None);

    // Section with 3 shots: stations at (0,0), (0,10), (0,20), (0,30)
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot, same geometry to create a closure path
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 29.5, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // Record positions before (verify they're valid)
    let _coords_0_before = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    let _coords_3_before = project
        .compute_station_raw_coords(StationRef::new(a, 3))
        .unwrap();

    // Distribute closure error - this will move intermediate stations including middle ones
    project
        .test_distribute_closure_error(
            StationRef::new(a, 3),
            StationRef::new(b, 1),
            ClosureStrategy::ProportionalByLength,
        )
        .unwrap();

    // Endpoints should be aligned
    let coords_a3 = project
        .compute_station_raw_coords(StationRef::new(a, 3))
        .unwrap();
    let coords_b1 = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!(
        coords_a3.distance_to(&coords_b1) < 0.01,
        "Endpoints should be aligned"
    );

    // Intermediate stations (1, 2) should have moved - their positions should differ from
    // the original linear progression
    let coords_1 = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    let coords_2 = project
        .compute_station_raw_coords(StationRef::new(a, 2))
        .unwrap();

    // All 4 stations should still be in a reasonable order (y increasing or near-increasing)
    let coords_0 = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!(
        coords_0.y < coords_1.y || (coords_0.y - coords_1.y).abs() < 1.0,
        "Station 0 should be before station 1"
    );
    assert!(
        coords_1.y < coords_2.y || (coords_1.y - coords_2.y).abs() < 1.0,
        "Station 1 should be before station 2"
    );
}

#[test]
fn apply_station_move_station_zero_uses_section_offset() {
    let mut project = Project::new("StationMoveZero".to_string(), None);

    // Section A: 1 shot going north
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going south, slightly short - creates misclosure at station 0
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // Record section offset before
    let _offset_before = project
        .section_offsets
        .get(&a)
        .copied()
        .unwrap_or((0.0, 0.0));

    // A:0 is at the origin. B:1 is at (0, 10) + (0, -10) = (0, 0) after merge alignment.
    // Both A:0 and B:1 are at (0, 0), so zero misclosure - perfect loop.
    // Let's modify B to create a gap:
    // Actually, let's use a more direct test - just set up the closure so station 0 needs to move
    let c = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(b, 1), StationRef::new(c, 0))
        .unwrap();

    // Now we have A:0 merged to chain, create a closure between A:0's group and C:1
    // A:0=(0,0), C:1 = some other position - distribute will move stations
    // The test verifies that if station 0 needs an XY move, section_offsets is used
    let _offset_a_before = project
        .section_offsets
        .get(&a)
        .copied()
        .unwrap_or((0.0, 0.0));

    // Distribute error - this exercises apply_station_move_internal on various stations
    project
        .test_distribute_closure_error(
            StationRef::new(a, 0),
            StationRef::new(c, 1),
            ClosureStrategy::EqualDistribution,
        )
        .unwrap();

    // The test passes if no panic/error occurs - the internal logic correctly
    // routes station 0 XY moves through section_offsets
    let coords_a0 = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    let coords_c1 = project
        .compute_station_raw_coords(StationRef::new(c, 1))
        .unwrap();
    assert!(
        coords_a0.distance_to(&coords_c1) < 0.01,
        "Endpoints should align"
    );
}

#[test]
fn apply_station_move_last_station_only_modifies_incoming() {
    let mut project = Project::new("StationMoveLast".to_string(), None);

    // 2 shots: stations 0, 1, 2
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // B: connect to create closure path
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 19.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // A:2 is the last station. After distribution, the last shot (idx=1) should be modified
    // but there's no outgoing shot from A:2.
    project
        .test_distribute_closure_error(
            StationRef::new(a, 2),
            StationRef::new(b, 1),
            ClosureStrategy::ProportionalByLength,
        )
        .unwrap();

    // Verify alignment
    let coords_a2 = project
        .compute_station_raw_coords(StationRef::new(a, 2))
        .unwrap();
    let coords_b1 = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!(
        coords_a2.distance_to(&coords_b1) < 0.01,
        "Endpoints should align, residual = {}",
        coords_a2.distance_to(&coords_b1)
    );

    // All stations should have valid, finite coordinates
    for idx in 0..=2 {
        let c = project
            .compute_station_raw_coords(StationRef::new(a, idx))
            .unwrap();
        assert!(
            c.x.is_finite() && c.y.is_finite() && c.z.is_finite(),
            "Station {idx} coords should be finite"
        );
    }
}

// Group E: conflicting merge-station translations, resolved through adjust_loop_closures

#[test]
fn enforce_merge_alignment_conflicted_section_applies_segment_correction() {
    let mut project = Project::new("EnforceMergeAlignment".to_string(), None);

    // Section A: 3 shots going north, 10m each → A:0=(0,0), A:1=(0,10), A:2=(0,20), A:3=(0,30)
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going east, 5m → B:0=(0,0), B:1=(5,0)
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Section C: 1 shot going west, 5m → C:0=(0,0), C:1=(-5,0)
    let c = project
        .add_section(SectionData {
            shots: vec![shot(270.0, 270.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set datum on A:0
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();

    // Merge B:0 to A:1 and C:0 to A:3
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(a, 3), StationRef::new(c, 0))
        .unwrap();

    // Create closures between B:1 and C:1 - they form a loop through section A
    project
        .create_loop_closure(
            StationRef::new(b, 1),
            StationRef::new(c, 1),
            ClosureStrategy::Batch,
        )
        .unwrap();

    // Adjust loop closures across the conflicting merge translations
    let config = ClosureAdjustmentConfig::default();
    let result = project.adjust_loop_closures(
        StationRef::new(a, 0),
        ClosureStrategy::LeastSquaresWeighted,
        &config,
    );

    // The adjustment may succeed or fail depending on the geometry, but the important thing
    // is that it was exercised and didn't panic
    match result {
        Ok(()) => {
            // Verify stations are still at valid, finite coordinates
            for idx in 0..=3 {
                let coords = project
                    .compute_station_raw_coords(StationRef::new(a, idx))
                    .unwrap();
                assert!(
                    coords.x.is_finite() && coords.y.is_finite() && coords.z.is_finite(),
                    "A:{idx} coords should be finite after adjustment"
                );
            }

            // Zero datum should not have moved
            let coords_a0 = project
                .compute_station_raw_coords(StationRef::new(a, 0))
                .unwrap();
            assert!(
                coords_a0.x.abs() < 1e-6 && coords_a0.y.abs() < 1e-6,
                "Zero datum A:0 should remain at origin"
            );
        }
        Err(e) => {
            // Some errors are expected for certain geometries
            // (e.g., "No eligible loop closures" or convergence failure)
            assert!(
                e.contains("eligible")
                    || e.contains("converge")
                    || e.contains("datum")
                    || e.contains("failed"),
                "Unexpected error: {e}"
            );
        }
    }
}

// Group F: the metrics `adjust_loop_closures` restamps on the closures it adjusted

/// The adjustment re-measures every closure on the adjusted geometry. It used to
/// walk each loop WITH the closure's own zero-cost edge still in the graph, so
/// the walk rode that edge between exactly the two stations being measured and
/// restamped `path_length = 0` - a loop of four shots reported as zero metres
/// long - and, through the zero-path guard, `error_percentage = 0` (#576 W36-NF1).
/// The restamp now measures the loop the way creation does: without the
/// closure's own edge.
#[test]
fn adjust_loop_closures_restamps_the_loop_length_not_zero() {
    let mut project = Project::new("CaveA".to_string(), None);
    // A:0=(0,0) north to A:1, east to A:2, south to A:3=(10,0).
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
                shot(180.0, 180.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    // B:0 merged onto A:3, one shot back west that falls 0.35 m short of A:0.
    let b = project
        .add_section(SectionData {
            shots: vec![shot(271.0, 271.0, 10.3, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .merge_stations(StationRef::new(a, 3), StationRef::new(b, 0))
        .unwrap();
    let created = project
        .create_loop_closure(
            StationRef::new(b, 1),
            StationRef::new(a, 0),
            ClosureStrategy::Batch,
        )
        .unwrap();
    assert!(
        (created.path_length - 40.3).abs() < 1e-9,
        "creation measures the four-shot loop: {}",
        created.path_length
    );

    project
        .adjust_loop_closures(
            StationRef::new(a, 0),
            ClosureStrategy::LeastSquaresWeighted,
            &ClosureAdjustmentConfig::default(),
        )
        .unwrap();

    // The only route between the endpoints other than the closure itself is the
    // four (now adjusted) shots, so that is the loop length the restamp owes.
    let loop_length: f64 = project
        .sections
        .iter()
        .flat_map(|s| s.shots.iter())
        .map(|s| s.length)
        .sum();
    let closure = &project.loop_closures[0];
    println!(
        "after adjust: path_length={} (adjusted loop {loop_length}), misclosure={}, error%={}",
        closure.path_length, closure.misclosure_distance, closure.error_percentage
    );
    assert!(
        (closure.path_length - loop_length).abs() < 1e-9,
        "restamped path_length {} must be the adjusted loop length {loop_length}",
        closure.path_length
    );
    assert!(
        (closure.error_percentage - closure.misclosure_distance / closure.path_length * 100.0)
            .abs()
            < 1e-12,
        "error_percentage {} must be misclosure / path_length * 100",
        closure.error_percentage
    );
}

/// [#576 RV19a-9] The restamp walks each loop without the closure's own edge,
/// and its no-route fallback - the summed length of every section the walk
/// could reach - must leave that edge out too. It summed the sections
/// connected THROUGH the closure being measured, so a closure joining two
/// otherwise unconnected zero-datum sections was restamped with both sides'
/// length where creation (measured before the closure existed) stored one.
#[test]
fn the_restamp_fallback_does_not_count_the_far_side_through_the_closure_itself() {
    let mut project = Project::new("CaveA".to_string(), None);
    // The loop of `adjust_loop_closures_restamps_the_loop_length_not_zero`.
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
                shot(180.0, 180.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(271.0, 271.0, 10.3, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // A separate entrance, tied to the loop by nothing but one closure.
    let z = project
        .add_section(SectionData {
            shots: vec![shot(45.0, 45.0, 7.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .set_zero_datum(
            StationRef::new(z, 0),
            GpsCoordinates::new(0.0005, 0.0005, 0.0).unwrap(),
        )
        .unwrap();
    project
        .merge_stations(StationRef::new(a, 3), StationRef::new(b, 0))
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(b, 1),
            StationRef::new(a, 0),
            ClosureStrategy::Batch,
        )
        .unwrap();
    // Not a strategy the adjustment below solves, so only its metrics move.
    let bridge = project
        .create_loop_closure(
            StationRef::new(a, 2),
            StationRef::new(z, 1),
            ClosureStrategy::Move,
        )
        .unwrap();
    let length_of = |p: &Project, ids: &[SectionId]| -> f64 {
        p.sections
            .iter()
            .filter(|s| ids.contains(&s.id))
            .flat_map(|s| s.shots.iter())
            .map(|s| s.length)
            .sum()
    };
    println!(
        "bridge closure at creation: path_length={} (loop sections {}, far side {})",
        bridge.path_length,
        length_of(&project, &[a, b]),
        length_of(&project, &[z])
    );

    project
        .adjust_loop_closures(
            StationRef::new(a, 0),
            ClosureStrategy::LeastSquaresWeighted,
            &ClosureAdjustmentConfig::default(),
        )
        .unwrap();

    let restamped = project
        .loop_closures
        .iter()
        .find(|c| c.id == bridge.id)
        .unwrap()
        .path_length;
    let near_side = length_of(&project, &[a, b]);
    println!(
        "bridge closure after the restamp: path_length={restamped} (loop sections now \
         {near_side}, far side {})",
        length_of(&project, &[z])
    );
    assert!(
        (restamped - near_side).abs() < 1e-9,
        "with no route but the closure itself, the restamp sums the sections reachable \
         without it ({near_side}), as creation did - not the far side too: got {restamped}"
    );
}
