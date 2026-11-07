//! Multi-section merge chain operations.
//!
//! Lines 22678-23301 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// ==========================================================================
// Group 3: Multi-section merge chain operations
// ==========================================================================

/// Delete the middle section in a 3-section merge chain (A↔B↔C).
/// Verify A and C become disconnected with no dangling merge references.
#[test]
fn test_delete_middle_section_in_merge_chain() {
    let state = state_with_project("ChainDelete");

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(4.0)],
        })
        .section_id
        .unwrap();

    let sec_c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![shot(6.0)],
        })
        .section_id
        .unwrap();

    // Chain: A:1 ↔ B:0, B:1 ↔ C:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 1),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_b, 1),
                station_b: StationRef::new(sec_c, 0),
            })
            .success
    );

    // Delete section B
    assert!(
        state
            .execute(Command::DeleteSection { section_id: sec_b })
            .success
    );

    let project_after = state.get_project_state().unwrap();

    // B should be gone
    assert!(
        !project_after.sections.iter().any(|s| s.id == sec_b),
        "section B should be deleted"
    );

    // No merge should reference section B
    for merge in &project_after.station_merges {
        assert_ne!(
            merge.station_a.section_id, sec_b,
            "merge should not reference deleted B"
        );
        assert_ne!(
            merge.station_b.section_id, sec_b,
            "merge should not reference deleted B"
        );
    }

    // No closure should reference section B
    for closure in &project_after.loop_closures {
        assert_ne!(
            closure.station_a.section_id, sec_b,
            "closure should not reference deleted B"
        );
        assert_ne!(
            closure.station_b.section_id, sec_b,
            "closure should not reference deleted B"
        );
    }

    // No zero datum should reference section B
    for datum in &project_after.zero_datums {
        assert_ne!(
            datum.station_ref.section_id, sec_b,
            "datum should not reference deleted B"
        );
    }

    // A and C should still exist
    assert!(
        project_after.sections.iter().any(|s| s.id == sec_a),
        "A should remain"
    );
    assert!(
        project_after.sections.iter().any(|s| s.id == sec_c),
        "C should remain"
    );
}

/// Set zero datum in a 3-section chain, then change GPS - verify all sections move together.
#[test]
fn test_zero_datum_in_chain_positions_all_sections() {
    let state = state_with_project("ChainDatum");

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(4.0)],
        })
        .section_id
        .unwrap();

    let sec_c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![shot(6.0)],
        })
        .section_id
        .unwrap();

    // Chain: A:1 ↔ B:0, B:1 ↔ C:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 1),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_b, 1),
                station_b: StationRef::new(sec_c, 0),
            })
            .success
    );

    // Set datum at A:0
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(sec_a, 0),
                coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
            })
            .success
    );

    let project = state.get_project_state().unwrap();

    // All stations in the chain should have computable coordinates
    let a0 = project.compute_station_raw_coords(StationRef::new(sec_a, 0));
    let a1 = project.compute_station_raw_coords(StationRef::new(sec_a, 1));
    let b0 = project.compute_station_raw_coords(StationRef::new(sec_b, 0));
    let b1 = project.compute_station_raw_coords(StationRef::new(sec_b, 1));
    let c0 = project.compute_station_raw_coords(StationRef::new(sec_c, 0));
    let c1 = project.compute_station_raw_coords(StationRef::new(sec_c, 1));

    assert!(a0.is_some(), "A:0 should have coordinates");
    assert!(c1.is_some(), "C:1 should have coordinates (end of chain)");

    // Merged stations should be colocated: A:1 ≈ B:0, B:1 ≈ C:0
    if let (Some(a1c), Some(b0c)) = (a1, b0) {
        let eps = 0.5;
        assert!(
            (a1c.x - b0c.x).abs() < eps,
            "A:1 and B:0 x should be colocated: {} vs {}",
            a1c.x,
            b0c.x
        );
        assert!(
            (a1c.y - b0c.y).abs() < eps,
            "A:1 and B:0 y should be colocated: {} vs {}",
            a1c.y,
            b0c.y
        );
    }
    if let (Some(b1c), Some(c0c)) = (b1, c0) {
        let eps = 0.5;
        assert!(
            (b1c.x - c0c.x).abs() < eps,
            "B:1 and C:0 x should be colocated: {} vs {}",
            b1c.x,
            c0c.x
        );
        assert!(
            (b1c.y - c0c.y).abs() < eps,
            "B:1 and C:0 y should be colocated: {} vs {}",
            b1c.y,
            c0c.y
        );
    }

    // Now update the GPS coordinates and verify the chain still holds
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(sec_a, 0),
                coordinates: GpsCoordinates::new(46.0, -74.0, 200.0).unwrap(),
            })
            .success
    );

    let project2 = state.get_project_state().unwrap();
    let a1_new = project2.compute_station_raw_coords(StationRef::new(sec_a, 1));
    let b0_new = project2.compute_station_raw_coords(StationRef::new(sec_b, 0));
    if let (Some(a1c), Some(b0c)) = (a1_new, b0_new) {
        let eps = 0.5;
        assert!(
            (a1c.x - b0c.x).abs() < eps,
            "after GPS update, A:1 and B:0 should remain colocated"
        );
    }
}

