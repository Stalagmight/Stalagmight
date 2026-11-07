//! Connectivity through merges and loop closures (transitive closure).
//!
//! Lines 3819-4657 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

#[test]
fn new_merge_expands_existing_closure_connectivity() {
    // Test: Adding a merge after a closure exists should expand connectivity
    // Setup: A-B closure, then merge B-C -> A should connect to C
    let mut project = Project::new("MergeExpandsClosure".to_string(), None);
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

    // Closure A-B (before any merges)
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        path_length: 5.0,
        misclosure_distance: 0.1,
        error_percentage: 2.0,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.next_closure_id += 1;

    // Before merge, A and B are reachable via closure (but not merge-connected)
    let merged_before = project.get_merged_section_ids(a);
    assert!(merged_before.contains(&a));
    assert!(!merged_before.contains(&b), "B not merge-connected to A");
    let connected_before = project.get_connected_section_ids(a);
    assert!(connected_before.contains(&a));
    assert!(connected_before.contains(&b), "B connected via closure");
    assert!(!connected_before.contains(&c), "C not yet connected");

    // Now merge B-C
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    // After merge, A should reach C via closure(A-B) + merge(B-C)
    let connected_after = project.get_connected_section_ids(a);
    assert!(connected_after.contains(&a), "A connected");
    assert!(connected_after.contains(&b), "B connected via closure");
    assert!(
        connected_after.contains(&c),
        "C connected via closure+merge"
    );
    assert_eq!(connected_after.len(), 3);

    // Merge-only: A is alone (no merges from A), B and C are merge-connected
    let merged_a = project.get_merged_section_ids(a);
    assert_eq!(merged_a.len(), 1, "A has no merges");
    let merged_b = project.get_merged_section_ids(b);
    assert!(merged_b.contains(&b));
    assert!(merged_b.contains(&c));
    assert_eq!(merged_b.len(), 2, "B and C are merge-connected");

    // Path including closures should traverse both closure and merge with zero distance
    let path = project.calculate_path_length_between(StationRef::new(a, 0), StationRef::new(c, 1));
    // A.0 -> A.1 (1m) -> B.0 (closure, 0) -> B.1 (1m) -> C.0 (merge, 0) -> C.1 (1m)
    assert!(
        (path - 3.0).abs() < 1e-6,
        "Path should be 3.0m (shots only), got {path}"
    );
}

#[test]
fn transitive_closure_computation_correct() {
    // Test: closures_with is computed correctly with transitive relationships
    // Setup: A.1 merged with B.0, closure between A.1 and C.0
    // Expected: B.0 gets transitive closure to C.0 (via A.1)
    //           C.0 gets transitive closure to B.0 (via A.1)
    let mut project = Project::new("TransitiveClosure".to_string(), None);
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
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A.1 with B.0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Closure A.1 to C.0
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 8.0,
        misclosure_distance: 0.2,
        error_percentage: 2.5,
        horizontal_misclosure: 0.2,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    let closure_id = project.next_closure_id;
    project.next_closure_id += 1;

    // Set datum so compute_station_coordinates works
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    let stations = project.compute_station_coordinates();

    // A.1: direct closure to C.0 (not transitive, A.1 is the endpoint)
    let a1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 1))
        .unwrap();
    let a1_direct = a1
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(c, 0) && !cr.is_transitive);
    assert!(a1_direct.is_some(), "A.1 should have direct closure to C.0");
    assert_eq!(a1_direct.unwrap().closure_id, closure_id);
    assert!(a1_direct.unwrap().via_station.is_none());

    // B.0: transitive closure to C.0 via A.1 (B.0 is merged with A.1)
    let b0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 0))
        .unwrap();
    let b0_transitive = b0
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(c, 0) && cr.is_transitive);
    assert!(
        b0_transitive.is_some(),
        "B.0 should have transitive closure to C.0 via A.1"
    );
    assert_eq!(
        b0_transitive.unwrap().via_station,
        Some(StationRef::new(a, 1))
    );

    // C.0: direct closure to A.1 (C.0 is the endpoint)
    let c0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 0))
        .unwrap();
    let c0_direct = c0
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(a, 1) && !cr.is_transitive);
    assert!(c0_direct.is_some(), "C.0 should have direct closure to A.1");

    // C.0: transitive closure to B.0 via A.1 (B.0 is merged with A.1)
    let c0_transitive = c0
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(b, 0) && cr.is_transitive);
    assert!(
        c0_transitive.is_some(),
        "C.0 should have transitive closure to B.0 via A.1"
    );
    assert_eq!(
        c0_transitive.unwrap().via_station,
        Some(StationRef::new(a, 1))
    );
}

