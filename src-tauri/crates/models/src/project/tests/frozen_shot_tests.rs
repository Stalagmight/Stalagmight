//! Frozen shots: which shots a closure or datum pins, and the edits refused on them.
//!
//! Lines 7638-7986 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

// ── Frozen shot tests ─────────────────────────────────────────────

#[test]
fn is_shot_frozen_single_datum_dead_end() {
    // Section with datum on station 0, shots going one direction.
    // No shot is frozen (dead end, no datum reachable on far side).
    let mut project = Project::new("Frozen".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 1.0, 0.0, 1.0),
                shot(0.0, 0.0, 1.0, 1.0, 2.0),
                shot(0.0, 0.0, 1.0, 2.0, 3.0),
            ],
            ..Default::default()
        })
        .id;

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(sec, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });

    let frozen = project.compute_frozen_shots();
    assert!(frozen.is_empty(), "Dead-end shots should not be frozen");
}

#[test]
fn is_shot_frozen_loop_with_two_datums() {
    // Two sections each with a datum, connected by merges at both ends.
    // All shots between the datums are frozen.
    //
    // Section A: station 0 (datum) --shot0--> station 1 --shot1--> station 2
    // Section B: station 0 (datum) --shot0--> station 1 --shot1--> station 2
    // Merges: A:0 <-> B:0, A:2 <-> B:2
    let mut project = Project::new("TwoDatums".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 1.0), shot(0.0, 0.0, 1.0, 1.0, 2.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(180.0, 180.0, 1.0, 0.0, 1.0),
                shot(180.0, 180.0, 1.0, 1.0, 2.0),
            ],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 2).into(),
        merged_at: Utc::now(),
    });

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });

    let frozen = project.compute_frozen_shots();
    // All 4 shots (2 per section) should be frozen
    assert_eq!(frozen.len(), 4, "All shots in the loop should be frozen");

    // Verify both sections' shots are present
    let a_frozen: Vec<_> = frozen.iter().filter(|f| f.section_id == a).collect();
    let b_frozen: Vec<_> = frozen.iter().filter(|f| f.section_id == b).collect();
    assert_eq!(a_frozen.len(), 2);
    assert_eq!(b_frozen.len(), 2);
}

#[test]
fn is_shot_frozen_loop_with_aligned_closure() {
    // Single datum, loop closed by an aligned closure.
    // Section A: station 0 (datum) --shot0--> station 1 --shot1--> station 2
    // Section B: station 0 --shot0--> station 1 --shot1--> station 2 --shot2--> station 3 (dead end branch)
    // Merge: A:0 <-> B:0
    // Aligned closure: A:2 <-> B:2
    // Shots A:0, A:1, B:0, B:1 should be frozen (in the loop).
    // Shot B:2 is NOT frozen (dead-end branch past the closure point).
    let mut project = Project::new("AlignedClosure".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 1.0), shot(0.0, 0.0, 1.0, 1.0, 2.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(180.0, 180.0, 1.0, 0.0, 1.0),
                shot(180.0, 180.0, 1.0, 1.0, 2.0),
                shot(180.0, 180.0, 1.0, 2.0, 3.0), // dead-end branch
            ],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });

    // Add aligned closure: A:2 <-> B:2
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 2).into(),
        path_length: 4.0,
        misclosure_distance: 0.1,
        error_percentage: 2.5,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: true,
    });
    project.next_closure_id += 1;

    let frozen = project.compute_frozen_shots();

    // A:0, A:1, B:0, B:1 should be frozen
    let a_frozen: Vec<_> = frozen.iter().filter(|f| f.section_id == a).collect();
    let b_frozen: Vec<_> = frozen.iter().filter(|f| f.section_id == b).collect();
    assert_eq!(
        a_frozen.len(),
        2,
        "Both shots in section A should be frozen"
    );
    assert_eq!(
        b_frozen.len(),
        2,
        "Only shots 0,1 in section B should be frozen"
    );

    // Verify B:2 (dead-end branch) is NOT frozen
    let b2_frozen = frozen
        .iter()
        .any(|f| f.section_id == b && f.shot_index == 2);
    assert!(!b2_frozen, "Dead-end shot B:2 should NOT be frozen");
}