/// Undo a merge that was part of a chain - verify the undone section disconnects
/// while the remaining merge stays intact.
#[test]
fn test_undo_merge_in_chain_disconnects_correctly() {
    let state = state_with_project("UndoChainMerge");

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(4.0)],
        })
        .section_id
        .unwrap();

    let sec_c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![shot(6.0)],
        })
        .section_id
        .unwrap();

    // Merge A:1 ↔ B:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 1),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    // Merge B:1 ↔ C:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_b, 1),
                station_b: StationRef::new(sec_c, 0),
            })
            .success
    );

    // Verify chain: 2 merges
    let project = state.get_project_state().unwrap();
    assert_eq!(project.station_merges.len(), 2);

    // Undo the B↔C merge (most recent operation)
    assert!(state.undo().success);

    let project_after = state.get_project_state().unwrap();
    assert_eq!(
        project_after.station_merges.len(),
        1,
        "only A↔B merge should remain"
    );

    // The remaining merge should be A:1 ↔ B:0
    let m = &project_after.station_merges[0];
    let merge_refs: HashSet<StationRef> = [&m.station_a, &m.station_b]
        .into_iter()
        .filter_map(|k| project_after.resolve_key(k))
        .collect();
    assert!(
        merge_refs.contains(&StationRef::new(sec_a, 1)),
        "A:1 should be in remaining merge"
    );
    assert!(
        merge_refs.contains(&StationRef::new(sec_b, 0)),
        "B:0 should be in remaining merge"
    );

    // C should still exist but disconnected
    assert!(
        project_after.sections.iter().any(|s| s.id == sec_c),
        "C should still exist"
    );

    // Redo should restore the B↔C merge
    assert!(state.redo().success);
    let project_redo = state.get_project_state().unwrap();
    assert_eq!(
        project_redo.station_merges.len(),
        2,
        "both merges should be restored after redo"
    );
}