#[test]
fn transitive_closure_both_endpoints_merged() {
    // Test: both closure endpoints are in different merge groups
    // Setup: A.1↔B.0 merge, B.1↔C.0 merge, closure between A.1 and C.0
    // Expected: B.0 gets transitive to C.0 (via A.1)
    //           B.1 gets transitive to A.1 (via C.0)
    //           A.1 gets transitive to B.1 (via C.0) - B.1 is in C.0's group
    //           C.0 gets transitive to B.0 (via A.1) - B.0 is in A.1's group
    let mut project = Project::new("TransitiveBothMerged".to_string(), None);
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
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A.1 ↔ B.0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Merge B.1 ↔ C.0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    // Closure A.1 ↔ C.0 (both endpoints are in merge groups)
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 8.0,
        misclosure_distance: 0.2,
        error_percentage: 2.5,
        horizontal_misclosure: 0.2,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    let closure_id = project.next_closure_id;
    project.next_closure_id += 1;

    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    let stations = project.compute_station_coordinates();

    // A.1: direct closure to C.0
    let a1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 1))
        .unwrap();
    assert!(
        a1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(c, 0)
                && !cr.is_transitive
                && cr.closure_id == closure_id),
        "A.1 should have direct closure to C.0, got: {:?}",
        a1.closures()
    );
    // A.1: transitive closure to B.1 (via C.0, since B.1 is in C.0's group)
    assert!(
        a1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(b, 1)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(c, 0))),
        "A.1 should have transitive closure to B.1 (via C.0), got: {:?}",
        a1.closures()
    );

    // B.0: transitive closure to C.0 (via A.1, since B.0 is in A.1's group)
    let b0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 0))
        .unwrap();
    assert!(
        b0.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(c, 0)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(a, 1))),
        "B.0 should have transitive closure to C.0 (via A.1), got: {:?}",
        b0.closures()
    );
    // B.0: transitive closure to B.1 (via C.0)
    assert!(
        b0.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(b, 1)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(c, 0))),
        "B.0 should have transitive closure to B.1 (via C.0), got: {:?}",
        b0.closures()
    );

    // C.0: direct closure to A.1
    let c0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 0))
        .unwrap();
    assert!(
        c0.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(a, 1)
                && !cr.is_transitive
                && cr.closure_id == closure_id),
        "C.0 should have direct closure to A.1, got: {:?}",
        c0.closures()
    );
    // C.0: transitive closure to B.0 (via A.1)
    assert!(
        c0.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(b, 0)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(a, 1))),
        "C.0 should have transitive closure to B.0 (via A.1), got: {:?}",
        c0.closures()
    );

    // B.1: transitive closure to A.1 (via C.0, since B.1 is in C.0's group)
    let b1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 1))
        .unwrap();
    assert!(
        b1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(a, 1)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(c, 0))),
        "B.1 should have transitive closure to A.1 (via C.0), got: {:?}",
        b1.closures()
    );
    // B.1: transitive closure to B.0 (via A.1)
    assert!(
        b1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(b, 0)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(a, 1))),
        "B.1 should have transitive closure to B.0 (via A.1), got: {:?}",
        b1.closures()
    );

    // A.0 and C.1 should have NO closures (they're not closure endpoints or merged with one)
    let a0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 0))
        .unwrap();
    assert!(
        a0.closures().is_empty(),
        "A.0 should have no closures, got: {:?}",
        a0.closures()
    );

    let c1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 1))
        .unwrap();
    assert!(
        c1.closures().is_empty(),
        "C.1 should have no closures, got: {:?}",
        c1.closures()
    );
}

