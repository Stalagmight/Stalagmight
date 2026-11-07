//! Station depth and raw xy coordinates.
//!
//! Lines 9405-9745 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

// (Removed `shift_station_refs_adjusts_all_references`: `shift_station_refs`
// no longer exists. Stable shot-UUID anchors are position-independent, so an
// insert/delete never rewrites a cross-reference's index - the ref resolves to
// the moved station on the fly. Covered by the insert/delete identity tests.)

// ── Station depth computation tests ─────────────────────────────────

#[test]
fn compute_station_depth_from_surrounding_shots() {
    // Station 1 has an incoming shot (depth_out=5.0) and outgoing shot (depth_in=6.0).
    // Average = (5 + 6) / 2 = 5.5
    let mut project = Project::new("Depth".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            shot(0.0, 0.0, 1.0, 0.0, 5.0),  // shot 0: depth_in=0, depth_out=5
            shot(0.0, 0.0, 1.0, 6.0, 10.0), // shot 1: depth_in=6, depth_out=10
        ],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    // Station 1 is between the two shots
    let coords = project
        .compute_station_raw_coords(StationRef::new(sec_id, 1))
        .unwrap();
    // Average of shot[0].depth_out (5.0) and shot[1].depth_in (6.0)
    assert!(
        (coords.z - 5.5).abs() < 1e-9,
        "depth should be 5.5, got {}",
        coords.z
    );
}

#[test]
fn compute_station_depth_first_station() {
    // Station 0 has only an outgoing shot - depth = shot[0].depth_in
    let mut project = Project::new("Depth".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(0.0, 0.0, 1.0, 3.0, 7.0)],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    let coords = project
        .compute_station_raw_coords(StationRef::new(sec_id, 0))
        .unwrap();
    assert!(
        (coords.z - 3.0).abs() < 1e-9,
        "first station depth should be 3.0, got {}",
        coords.z
    );
}

#[test]
fn compute_station_depth_last_station() {
    // Station 2 (last) has only an incoming shot - depth = shot[1].depth_out
    let mut project = Project::new("Depth".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            shot(0.0, 0.0, 1.0, 0.0, 5.0),
            shot(0.0, 0.0, 1.0, 5.0, 12.0),
        ],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    let coords = project
        .compute_station_raw_coords(StationRef::new(sec_id, 2))
        .unwrap();
    assert!(
        (coords.z - 12.0).abs() < 1e-9,
        "last station depth should be 12.0, got {}",
        coords.z
    );
}

#[test]
fn compute_station_depth_with_merge_averages_both_sections() {
    // Two sections merged at a station - depth should average across all colocated readings
    let mut project = Project::new("MergeDepth".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 1.0, 0.0, 8.0), // shot 0: depth_out=8
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 1.0, 10.0, 20.0), // shot 0: depth_in=10
            ],
            ..Default::default()
        })
        .id;

    // Merge A:1 with B:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Station A:1 is colocated with B:0
    // Depths: A shot[0].depth_out=8, B shot[0].depth_in=10 → average = 9
    let coords = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    assert!(
        (coords.z - 9.0).abs() < 1e-9,
        "merged station depth should be 9.0, got {}",
        coords.z
    );
}

/// [#469] The single-station and whole-component coordinate APIs agree to the BIT on
/// the depth of the same colocated group.
///
/// They average the same measurements in different orders - `compute_station_raw_coords`
/// walks the BFS order out of `get_colocated_stations` (which starts at the station
/// asked about), `compute_component_raw_coords` walks station-list order - and
/// floating-point addition is not associative. The two depths per station below are
/// chosen so that difference is REAL rather than theoretical: summed
/// `[6.58, 6.35, 33.29, 40.99]` the average is 21.802500000000002, and summed
/// `[33.29, 40.99, 6.58, 6.35]` it is 21.8025. Sorting the group inside
/// `compute_station_depth` is what collapses the two.
#[test]
fn the_two_coordinate_apis_average_a_colocated_group_identically() {
    let mut project = Project::new("AverageOrder".to_string(), None);
    // Section A first, so it sorts first AND comes first in `self.sections` - the
    // component API's order. The BFS order is whatever station is asked about, which
    // below is B's.
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 1.0, 0.0, 33.29),
                shot(0.0, 0.0, 1.0, 40.99, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 1.0, 0.0, 6.58),
                shot(90.0, 90.0, 1.0, 6.35, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 1).into(),
        merged_at: Utc::now(),
    });

    let component: HashSet<SectionId> = [a, b].into_iter().collect();
    let by_component = project.compute_component_raw_coords(&component);
    for station in [StationRef::new(a, 1), StationRef::new(b, 1)] {
        let single = project.compute_station_raw_coords(station).unwrap();
        let component_z = by_component.get(&station).unwrap().z;
        assert_eq!(
            single.z.to_bits(),
            component_z.to_bits(),
            "the same group must average to the same bits whichever API asks: \
             {} vs {component_z} for {station:?}",
            single.z
        );
    }
}

