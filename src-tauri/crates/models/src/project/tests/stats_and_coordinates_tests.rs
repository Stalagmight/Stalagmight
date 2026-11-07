//! Project stats, penetration, merge validation, raw and absolute station coordinates and path
//! length.
//!
//! Lines 7987-8541 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

// =====================================================================
// stats() tests
// =====================================================================

#[test]
fn stats_empty_project() {
    let project = Project::new("Empty".to_string(), None);
    let stats = project.stats();

    assert_eq!(stats.section_count, 0);
    assert_eq!(stats.shot_count, 0);
    assert_eq!(stats.station_count, 0);
    assert_eq!(stats.loop_count, 0);
    assert!((stats.total_length - 0.0).abs() < 1e-6);
    // Depth is a station figure, and an empty project has no stations (#481).
    assert_eq!(
        Project::depth_range(&project.compute_station_coordinates()),
        (0.0, 0.0)
    );
}

#[test]
fn stats_single_section() {
    let mut project = Project::new("Single".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            shot(0.0, 0.0, 10.0, 0.0, 5.0),
            shot(90.0, 90.0, 8.0, 5.0, 12.0),
        ],
        ..Default::default()
    });

    let stats = project.stats();
    assert_eq!(stats.section_count, 1);
    assert_eq!(stats.shot_count, 2);
    assert_eq!(stats.station_count, 3); // 2 shots + 1 first station
    assert!((stats.total_length - 18.0).abs() < 1e-6);
    // stats() leaves depth to the caller, exactly like penetration_distance (#481).
    assert!((stats.min_depth - 0.0).abs() < 1e-6);
    assert!((stats.max_depth - 0.0).abs() < 1e-6);
    // Stations: 0.0, (5+5)/2 = 5.0, 12.0.
    let (min_depth, max_depth) = Project::depth_range(&project.compute_station_coordinates());
    assert!((min_depth - 0.0).abs() < 1e-6);
    assert!((max_depth - 12.0).abs() < 1e-6);
}

#[test]
fn stats_multiple_sections() {
    let mut project = Project::new("Multi".to_string(), None);
    // Section 1 stations: -2, -5
    project.add_section(SectionData {
        shots: vec![shot(0.0, 0.0, 10.0, -2.0, -5.0)],
        ..Default::default()
    });
    // Section 2 stations: 0, (1+1)/2 = 1, 3
    project.add_section(SectionData {
        shots: vec![
            shot(90.0, 90.0, 5.0, 0.0, 1.0),
            shot(90.0, 90.0, 7.0, 1.0, 3.0),
        ],
        ..Default::default()
    });

    let stats = project.stats();
    assert_eq!(stats.section_count, 2);
    assert_eq!(stats.shot_count, 3);
    assert_eq!(stats.station_count, 5); // 3 shots + 2 first stations
    assert!((stats.total_length - 22.0).abs() < 1e-6);
    // Absolute over both sections, not per-section excursions (#481).
    let (min_depth, max_depth) = Project::depth_range(&project.compute_station_coordinates());
    assert!((min_depth - (-5.0)).abs() < 1e-6);
    assert!((max_depth - 3.0).abs() < 1e-6);
}

#[test]
fn stats_with_loop_closures() {
    let mut project = Project::new("Loops".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        strategy: ClosureStrategy::Move,
        is_aligned: false,
        path_length: 10.0,
        misclosure_distance: 0.5,
        horizontal_misclosure: 0.3,
        vertical_misclosure: 0.2,
        error_percentage: 5.0,
    });

    let stats = project.stats();
    assert_eq!(stats.loop_count, 1);
}

/// A section that never comes back up: every station is deep, and the old
/// per-section delta walk would have reported the 0..5 excursion instead of the
/// 30..35 the stations actually sit at (#481).
#[test]
fn depth_range_is_absolute_not_a_delta_walk() {
    let mut project = Project::new("Deep".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            shot(0.0, 0.0, 10.0, 30.0, 32.0),
            shot(0.0, 0.0, 10.0, 32.0, 35.0),
        ],
        ..Default::default()
    });

    // Stations: 30.0, (32+32)/2 = 32.0, 35.0.
    let (min_depth, max_depth) = Project::depth_range(&project.compute_station_coordinates());
    assert!((min_depth - 30.0).abs() < 1e-6);
    assert!((max_depth - 35.0).abs() < 1e-6);

    assert_eq!(Project::depth_range(&[]), (0.0, 0.0));
}