#[test]
fn transitive_closure_through_create_loop_closure_api() {
    // Test: using the actual create_loop_closure API (not raw push)
    // Setup: 3 sections, merge chain A.1↔B.0 and B.1↔C.0, closure A.0↔C.1
    // Expected: A.0 and C.1 get direct closures to each other
    //           No transitive closures (endpoints not in any merge group)
    let mut project = Project::new("TransitiveAPI".to_string(), None);
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
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Merge A.1 ↔ B.0 (unconnected, so merge works)
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // Merge B.1 ↔ C.0 (B and C not yet connected)
    project
        .merge_stations(StationRef::new(b, 1), StationRef::new(c, 0))
        .unwrap();

    // Now A, B, C are all connected. Create closure A.0 ↔ C.1
    let closure = project
        .create_loop_closure(
            StationRef::new(a, 0),
            StationRef::new(c, 1),
            ClosureStrategy::default(),
        )
        .unwrap();

    let stations = project.compute_station_coordinates();

    // A.0: direct closure to C.1 (endpoint, not merged with anything)
    let a0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 0))
        .unwrap();
    assert!(
        a0.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(c, 1)
                && !cr.is_transitive
                && cr.closure_id == closure.id),
        "A.0 should have direct closure to C.1, got: {:?}",
        a0.closures()
    );
    assert_eq!(
        a0.closures().len(),
        1,
        "A.0 should have exactly 1 closure ref (no transitive since A.0 not merged)"
    );

    // C.1: direct closure to A.0
    let c1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 1))
        .unwrap();
    assert!(
        c1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(a, 0)
                && !cr.is_transitive
                && cr.closure_id == closure.id),
        "C.1 should have direct closure to A.0, got: {:?}",
        c1.closures()
    );
    assert_eq!(
        c1.closures().len(),
        1,
        "C.1 should have exactly 1 closure ref"
    );

    // Other stations (A.1, B.0, B.1, C.0) should NOT have any closure refs
    // because neither closure endpoint (A.0, C.1) is in a merge group
    for (section_id, station_index, label) in
        [(a, 1, "A.1"), (b, 0, "B.0"), (b, 1, "B.1"), (c, 0, "C.0")]
    {
        let station = stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(section_id, station_index))
            .unwrap();
        assert!(
            station.closures().is_empty(),
            "{} should have no closures (endpoint not in its merge group), got: {:?}",
            label,
            station.closures()
        );
    }
}

#[test]
fn closure_chain_transitivity_two_closures_shared_endpoint() {
    // Test: closure-chain transitivity
    // Setup: 3 sections A, B, C. Merge chain: A.1↔B.0, B.1↔C.0
    //   Closure 1: A.0 ↔ B.1 (between A start and B end)
    //   Closure 2: B.1 ↔ C.1 (between B end and C end)
    //
    // B.1 has 2 direct closures: to A.0 and to C.1
    // A.0 should see transitive closure to C.1 via B.1
    // C.1 should see transitive closure to A.0 via B.1
    let mut project = Project::new("ChainTransitive".to_string(), None);

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
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Merge chain
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(b, 1), StationRef::new(c, 0))
        .unwrap();

    // Closure 1: A.0 ↔ B.1 (they're connected via merges)
    let closure1 = project
        .create_loop_closure(
            StationRef::new(a, 0),
            StationRef::new(b, 1),
            ClosureStrategy::default(),
        )
        .unwrap();

    // Closure 2: B.1 ↔ C.1 (B.1 is merged with C.0, so B and C are connected)
    // Wait - B.1 and C.0 are merged. So B.1 and C.1 are in the same section group.
    // But are_stations_merged(B.1, C.1)? No (direct check only).
    // Actually C.1 is in section C. B.1 is in section B. They're connected through merge.
    // create_loop_closure checks connectivity via get_connected_section_ids.
    let closure2 = project
        .create_loop_closure(
            StationRef::new(b, 1),
            StationRef::new(c, 1),
            ClosureStrategy::default(),
        )
        .unwrap();

    let stations = project.compute_station_coordinates();

    // B.1: should have 2 direct closures (to A.0 and to C.1)
    let b1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 1))
        .unwrap();
    let b1_to_a0 = b1
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(a, 0) && !cr.is_transitive);
    assert!(
        b1_to_a0.is_some(),
        "B.1 should have direct closure to A.0, got: {:?}",
        b1.closures()
    );
    let b1_to_c1 = b1
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(c, 1) && !cr.is_transitive);
    assert!(
        b1_to_c1.is_some(),
        "B.1 should have direct closure to C.1, got: {:?}",
        b1.closures()
    );

    // A.0: should have direct closure to B.1 AND transitive closure to C.1 (via B.1)
    let a0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 0))
        .unwrap();
    let a0_direct = a0
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(b, 1) && !cr.is_transitive);
    assert!(
        a0_direct.is_some(),
        "A.0 should have direct closure to B.1, got: {:?}",
        a0.closures()
    );
    let a0_transitive = a0
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(c, 1) && cr.is_transitive);
    assert!(
        a0_transitive.is_some(),
        "A.0 should have transitive closure to C.1 (via B.1), got: {:?}",
        a0.closures()
    );
    assert_eq!(
        a0_transitive.unwrap().via_station,
        Some(StationRef::new(b, 1)),
        "A.0 transitive to C.1 should be via B.1"
    );
    assert_eq!(
        a0_transitive.unwrap().closure_id,
        closure2.id,
        "A.0 transitive to C.1 should reference closure2's id"
    );

    // C.1: should have direct closure to B.1 AND transitive closure to A.0 (via B.1)
    let c1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 1))
        .unwrap();
    let c1_direct = c1
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(b, 1) && !cr.is_transitive);
    assert!(
        c1_direct.is_some(),
        "C.1 should have direct closure to B.1, got: {:?}",
        c1.closures()
    );
    let c1_transitive = c1
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(a, 0) && cr.is_transitive);
    assert!(
        c1_transitive.is_some(),
        "C.1 should have transitive closure to A.0 (via B.1), got: {:?}",
        c1.closures()
    );
    assert_eq!(
        c1_transitive.unwrap().via_station,
        Some(StationRef::new(b, 1)),
        "C.1 transitive to A.0 should be via B.1"
    );
    assert_eq!(
        c1_transitive.unwrap().closure_id,
        closure1.id,
        "C.1 transitive to A.0 should reference closure1's id"
    );
}

