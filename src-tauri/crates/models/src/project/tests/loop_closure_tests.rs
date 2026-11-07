//! Loop closure creation and validation, its interplay with merge and unmerge, and weighted path
//! length.
//!
//! Lines 2542-3272 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

// === Loop closure tests ===

#[test]
fn create_loop_closure_creates_correct_values() {
    let mut project = Project::new("ClosureTest".to_string(), None);

    // Section A: shot goes north 10m
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 1.0)],
            ..Default::default()
        })
        .id;

    // Section B: shot goes east 5m
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge B.0 to A.1 so they're connected
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // Create closure between A.0 and B.1 (same-section closures require same connected group)
    let closure = project
        .create_loop_closure(
            StationRef::new(a, 0),
            StationRef::new(b, 1),
            ClosureStrategy::Batch,
        )
        .unwrap();

    assert!(project.key_is_ref(&closure.station_a, StationRef::new(a, 0)));
    assert!(project.key_is_ref(&closure.station_b, StationRef::new(b, 1)));

    // Path length should be sum of shot lengths: 10m + 5m = 15m
    assert!(
        (closure.path_length - 15.0).abs() < 0.1,
        "Path length should be ~15m (10+5), got {}",
        closure.path_length
    );

    // Misclosure is Euclidean distance between A.0 and B.1
    assert!(
        closure.misclosure_distance > 0.0,
        "Misclosure should be positive for non-coincident stations"
    );

    // Verify error_percentage formula: (misclosure / path_length) * 100
    let expected_pct = (closure.misclosure_distance / closure.path_length) * 100.0;
    assert!(
        (closure.error_percentage - expected_pct).abs() < 1e-6,
        "error_percentage should equal misclosure/path_length*100: expected {}, got {}",
        expected_pct,
        closure.error_percentage
    );

    // Horizontal + vertical misclosure should be consistent with total misclosure
    // (total >= horizontal because total includes vertical component)
    assert!(
        closure.misclosure_distance >= closure.horizontal_misclosure,
        "Total misclosure {} should be >= horizontal {}",
        closure.misclosure_distance,
        closure.horizontal_misclosure
    );
    assert!(closure.vertical_misclosure >= 0.0);

    assert_eq!(project.loop_closures.len(), 1);
}