// =====================================================================
// calculate_penetration tests
// =====================================================================

#[test]
fn calculate_penetration_empty_stations_no_datum() {
    let project = Project::new("NoDatum".to_string(), None);
    let stations: Vec<Station> = vec![];
    assert!((project.calculate_penetration(&stations) - 0.0).abs() < 1e-6);
}

#[test]
fn calculate_penetration_single_datum_measures_max_distance() {
    let mut project = Project::new("Penetration".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 15.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Set datum on station 0
    let gps = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 0), gps)
        .unwrap();

    let stations = project.compute_station_coordinates();
    let penetration = project.calculate_penetration(&stations);

    // With shots going north (azimuth 0): total horizontal distance = 10 + 15 = 25
    assert!(
        penetration > 24.0,
        "Expected penetration > 24, got {penetration}"
    );
    assert!(
        penetration < 26.0,
        "Expected penetration < 26, got {penetration}"
    );
}

// =====================================================================
// merge_stations validation tests
// =====================================================================

#[test]
fn merge_stations_rejects_nonexistent_station() {
    let mut project = Project::new("MergeInvalid".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    let result = project.merge_stations(
        StationRef::new(a, 0),
        StationRef::new(999, 0), // nonexistent section
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("do not exist"));
}

#[test]
fn merge_stations_rejects_already_merged_pair() {
    let mut project = Project::new("MergeDupe".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // First merge succeeds
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // Second merge of same pair fails
    let result = project.merge_stations(StationRef::new(a, 0), StationRef::new(b, 0));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already merged"));
}

#[test]
fn merge_stations_rejects_both_datums() {
    let mut project = Project::new("MergeBothDatums".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
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
            StationRef::new(b, 0),
            GpsCoordinates::new(1.0, 1.0, 0.0).unwrap(),
        )
        .unwrap();

    let result = project.merge_stations(StationRef::new(a, 1), StationRef::new(b, 1));
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .contains("Both stations are connected to zero datums"));
}

// =====================================================================
// compute_station_raw_coords with station offsets
// =====================================================================

#[test]
fn compute_station_raw_coords_with_individual_station_offset() {
    let mut project = Project::new("StationOffset".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Station 1 is at (0, 10, 0) without offset
    let station_ref = StationRef::new(section_id, 1);
    let coords_before = project.compute_station_raw_coords(station_ref).unwrap();
    assert!((coords_before.x - 0.0).abs() < 1e-6);
    assert!((coords_before.y - 10.0).abs() < 1e-6);

    // Add individual station offset
    project
        .station_offsets
        .insert(station_ref.into(), (3.0, -2.0, 1.0));

    let coords_after = project.compute_station_raw_coords(station_ref).unwrap();
    assert!((coords_after.x - 3.0).abs() < 1e-6);
    assert!((coords_after.y - 8.0).abs() < 1e-6);
    assert!((coords_after.z - 1.0).abs() < 1e-6);
}

#[test]
fn compute_station_raw_coords_station_zero_with_section_offset() {
    let mut project = Project::new("SectionOffset".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project.section_offsets.insert(section_id, (100.0, 200.0));

    // Station 0 should be at the section offset
    let coords = project
        .compute_station_raw_coords(StationRef::new(section_id, 0))
        .unwrap();
    assert!((coords.x - 100.0).abs() < 1e-6);
    assert!((coords.y - 200.0).abs() < 1e-6);
}

// =====================================================================
// calculate_path_length_between tests
// =====================================================================

#[test]
fn path_length_simple_linear_section() {
    let mut project = Project::new("PathLen".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 15.0, 0.0, 0.0),
                shot(0.0, 0.0, 8.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let path_len =
        project.calculate_path_length_between(StationRef::new(a, 0), StationRef::new(a, 3));
    assert!(
        (path_len - 33.0).abs() < 1e-6,
        "Expected 10+15+8=33, got {path_len}"
    );
}

#[test]
fn path_length_partial_section() {
    let mut project = Project::new("PartialPath".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 15.0, 0.0, 0.0),
                shot(0.0, 0.0, 8.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Path from station 1 to station 3 (two shots: 15 + 8 = 23)
    let path_len =
        project.calculate_path_length_between(StationRef::new(a, 1), StationRef::new(a, 3));
    assert!(
        (path_len - 23.0).abs() < 1e-6,
        "Expected 15+8=23, got {path_len}"
    );
}

#[test]
fn path_length_through_merge() {
    let mut project = Project::new("PathThroughMerge".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge a.1 to b.0 (zero-distance edge)
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Path from a.0 to b.1: 10 (shot in a) + 0 (merge) + 5 (shot in b) = 15
    let path_len =
        project.calculate_path_length_between(StationRef::new(a, 0), StationRef::new(b, 1));
    assert!(
        (path_len - 15.0).abs() < 1e-6,
        "Expected 10+5=15, got {path_len}"
    );
}

#[test]
fn path_length_nonexistent_station_returns_zero() {
    let project = Project::new("NoStation".to_string(), None);
    let path_len =
        project.calculate_path_length_between(StationRef::new(999, 0), StationRef::new(999, 1));
    assert!((path_len - 0.0).abs() < 1e-6);
}

#[test]
fn path_length_fallback_clamps_negative_shot_lengths() {
    // When the target station EXISTS but is unreachable, calculate_path_length_impl
    // falls back to the sum of the start component's shot lengths. That sum must
    // clamp negative lengths to 0 - same as the Dijkstra edges and the other graph
    // builders - so a corrupt/deserialized negative leg doesn't under-count (#176).
    let mut project = Project::new("NegFallback".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, -5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    // A disconnected second section so the target station exists yet is unreachable
    // from A - this forces the fallback-sum branch, not the early not-found return.
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 7.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();

    let len = project.calculate_path_length_between(StationRef::new(a, 0), StationRef::new(b, 0));
    // 10 + max(0, -5) = 10; without the clamp this would be 10 + (-5) = 5.
    assert!(
        (len - 10.0).abs() < 1e-6,
        "fallback sum must clamp the -5 leg to 0 (expected 10.0), got {len}"
    );
}

// =====================================================================
// compute_station_coordinates integration tests
// =====================================================================

#[test]
fn compute_station_coordinates_basic_section() {
    let mut project = Project::new("StationCoords".to_string(), None);
    // Use zero depth change so horizontal_length == length
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let stations = project.compute_station_coordinates();

    // 3 stations: station 0, 1, 2
    let section_stations: Vec<_> = stations
        .iter()
        .filter(|s| s.station_ref.section_id == section_id)
        .collect();
    assert_eq!(section_stations.len(), 3);

    // Station 0 at origin (no section offset)
    let s0 = section_stations
        .iter()
        .find(|s| s.station_ref.station_index == 0)
        .unwrap();
    assert!((s0.coordinates.x).abs() < 1e-6);
    assert!((s0.coordinates.y).abs() < 1e-6);

    // Station 1: north 10m (azimuth 0), z = 0 (no depth change)
    let s1 = section_stations
        .iter()
        .find(|s| s.station_ref.station_index == 1)
        .unwrap();
    assert!(
        (s1.coordinates.x).abs() < 1e-6,
        "s1 x = {}",
        s1.coordinates.x
    );
    assert!(
        (s1.coordinates.y - 10.0).abs() < 1e-6,
        "s1 y = {}",
        s1.coordinates.y
    );
    assert!(
        (s1.coordinates.z).abs() < 1e-6,
        "s1 z = {}",
        s1.coordinates.z
    );

    // Station 2: north 10m then east 5m
    let s2 = section_stations
        .iter()
        .find(|s| s.station_ref.station_index == 2)
        .unwrap();
    assert!(
        (s2.coordinates.x - 5.0).abs() < 1e-6,
        "s2 x = {}",
        s2.coordinates.x
    );
    assert!(
        (s2.coordinates.y - 10.0).abs() < 1e-6,
        "s2 y = {}",
        s2.coordinates.y
    );
    assert!(
        (s2.coordinates.z).abs() < 1e-6,
        "s2 z = {}",
        s2.coordinates.z
    );
}

#[test]
fn compute_station_coordinates_merged_sections_have_merged_with() {
    let mut project = Project::new("MergedCoords".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    let stations = project.compute_station_coordinates();

    // Station a.1 should have b.0 in merged_with
    let a1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 1))
        .unwrap();
    assert!(
        a1.merged_with.contains(&StationRef::new(b, 0)),
        "a.1 should have b.0 in merged_with"
    );

    // Station b.0 should have a.1 in merged_with
    let b0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 0))
        .unwrap();
    assert!(
        b0.merged_with.contains(&StationRef::new(a, 1)),
        "b.0 should have a.1 in merged_with"
    );
}