#[test]
fn compute_station_depth_no_shots_returns_zero() {
    // Edge case: section with no shots, station 0 has no surrounding shots
    let mut project = Project::new("Empty".to_string(), None);
    project.add_section(SectionData {
        shots: vec![],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    let coords = project
        .compute_station_raw_coords(StationRef::new(sec_id, 0))
        .unwrap();
    assert!(
        (coords.z).abs() < 1e-9,
        "depth should be 0.0 for empty section, got {}",
        coords.z
    );
}

#[test]
fn compute_raw_coords_xy_walk_north() {
    // A shot due north (azimuth=0) with length 5m, flat (depth_in == depth_out)
    // horizontal_length = sqrt(5^2 - 0^2) = 5
    // dx = 5 * sin(0) = 0, dy = 5 * cos(0) = 5
    let mut project = Project::new("Walk".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(0.0, 0.0, 5.0, 10.0, 10.0)],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    let c0 = project
        .compute_station_raw_coords(StationRef::new(sec_id, 0))
        .unwrap();
    assert!((c0.x).abs() < 1e-9, "station 0 x should be 0");
    assert!((c0.y).abs() < 1e-9, "station 0 y should be 0");

    let c1 = project
        .compute_station_raw_coords(StationRef::new(sec_id, 1))
        .unwrap();
    assert!((c1.x).abs() < 1e-9, "station 1 x should be 0 (due north)");
    assert!(
        (c1.y - 5.0).abs() < 1e-9,
        "station 1 y should be 5.0, got {}",
        c1.y
    );
}

#[test]
fn compute_raw_coords_xy_walk_east() {
    // A shot due east (azimuth=90) with length 3m, flat
    let mut project = Project::new("Walk".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(90.0, 90.0, 3.0, 5.0, 5.0)],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    let c1 = project
        .compute_station_raw_coords(StationRef::new(sec_id, 1))
        .unwrap();
    assert!(
        (c1.x - 3.0).abs() < 1e-9,
        "station 1 x should be 3.0 (due east), got {}",
        c1.x
    );
    assert!(
        (c1.y).abs() < 1e-9,
        "station 1 y should be ~0, got {}",
        c1.y
    );
}

#[test]
fn compute_raw_coords_depth_independent_of_xy() {
    // Z is computed from absolute depth values, not from XY walk
    // Two shots: shot 0 goes north 10m at depth 5→8, shot 1 goes east 10m at depth 7→12
    let mut project = Project::new("IndepZ".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            shot(0.0, 0.0, 10.0, 5.0, 8.0),
            shot(90.0, 90.0, 10.0, 7.0, 12.0),
        ],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    // Station 0: only outgoing shot → depth_in = 5.0
    let c0 = project
        .compute_station_raw_coords(StationRef::new(sec_id, 0))
        .unwrap();
    assert!(
        (c0.z - 5.0).abs() < 1e-9,
        "station 0 depth should be 5.0, got {}",
        c0.z
    );

    // Station 1: incoming depth_out=8, outgoing depth_in=7 → average = 7.5
    let c1 = project
        .compute_station_raw_coords(StationRef::new(sec_id, 1))
        .unwrap();
    assert!(
        (c1.z - 7.5).abs() < 1e-9,
        "station 1 depth should be 7.5, got {}",
        c1.z
    );

    // Station 2: only incoming → depth_out = 12.0
    let c2 = project
        .compute_station_raw_coords(StationRef::new(sec_id, 2))
        .unwrap();
    assert!(
        (c2.z - 12.0).abs() < 1e-9,
        "station 2 depth should be 12.0, got {}",
        c2.z
    );
}

#[test]
fn compute_raw_coords_with_section_offset() {
    let mut project = Project::new("Offset".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 10.0, 10.0)],
            ..Default::default()
        })
        .id;

    project.section_offsets.insert(sec, (100.0, 200.0));

    let c0 = project
        .compute_station_raw_coords(StationRef::new(sec, 0))
        .unwrap();
    assert!(
        (c0.x - 100.0).abs() < 1e-9,
        "x should include section offset"
    );
    assert!(
        (c0.y - 200.0).abs() < 1e-9,
        "y should include section offset"
    );
}

#[test]
fn compute_raw_coords_with_station_offset() {
    let mut project = Project::new("StaOff".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 10.0, 10.0)],
            ..Default::default()
        })
        .id;

    project
        .station_offsets
        .insert(StationRef::new(sec, 0).into(), (1.0, 2.0, 3.0));

    let c0 = project
        .compute_station_raw_coords(StationRef::new(sec, 0))
        .unwrap();
    assert!((c0.x - 1.0).abs() < 1e-9, "x should include station offset");
    assert!((c0.y - 2.0).abs() < 1e-9, "y should include station offset");
    assert!(
        (c0.z - 13.0).abs() < 1e-9,
        "z should be depth(10) + offset(3)"
    );
}