#[test]
fn closure_chain_transitivity_three_closures_shared_endpoint() {
    // Test: station with 3 closures creates transitive refs for all pairs
    // Setup: 4 sections, all connected through merges
    //   Station X has closures to A, B, and C
    //   A should see transitive to B (via X) and transitive to C (via X)
    //   B should see transitive to A (via X) and transitive to C (via X)
    //   C should see transitive to A (via X) and transitive to B (via X)
    let mut project = Project::new("ChainTriple".to_string(), None);

    let sec_main = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let sec_a = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let sec_b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let sec_c = project
        .add_section(SectionData {
            shots: vec![shot(270.0, 270.0, 4.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .set_zero_datum(
            StationRef::new(sec_main, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Connect all sections to main: main.1↔A.0, main.2↔B.0, main.3↔C.0
    project
        .merge_stations(StationRef::new(sec_main, 1), StationRef::new(sec_a, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(sec_main, 2), StationRef::new(sec_b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(sec_main, 3), StationRef::new(sec_c, 0))
        .unwrap();

    // Station main.0 (X) has closures to A.1, B.1, C.1
    let cl_a = project
        .create_loop_closure(
            StationRef::new(sec_main, 0),
            StationRef::new(sec_a, 1),
            ClosureStrategy::default(),
        )
        .unwrap();
    let cl_b = project
        .create_loop_closure(
            StationRef::new(sec_main, 0),
            StationRef::new(sec_b, 1),
            ClosureStrategy::default(),
        )
        .unwrap();
    let cl_c = project
        .create_loop_closure(
            StationRef::new(sec_main, 0),
            StationRef::new(sec_c, 1),
            ClosureStrategy::default(),
        )
        .unwrap();

    let stations = project.compute_station_coordinates();

    // X (main.0) should have 3 direct closures
    let x = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_main, 0))
        .unwrap();
    let x_directs: Vec<_> = x.closures().iter().filter(|cr| !cr.is_transitive).collect();
    assert_eq!(
        x_directs.len(),
        3,
        "X should have 3 direct closures, got: {:?}",
        x.closures()
    );

    // A.1 should have: direct to X, transitive to B.1 (via X), transitive to C.1 (via X)
    let a1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_a, 1))
        .unwrap();
    assert!(
        a1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(sec_main, 0) && !cr.is_transitive),
        "A.1 direct to X"
    );
    assert!(
        a1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(sec_b, 1)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(sec_main, 0))
                && cr.closure_id == cl_b.id),
        "A.1 transitive to B.1 via X, got: {:?}",
        a1.closures()
    );
    assert!(
        a1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(sec_c, 1)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(sec_main, 0))
                && cr.closure_id == cl_c.id),
        "A.1 transitive to C.1 via X, got: {:?}",
        a1.closures()
    );

    // B.1 should have: direct to X, transitive to A.1 (via X), transitive to C.1 (via X)
    let b1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_b, 1))
        .unwrap();
    assert!(
        b1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(sec_a, 1)
                && cr.is_transitive
                && cr.closure_id == cl_a.id),
        "B.1 transitive to A.1 via X, got: {:?}",
        b1.closures()
    );
    assert!(
        b1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(sec_c, 1)
                && cr.is_transitive
                && cr.closure_id == cl_c.id),
        "B.1 transitive to C.1 via X, got: {:?}",
        b1.closures()
    );

    // C.1 should have: direct to X, transitive to A.1 (via X), transitive to B.1 (via X)
    let c1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_c, 1))
        .unwrap();
    assert!(
        c1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(sec_a, 1)
                && cr.is_transitive
                && cr.closure_id == cl_a.id),
        "C.1 transitive to A.1 via X, got: {:?}",
        c1.closures()
    );
    assert!(
        c1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(sec_b, 1)
                && cr.is_transitive
                && cr.closure_id == cl_b.id),
        "C.1 transitive to B.1 via X, got: {:?}",
        c1.closures()
    );
}
