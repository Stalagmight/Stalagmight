//! Penetration, distance fields and reachability.
//!
//! Lines 5393-5746 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

#[test]
fn calculate_penetration_with_zero_datum() {
    let mut project = Project::new("Penetration".to_string(), None);

    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Set a zero datum on station 0
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(section_id, 0).into(),
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });

    // Compute stations
    let stations = project.compute_station_coordinates();

    // Station 0 at origin, station 1 at (0, 10, 0), station 2 at (0, 15, 0)
    // Penetration = max distance from datum station to any station = 15.0
    let penetration = project.calculate_penetration(&stations);
    assert!(
        (penetration - 15.0).abs() < 1e-6,
        "Expected penetration ~15.0, got {penetration}"
    );
}

#[test]
fn calculate_penetration_no_datums_returns_zero() {
    let mut project = Project::new("NoDatums".to_string(), None);

    project.add_section(SectionData {
        shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
        ..Default::default()
    });

    // No zero datums set
    let stations = project.compute_station_coordinates();
    let penetration = project.calculate_penetration(&stations);
    assert!(
        (penetration - 0.0).abs() < 1e-6,
        "Expected penetration 0.0, got {penetration}"
    );
}

#[test]
fn compute_distances_from_chosen_station() {
    // Generalized API rooted at an arbitrary (non-datum) station: the gradient's
    // click-to-pick case. Chain of 3 stations at y = 0, 10, 15.
    let mut project = Project::new("DistFromStation".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let stations = project.compute_station_coordinates();
    let idx = |i: u32| {
        stations
            .iter()
            .position(|s| s.station_ref == StationRef::new(section_id, i))
            .unwrap()
    };

    // Root at the middle station (index 1) - no zero datum needed: distances are
    // 10 (to station 0), 0 (self), 5 (to station 2).
    let dist = project.compute_distances_from(&stations, &[idx(1)]);
    assert!((dist[idx(0)] - 10.0).abs() < 1e-6, "got {}", dist[idx(0)]);
    assert!((dist[idx(1)] - 0.0).abs() < 1e-6, "got {}", dist[idx(1)]);
    assert!((dist[idx(2)] - 5.0).abs() < 1e-6, "got {}", dist[idx(2)]);
}

#[test]
fn compute_distances_from_empty_roots_all_infinity() {
    let mut project = Project::new("DistNoRoots".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
        ..Default::default()
    });
    let stations = project.compute_station_coordinates();
    let dist = project.compute_distances_from(&stations, &[]);
    assert_eq!(dist.len(), stations.len());
    assert!(dist.iter().all(|d| d.is_infinite()));
}

#[test]
fn compute_distances_from_zero_datum_roots_matches_penetration() {
    // The generalized API, seeded with the zero-datum indices, must reproduce
    // compute_penetration_distances exactly (penetration is the default origin).
    let mut project = Project::new("DistMatchesPen".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(section_id, 0).into(),
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });

    let stations = project.compute_station_coordinates();
    let datum_idx = stations
        .iter()
        .position(|s| s.station_ref == StationRef::new(section_id, 0))
        .unwrap();

    let pen = project.compute_penetration_distances(&stations);
    let from = project.compute_distances_from(&stations, &[datum_idx]);
    assert_eq!(pen.len(), from.len());
    for (a, b) in pen.iter().zip(from.iter()) {
        assert!(
            (a - b).abs() < 1e-6 || (a.is_infinite() && b.is_infinite()),
            "penetration={a} distances_from={b}"
        );
    }
}

#[test]
fn reachability_keeps_jump_reference_shots() {
    // A jump/reference shot is a real measured leg: a virtual shot that is
    // the only link to a tail station keeps it reachable (it's included in the
    // graph like any other shot).
    let mut project = Project::new("JumpKept".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // permanent A0->A1
                shot(0.0, 0.0, 5.0, 0.0, 0.0),  // marked reference A1->A2
            ],
            ..Default::default()
        })
        .id;
    project
        .sections
        .iter_mut()
        .find(|s| s.id == section_id)
        .unwrap()
        .shots[1]
        .kind = ShotKind::Jump;

    let stations = project.compute_station_coordinates();
    let idx = |i: u32| {
        stations
            .iter()
            .position(|s| s.station_ref == StationRef::new(section_id, i))
            .unwrap()
    };
    // Root at A0: A2 is reachable only across the reference shot → kept as a jump.
    let dist = project.compute_distances_from(&stations, &[idx(0)]);
    assert!(
        (dist[idx(2)] - 15.0).abs() < 1e-6,
        "jump must keep A2 reachable at 15.0, got {}",
        dist[idx(2)]
    );
}