#[test]
fn create_loop_closures_batch_matches_sequential() {
    // The batch path must be byte-for-byte equivalent to calling
    // create_loop_closure(.., Batch) for each pair in order - same created
    // closures (ids, keys, metrics) and same per-pair Ok/Err outcomes.
    fn build() -> (Project, Vec<(StationRef, StationRef)>) {
        let mut p = Project::new("Batch".to_string(), None);
        let a = p
            .add_section(SectionData {
                shots: vec![
                    shot(0.0, 0.0, 10.0, 0.0, 0.0),
                    shot(0.0, 0.0, 10.0, 0.0, 0.0),
                ],
                ..Default::default()
            })
            .id;
        let b = p
            .add_section(SectionData {
                shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id;
        let c = p
            .add_section(SectionData {
                shots: vec![shot(180.0, 180.0, 8.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id;
        let d = p
            .add_section(SectionData {
                shots: vec![shot(270.0, 270.0, 3.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id;
        // Tree of merges connecting all four sections.
        p.merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
            .unwrap();
        p.merge_stations(StationRef::new(a, 2), StationRef::new(c, 0))
            .unwrap();
        p.merge_stations(StationRef::new(b, 1), StationRef::new(d, 0))
            .unwrap();
        let pairs = vec![
            (StationRef::new(a, 0), StationRef::new(c, 1)), // valid loop
            (StationRef::new(a, 1), StationRef::new(b, 0)), // already merged → skip
            (StationRef::new(a, 0), StationRef::new(c, 1)), // duplicate → skip
            (StationRef::new(d, 1), StationRef::new(c, 1)), // valid (connected via merges)
        ];
        (p, pairs)
    }

    let (mut seq, pairs) = build();
    // Clone so both runs share identical shot UUIDs (Section::new mints random
    // ones), making the stored AfterShot(uuid) keys directly comparable.
    let mut bat = seq.clone();
    let seq_results: Vec<_> = pairs
        .iter()
        .map(|&(a, b)| seq.create_loop_closure(a, b, ClosureStrategy::Batch))
        .collect();
    let bat_results = bat.create_loop_closures_batch(&pairs);

    assert_eq!(
        seq.loop_closures.len(),
        bat.loop_closures.len(),
        "created-closure count differs (seq {} vs batch {})",
        seq.loop_closures.len(),
        bat.loop_closures.len()
    );
    for (s, t) in seq.loop_closures.iter().zip(&bat.loop_closures) {
        assert_eq!(s.id, t.id, "closure id differs");
        assert_eq!(s.station_a, t.station_a, "station_a key differs");
        assert_eq!(s.station_b, t.station_b, "station_b key differs");
        assert!(
            (s.path_length - t.path_length).abs() < 1e-9,
            "path_length differs: {} vs {}",
            s.path_length,
            t.path_length
        );
        assert!(
            (s.misclosure_distance - t.misclosure_distance).abs() < 1e-9,
            "misclosure differs"
        );
        assert!(
            (s.error_percentage - t.error_percentage).abs() < 1e-9,
            "error_percentage differs"
        );
        assert_eq!(s.is_aligned, t.is_aligned);
    }
    assert_eq!(seq_results.len(), bat_results.len());
    for (s, t) in seq_results.iter().zip(&bat_results) {
        assert_eq!(
            s.is_ok(),
            t.is_ok(),
            "per-pair outcome differs: seq {s:?} vs batch {t:?}"
        );
    }
    // Sanity: exactly the two valid pairs were created.
    assert_eq!(bat.loop_closures.len(), 2);
}

#[test]
fn create_loop_closures_batch_matches_sequential_zero_datum_unconnected() {
    // Covers the order-dependent branch the main equivalence test omits: two
    // sections that are NOT merge-connected but BOTH carry a zero datum. That is
    // the only path that accepts a closure between an unconnected pair
    // (`!connected && a_has_zd && b_has_zd`), and it must stay byte-for-byte
    // equivalent between the sequential and batch paths.
    fn build() -> (Project, Vec<(StationRef, StationRef)>) {
        let mut p = Project::new("BatchZD".to_string(), None);
        let a = p
            .add_section(SectionData {
                shots: vec![
                    shot(0.0, 0.0, 10.0, 0.0, 0.0),
                    shot(0.0, 0.0, 10.0, 0.0, 0.0),
                ],
                ..Default::default()
            })
            .id;
        let b = p
            .add_section(SectionData {
                shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id;
        // Two disconnected sections (no merge between them), each with its own
        // zero datum - the only setup that accepts an unconnected closure pair.
        p.set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();
        p.set_zero_datum(
            StationRef::new(b, 0),
            GpsCoordinates::new(46.0, -91.0, 0.0).unwrap(),
        )
        .unwrap();
        let pairs = vec![
            (StationRef::new(a, 1), StationRef::new(b, 1)), // unconnected but both datum'd → created
            (StationRef::new(a, 1), StationRef::new(b, 1)), // duplicate → skip
        ];
        (p, pairs)
    }

    let (mut seq, pairs) = build();
    // Clone so both runs share identical shot UUIDs (Section::new mints random
    // ones), making the stored AfterShot(uuid) keys directly comparable.
    let mut bat = seq.clone();
    let seq_results: Vec<_> = pairs
        .iter()
        .map(|&(a, b)| seq.create_loop_closure(a, b, ClosureStrategy::Batch))
        .collect();
    let bat_results = bat.create_loop_closures_batch(&pairs);

    assert_eq!(
        seq.loop_closures.len(),
        bat.loop_closures.len(),
        "created-closure count differs (seq {} vs batch {})",
        seq.loop_closures.len(),
        bat.loop_closures.len()
    );
    for (s, t) in seq.loop_closures.iter().zip(&bat.loop_closures) {
        assert_eq!(s.id, t.id, "closure id differs");
        assert_eq!(s.station_a, t.station_a, "station_a key differs");
        assert_eq!(s.station_b, t.station_b, "station_b key differs");
        assert!(
            (s.path_length - t.path_length).abs() < 1e-9,
            "path_length differs: {} vs {}",
            s.path_length,
            t.path_length
        );
        assert!(
            (s.misclosure_distance - t.misclosure_distance).abs() < 1e-9,
            "misclosure differs"
        );
        assert!(
            (s.error_percentage - t.error_percentage).abs() < 1e-9,
            "error_percentage differs"
        );
        assert_eq!(s.is_aligned, t.is_aligned);
    }
    assert_eq!(seq_results.len(), bat_results.len());
    for (s, t) in seq_results.iter().zip(&bat_results) {
        assert_eq!(
            s.is_ok(),
            t.is_ok(),
            "per-pair outcome differs: seq {s:?} vs batch {t:?}"
        );
    }
    // Sanity: the unconnected-but-datum'd pair was accepted exactly once.
    assert_eq!(bat.loop_closures.len(), 1);
}

#[test]
fn create_loop_closure_proportional_is_deferred() {
    let mut project = Project::new("ClosureDist".to_string(), None);

    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let station_a = StationRef::new(section_id, 0);
    let station_b = StationRef::new(section_id, 2);

    let coords_a_before = project.compute_station_raw_coords(station_a).unwrap();
    let coords_b_before = project.compute_station_raw_coords(station_b).unwrap();

    let result =
        project.create_loop_closure(station_a, station_b, ClosureStrategy::ProportionalByLength);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("deferred"));
    assert!(project.loop_closures.is_empty());

    let coords_a_after = project.compute_station_raw_coords(station_a).unwrap();
    let coords_b_after = project.compute_station_raw_coords(station_b).unwrap();
    assert!(coords_a_before.distance_to(&coords_a_after) < 1e-6);
    assert!(coords_b_before.distance_to(&coords_b_after) < 1e-6);
}

#[test]
fn create_loop_closure_equal_distribution_is_deferred() {
    let mut project = Project::new("ClosureFallback".to_string(), None);

    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let station_a = StationRef::new(section_id, 0);
    let station_b = StationRef::new(section_id, 2);

    let result =
        project.create_loop_closure(station_a, station_b, ClosureStrategy::EqualDistribution);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("deferred"));
    assert!(project.loop_closures.is_empty());
}

#[test]
fn create_loop_closure_rejects_duplicate() {
    let mut project = Project::new("DupClosure".to_string(), None);

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

    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // First closure succeeds
    project
        .create_loop_closure(
            StationRef::new(a, 0),
            StationRef::new(b, 1),
            ClosureStrategy::default(),
        )
        .unwrap();

    // Duplicate should fail
    let result = project.create_loop_closure(
        StationRef::new(a, 0),
        StationRef::new(b, 1),
        ClosureStrategy::default(),
    );
    assert!(result.is_err(), "Duplicate closure should be rejected");
    assert!(result.unwrap_err().contains("already exists"));

    // Reversed order should also fail
    let result = project.create_loop_closure(
        StationRef::new(b, 1),
        StationRef::new(a, 0),
        ClosureStrategy::default(),
    );
    assert!(
        result.is_err(),
        "Reversed duplicate closure should be rejected"
    );
}

#[test]
fn create_loop_closure_allows_same_section() {
    let mut project = Project::new("SameSectionClosure".to_string(), None);

    // A section with 3 shots - all stations are connected within the section
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

    // Stations 0 and 3 are in the same section and connected through shots
    let result = project.create_loop_closure(
        StationRef::new(a, 0),
        StationRef::new(a, 3),
        ClosureStrategy::default(),
    );
    assert!(
        result.is_ok(),
        "Same-section closure should be allowed: {:?}",
        result.err()
    );
    assert_eq!(project.loop_closures.len(), 1);
}

#[test]
fn create_loop_closure_rejects_unconnected_stations() {
    let mut project = Project::new("UnconnectedClosure".to_string(), None);

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

    // Don't merge them - they are unconnected
    let result = project.create_loop_closure(
        StationRef::new(a, 0),
        StationRef::new(b, 0),
        ClosureStrategy::default(),
    );
    assert!(result.is_err(), "Unconnected stations should be rejected");
    assert!(result.unwrap_err().contains("not connected"));
}

#[test]
fn merge_stations_rejects_already_connected() {
    let mut project = Project::new("ConnectedMerge".to_string(), None);

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

    // First merge succeeds
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // Second merge between connected sections should fail
    let result = project.merge_stations(StationRef::new(a, 0), StationRef::new(b, 1));
    assert!(
        result.is_err(),
        "Merging already-connected stations should be rejected"
    );
    assert!(result.unwrap_err().contains("already connected"));
}

#[test]
fn unmerge_stations_removes_matching_closure_and_preserves_others() {
    let mut project = Project::new("UnmergePair".to_string(), None);

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

    // Merge A-B and B-C
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(b, 1), StationRef::new(c, 0))
        .unwrap();

    // Inject a closure matching the A-B merge pair directly.
    // (create_loop_closure won't accept stations that are the merge pair itself)
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        path_length: 10.0,
        misclosure_distance: 0.2,
        error_percentage: 2.0,
        horizontal_misclosure: 0.15,
        vertical_misclosure: 0.05,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    // Also inject an unrelated closure for B-C pair
    project.loop_closures.push(LoopClosure {
        id: 2,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 3.0,
        misclosure_distance: 0.1,
        error_percentage: 3.3,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    assert_eq!(project.loop_closures.len(), 2);

    // Unmerge A-B: should remove the A.1-B.0 closure but keep B.1-C.0
    let (success, removed) = project.unmerge_stations(StationRef::new(a, 1), StationRef::new(b, 0));
    assert!(success);
    assert_eq!(
        removed.len(),
        1,
        "Should remove exactly 1 closure matching the merge pair"
    );
    assert!(project.key_is_ref(&removed[0].station_a, StationRef::new(a, 1)));
    assert!(project.key_is_ref(&removed[0].station_b, StationRef::new(b, 0)));

    // Only the B-C closure should remain
    assert_eq!(project.loop_closures.len(), 1);
    assert!(project.key_is_ref(&project.loop_closures[0].station_a, StationRef::new(b, 1)));
    assert!(project.key_is_ref(&project.loop_closures[0].station_b, StationRef::new(c, 0)));
}

#[test]
fn unmerge_stations_preserves_unrelated_closures() {
    let mut project = Project::new("UnmergePreserve".to_string(), None);

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

    // Merge A-B and B-C
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(b, 1), StationRef::new(c, 0))
        .unwrap();

    // Create closure using B-C pair stations (b.1, c.0)
    // This is a direct closure at the merge point
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 3.0,
        misclosure_distance: 0.1,
        error_percentage: 3.3,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    assert_eq!(project.loop_closures.len(), 1);

    // Unmerge A-B: should NOT remove the B.1-C.0 closure (different pair)
    let (success, removed) = project.unmerge_stations(StationRef::new(a, 1), StationRef::new(b, 0));
    assert!(success);
    assert!(
        removed.is_empty(),
        "No closures should be removed for unrelated pair"
    );
    assert_eq!(
        project.loop_closures.len(),
        1,
        "B-C closure should be preserved"
    );
}

#[test]
fn closure_expands_connected_sections_with_existing_merges() {
    // Test: A closure between stations in different sections makes those sections
    // connected (just like a merge). Existing merges are also traversed.
    let mut project = Project::new("ClosureExpands".to_string(), None);
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

    // Merge A-B
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Closure B-C (no merge, just a closure)
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 5.0,
        misclosure_distance: 0.1,
        error_percentage: 2.0,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.next_closure_id += 1;

    // Merge-only connectivity: A and B are merged, C is NOT in the merge group
    let merged = project.get_merged_section_ids(a);
    assert!(merged.contains(&a), "A should be in merge group");
    assert!(merged.contains(&b), "B should be in merge group");
    assert!(
        !merged.contains(&c),
        "C should NOT be in merge group (only closure-linked)"
    );
    assert_eq!(merged.len(), 2);

    // Full connectivity (merges + closures): all three sections are connected
    let reachable = project.get_connected_section_ids(a);
    assert!(reachable.contains(&a), "A should be reachable");
    assert!(reachable.contains(&b), "B should be reachable");
    assert!(reachable.contains(&c), "C should be reachable via closure");
    assert_eq!(reachable.len(), 3);

    // Path length including closures: BFS traverses closure as zero-distance edge
    let path = project.calculate_path_length_between(StationRef::new(a, 0), StationRef::new(c, 1));
    // A.0 -> A.1 (1m shot) -> B.0 (merge, 0) -> B.1 (1m shot) -> C.0 (closure, 0) -> C.1 (1m shot)
    assert!(
        (path - 3.0).abs() < 1e-6,
        "Path A.0 to C.1 should be 3.0m (shots only, merge+closure are zero-distance), got {path}"
    );

    // Merge-only path length: cannot reach C.1 from A.0 without closures
    // Falls back to sum of merged section lengths
    let merge_only_path =
        project.calculate_merge_only_path_length(StationRef::new(a, 0), StationRef::new(c, 1));
    // A and B are merge-connected (total 2m), C is not reachable without closures
    // Fallback returns sum of A+B shots = 2.0
    assert!(
        (merge_only_path - 2.0).abs() < 1e-6,
        "Merge-only path should fall back to merged section sum (2.0m), got {merge_only_path}"
    );

    // are_stations_in_closure should detect the closure
    assert!(project.are_stations_in_closure(StationRef::new(b, 1), StationRef::new(c, 0),));
    // Reverse direction also works
    assert!(project.are_stations_in_closure(StationRef::new(c, 0), StationRef::new(b, 1),));
    // Non-closure pair returns false
    assert!(!project.are_stations_in_closure(StationRef::new(a, 0), StationRef::new(c, 0),));
}

#[test]
fn calculate_path_length_prefers_shorter_weighted_path() {
    // Two paths between A.0 and A.1:
    // 1) Direct shot length 100
    // 2) Merge to B.0, two shots of length 1, merge back at B.2 (total 2)
    let mut project = Project::new("ShortestPath".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 100.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 1.0, 0.0, 0.0),
                shot(90.0, 90.0, 1.0, 0.0, 0.0),
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
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 2).into(),
        merged_at: Utc::now(),
    });

    let path = project.calculate_path_length_between(StationRef::new(a, 0), StationRef::new(a, 1));
    assert!(
        (path - 2.0).abs() < 1e-6,
        "Path should prefer length 2.0 over direct 100.0, got {path}"
    );
}