/// Verify a merge chain survives save/reopen with coordinates intact.
#[test]
fn test_merge_chain_survives_save_reopen() {
    let temp_dir = TempDir::new().unwrap();
    let save_path = temp_dir.path().join("chain.stalx");

    let state = state_with_project("ChainPersist");

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(4.0)],
        })
        .section_id
        .unwrap();

    let sec_c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![shot(6.0)],
        })
        .section_id
        .unwrap();

    // Chain: A:1 ↔ B:0, B:1 ↔ C:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 1),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_b, 1),
                station_b: StationRef::new(sec_c, 0),
            })
            .success
    );

    // Set datum so coordinates are computable
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(sec_a, 0),
                coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
            })
            .success
    );

    // Snapshot coordinates
    let project_before = state.get_project_state().unwrap();
    let c1_before = project_before.compute_station_raw_coords(StationRef::new(sec_c, 1));

    // Save and reopen
    assert!(state.save_project(Some(save_path.clone())).success);
    let state2 = AppState::new().unwrap();
    assert!(state2.open_project(save_path).success);

    let project_after = state2.get_project_state().unwrap();

    // Verify structure
    assert_eq!(
        project_after.station_merges.len(),
        2,
        "both merges should persist"
    );
    assert_eq!(project_after.zero_datums.len(), 1, "datum should persist");
    assert_eq!(
        project_after.sections.len(),
        3,
        "all 3 sections should persist"
    );

    // Verify coordinates match
    let c1_after = project_after.compute_station_raw_coords(StationRef::new(sec_c, 1));
    if let (Some(before), Some(after)) = (c1_before, c1_after) {
        let eps = 0.01;
        assert!(
            (before.x - after.x).abs() < eps,
            "C:1 x should match: {} vs {}",
            before.x,
            after.x
        );
        assert!(
            (before.y - after.y).abs() < eps,
            "C:1 y should match: {} vs {}",
            before.y,
            after.y
        );
    }
}

/// Three sections forming a triangle: A↔B via merge, B↔C via merge, C↔A via loop closure.
/// Verifies that a loop closure can be created between non-merged stations in the same
/// connected group, and that all references remain valid.
#[test]
fn test_triangle_topology_with_loop_closure() {
    let state = state_with_project("Triangle");

    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0), shot(3.0)],
        })
        .section_id
        .unwrap();

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(4.0), shot(6.0)],
        })
        .section_id
        .unwrap();

    let sec_c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![shot(7.0), shot(2.0)],
        })
        .section_id
        .unwrap();

    // A:2 ↔ B:0 (merge)
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 2),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    // B:2 ↔ C:0 (merge)
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_b, 2),
                station_b: StationRef::new(sec_c, 0),
            })
            .success
    );

    // C:2 ↔ A:0 - third merge should be rejected (already connected transitively)
    let third_merge = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_c, 2),
        station_b: StationRef::new(sec_a, 0),
    });
    assert!(
        !third_merge.success,
        "third merge should fail - stations already connected"
    );

    // Close the triangle with a loop closure C:2 ↔ A:0.
    // These are the free endpoints of their sections - the merge graph only has
    // edges A:2↔B:0 and B:2↔C:0, so are_stations_merged(C:2, A:0) is false.
    // The sections are connected (same group), so the closure succeeds and
    // measures the survey error around the A→B→C→A triangle.
    let closure_result = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(sec_c, 2),
        station_b: StationRef::new(sec_a, 0),
        closure_strategy: ClosureStrategy::Batch,
    });
    assert!(
        closure_result.success,
        "loop closure between free endpoints should succeed"
    );

    // Verify the project is in a consistent state: 2 merges + 1 closure.
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 3, "all 3 sections should exist");
    assert_eq!(project.station_merges.len(), 2, "two merges should exist");
    assert_eq!(
        project.loop_closures.len(),
        1,
        "one loop closure should exist"
    );

    // All merges reference valid stations
    for merge in &project.station_merges {
        assert!(
            project.resolve_key(&merge.station_a).is_some(),
            "merge station_a {:?} should be valid",
            merge.station_a
        );
        assert!(
            project.resolve_key(&merge.station_b).is_some(),
            "merge station_b {:?} should be valid",
            merge.station_b
        );
    }

    // Loop closure references valid stations
    let lc = &project.loop_closures[0];
    assert!(
        project.resolve_key(&lc.station_a).is_some(),
        "closure station_a should be valid"
    );
    assert!(
        project.resolve_key(&lc.station_b).is_some(),
        "closure station_b should be valid"
    );
}