#[test]
fn reachability_uses_jump_shot_shortcut() {
    // A JUMP is a swum connection, so it stays in the graph: B's jump links
    // B0-B1 (len 5). Even though B0/B1 are ALSO connected the long way through
    // section A (A0-A1-A2 = 20), shortest-path takes the direct jump (5). This
    // is 88ccbaff's regression guard (#34); #27 narrowed it from "every virtual
    // shot" to "jumps only", and the Reference half is pinned by the companion
    // test below.
    let mut project = Project::new("JumpShortcut".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // A0->A1
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // A1->A2
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)], // B0->B1
            ..Default::default()
        })
        .id;
    project
        .sections
        .iter_mut()
        .find(|s| s.id == b)
        .unwrap()
        .shots[0]
        .kind = ShotKind::Jump;

    // Tie B's endpoints onto A's chain so B0-B1 has an alternate route: B0<->A0
    // via a merge, B1<->A2 via an aligned loop closure (merging an already-
    // connected pair is rejected - cycles must be closures).
    project
        .merge_stations(StationRef::new(b, 0), StationRef::new(a, 0))
        .unwrap();
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(a, 2).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: true,
    });

    let stations = project.compute_station_coordinates();
    let pos = |sec, i| {
        stations
            .iter()
            .position(|s| s.station_ref == StationRef::new(sec, i))
            .unwrap()
    };
    let dist = project.compute_distances_from(&stations, &[pos(b, 0)]);
    assert!(
        (dist[pos(b, 1)] - 5.0).abs() < 1e-6,
        "jump shortcut must be used (B1 direct = 5, not 20 via A), got {}",
        dist[pos(b, 1)]
    );

    // Flipping the SAME fixture to Reference must route the long way instead:
    // a reference tie is a tape measurement, not a swimmable passage. This is
    // the false-shortcut risk #77 left open, now closed by #27.
    project
        .sections
        .iter_mut()
        .find(|s| s.id == b)
        .unwrap()
        .shots[0]
        .kind = ShotKind::Reference;
    let dist = project.compute_distances_from(&stations, &[pos(b, 0)]);
    assert!(
        (dist[pos(b, 1)] - 20.0).abs() < 1e-6,
        "a reference tie is not swimmable, so the route must go the long way \
         via A (20), got {}",
        dist[pos(b, 1)]
    );

    // ...unless the user opts references back in, which restores the old
    // inclusive behaviour exactly (#27 q13).
    let dist = project.compute_distances_from_with(&stations, &[pos(b, 0)], true);
    assert!(
        (dist[pos(b, 1)] - 5.0).abs() < 1e-6,
        "include_reference_legs must restore the pre-#27 shortcut, got {}",
        dist[pos(b, 1)]
    );
}

#[test]
fn reachability_includes_non_aligned_closures() {
    // A non-aligned (e.g. Batch) loop closure is still a real physical connection.
    // Section B is reachable from A's zero datum ONLY across a non-aligned closure
    // - it must NOT show as unreachable. (#34: areas connected via closure)
    let mut project = Project::new("NonAlignedClosure".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)], // A0->A1
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 7.0, 0.0, 0.0)], // B0->B1, disconnected from A
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });
    // A1 <-> B0 via a NON-aligned closure (is_aligned = false).
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    let stations = project.compute_station_coordinates();
    let field = project.distance_field_from(&stations, None); // from the zero datum
    let pos = |sec, i| {
        stations
            .iter()
            .position(|s| s.station_ref == StationRef::new(sec, i))
            .unwrap()
    };
    // A0=0, A1=10, B0=10 (closure, 0-cost), B1=17 - all finite (reachable).
    assert_eq!(
        field.distances[pos(b, 0)],
        Some(10.0),
        "B0 reachable via closure"
    );
    assert_eq!(
        field.distances[pos(b, 1)],
        Some(17.0),
        "B1 reachable via closure"
    );
}

#[test]
fn distance_field_uses_coordinate_distances() {
    // #2 (#34): edge weights come from the station coordinates passed in, so the
    // gradient's distances reflect the (closure-adjusted) geometry on the map,
    // not the raw measured shot length.
    let mut project = Project::new("DistCoords".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });

    let mut stations = project.compute_station_coordinates();
    let i1 = stations
        .iter()
        .position(|s| s.station_ref == StationRef::new(a, 1))
        .unwrap();
    // Baseline: a1 sits 10 m out (matches the measured shot length).
    assert_eq!(
        project.distance_field_from(&stations, None).distances[i1],
        Some(10.0)
    );
    // Move a1 to 25 m out → the distance follows the coordinate, not the length.
    stations[i1].coordinates = Coordinates::new(0.0, 25.0, 0.0);
    assert_eq!(
        project.distance_field_from(&stations, None).distances[i1],
        Some(25.0)
    );
}