#[test]
fn is_shot_frozen_unaligned_closure_not_traversed() {
    // Same topology as above but closure is NOT aligned. No shots should be frozen.
    let mut project = Project::new("UnalignedClosure".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 1.0), shot(0.0, 0.0, 1.0, 1.0, 2.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(180.0, 180.0, 1.0, 0.0, 1.0),
                shot(180.0, 180.0, 1.0, 1.0, 2.0),
            ],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });

    // Unaligned closure: A:2 <-> B:2
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 2).into(),
        path_length: 4.0,
        misclosure_distance: 0.1,
        error_percentage: 2.5,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });
    project.next_closure_id += 1;

    let frozen = project.compute_frozen_shots();
    assert!(
        frozen.is_empty(),
        "Unaligned closure should not cause shots to be frozen"
    );
}

#[test]
fn touches_geometry_detects_geometry_fields() {
    let geo_update = ShotUpdates {
        length: Some(5.0),
        ..Default::default()
    };
    assert!(geo_update.touches_geometry());

    let azimuth_update = ShotUpdates {
        azimuth_in: Some(90.0),
        ..Default::default()
    };
    assert!(azimuth_update.touches_geometry());

    let depth_update = ShotUpdates {
        depth_out: Some(3.0),
        ..Default::default()
    };
    assert!(depth_update.touches_geometry());

    let pitch_update = ShotUpdates {
        pitch_in: Some(Some(5.0)),
        ..Default::default()
    };
    assert!(pitch_update.touches_geometry());

    // Non-geometry updates
    let lrud_update = ShotUpdates {
        left: Some(Some(1.0)),
        right: Some(Some(2.0)),
        up: Some(Some(3.0)),
        down: Some(Some(4.0)),
        comment: Some(Some("test".to_string())),
        ..Default::default()
    };
    assert!(
        !lrud_update.touches_geometry(),
        "LRUD/comment should not be geometry"
    );

    let empty_update = ShotUpdates::default();
    assert!(
        !empty_update.touches_geometry(),
        "Empty update should not touch geometry"
    );
}

#[test]
fn update_shot_rejects_frozen_geometry() {
    // Create a loop with two datums so all shots are frozen
    let mut project = Project::new("FrozenUpdate".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 1.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 1.0, 0.0, 1.0)],
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
        station_b: StationRef::new(b, 1).into(),
        merged_at: Utc::now(),
    });

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });

    // Verify shots are frozen
    let frozen = project.compute_frozen_shots();
    assert!(
        !frozen.is_empty(),
        "Shots should be frozen in a loop with two datums"
    );

    // The update_shot validation is in the command handler, not on Project directly.
    // But we can verify the frozen status and touches_geometry independently.
    let shot_id = project.sections.iter().find(|s| s.id == a).unwrap().shots[0].id;
    let geo_update = ShotUpdates {
        length: Some(5.0),
        ..Default::default()
    };
    assert!(geo_update.touches_geometry());

    // Verify the shot IS frozen
    let section_lookup = project.build_section_lookup();
    let merge_adj = project.build_merge_adjacency(&section_lookup);
    let closure_adj = project.build_aligned_closure_adjacency(&section_lookup);
    let datum_stations: HashSet<StationRef> = project
        .zero_datums
        .iter()
        .filter_map(|d| project.resolve_key(&d.station_ref))
        .collect();
    assert!(project.is_shot_frozen(
        a,
        0,
        &section_lookup,
        &merge_adj,
        &closure_adj,
        &datum_stations
    ));

    // Non-geometry update should be allowed (touches_geometry returns false)
    let non_geo_update = ShotUpdates {
        comment: Some(Some("hello".to_string())),
        ..Default::default()
    };
    assert!(!non_geo_update.touches_geometry());

    // Verify the shot can still be updated with non-geometry fields
    let result = project.update_shot(a, shot_id, &non_geo_update);
    assert!(
        result.is_some(),
        "Non-geometry update on frozen shot should succeed"
    );
    assert_eq!(
        project.get_section(a).unwrap().shots[0].comment.as_deref(),
        Some("hello")
    );
}
