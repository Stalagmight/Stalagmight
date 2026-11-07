//! Merge chains and the comment system.
//!
//! Lines 20663-21911 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project, straight_shot};

// === Merge chain tests ===

/// Test 2a: Transitive offset correctness in a 3-section chain.
///
/// When neither section has a datum, the merge logic makes the *second* argument
/// the anchor (B is the anchor, A moves). Station positions are:
///
///   A: 1 shot east (azimuth 90°), length 10m → A.0 at its offset, A.1 = A.0 + (10, 0)
///   B: 1 shot north (azimuth 0°), length 5m  → B.0=(0,0), B.1=(0,5)
///   C: 1 shot east (azimuth 90°), length 3m  → initially C.0=(0,0), C.1=(3,0)
///
///   Merge A.1 ↔ B.0: B is anchor, A moves so A.1 = B.0 = (0,0) → A.0=(-10,0)
///   Merge B.1 ↔ C.0: B is part of existing group (B is anchor), C moves so C.0 = B.1 = (0,5)
///                     → C.1 = (3, 5)
///
/// The key invariant is that both merge points are colocated and the relative
/// geometry within each section is preserved exactly.
#[test]
fn test_merge_chain_transitive_offsets_are_correct() {
    let state = state_with_project("Chain");

    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![straight_shot(90.0, 10.0)],
        })
        .section_id
        .unwrap();

    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![straight_shot(0.0, 5.0)],
        })
        .section_id
        .unwrap();

    let c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![straight_shot(90.0, 3.0)],
        })
        .section_id
        .unwrap();

    // Merge A.1 ↔ B.0
    let r1 = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    assert!(r1.success, "First merge should succeed");

    // Merge B.1 ↔ C.0
    let r2 = state.execute(Command::MergeStations {
        station_a: StationRef::new(b, 1),
        station_b: StationRef::new(c, 0),
    });
    assert!(r2.success, "Second merge should succeed");

    let proj = state.get_project_state().unwrap();
    let stations = proj.compute_station_coordinates();
    let coord = |sec: SectionId, idx: u32| -> Coordinates {
        stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(sec, idx))
            .unwrap_or_else(|| panic!("station {}:{} not found", sec, idx))
            .coordinates
    };

    let eps = 1e-4;

    // Invariant 1: merge points must be exactly colocated
    assert!(
        (coord(a, 1).x - coord(b, 0).x).abs() < eps,
        "A.1 and B.0 must be colocated (x): A.1={} B.0={}",
        coord(a, 1).x,
        coord(b, 0).x
    );
    assert!(
        (coord(a, 1).y - coord(b, 0).y).abs() < eps,
        "A.1 and B.0 must be colocated (y)"
    );
    assert!(
        (coord(b, 1).x - coord(c, 0).x).abs() < eps,
        "B.1 and C.0 must be colocated (x): B.1={} C.0={}",
        coord(b, 1).x,
        coord(c, 0).x
    );
    assert!(
        (coord(b, 1).y - coord(c, 0).y).abs() < eps,
        "B.1 and C.0 must be colocated (y)"
    );

    // Invariant 2: relative geometry within each section is preserved.
    // A: one shot east 10m, so A.1 = A.0 + (10, 0)
    assert!(
        (coord(a, 1).x - coord(a, 0).x - 10.0).abs() < eps,
        "A internal geometry: A.1.x - A.0.x should be 10, got {}",
        coord(a, 1).x - coord(a, 0).x
    );
    assert!(
        (coord(a, 1).y - coord(a, 0).y).abs() < eps,
        "A internal geometry: A.1.y - A.0.y should be 0"
    );

    // B: one shot north 5m, so B.1 = B.0 + (0, 5)
    assert!(
        (coord(b, 1).x - coord(b, 0).x).abs() < eps,
        "B internal geometry: B.1.x - B.0.x should be 0"
    );
    assert!(
        (coord(b, 1).y - coord(b, 0).y - 5.0).abs() < eps,
        "B internal geometry: B.1.y - B.0.y should be 5, got {}",
        coord(b, 1).y - coord(b, 0).y
    );

    // C: one shot east 3m, so C.1 = C.0 + (3, 0)
    assert!(
        (coord(c, 1).x - coord(c, 0).x - 3.0).abs() < eps,
        "C internal geometry: C.1.x - C.0.x should be 3"
    );
    assert!(
        (coord(c, 1).y - coord(c, 0).y).abs() < eps,
        "C internal geometry: C.1.y - C.0.y should be 0"
    );

    // Invariant 3: transitive positioning - C.0 must equal B.1, which equals B.0 + (0,5).
    // And B.0 equals A.1, which equals A.0 + (10, 0).
    // So the whole chain is geometrically consistent.
    let a1 = coord(a, 1);
    let b0 = coord(b, 0);
    let b1 = coord(b, 1);
    let c0 = coord(c, 0);
    assert!(
        (b1.x - b0.x).abs() < eps && (b1.y - b0.y - 5.0).abs() < eps,
        "B.1 should be 5m north of B.0"
    );
    // C.0 == B.1, and B.1 == A.1 + (0, 5) (since B is 5m north of its start and B.0 == A.1)
    assert!(
        (c0.x - a1.x).abs() < eps,
        "C.0.x should equal A.1.x (transitive via B): C.0.x={} A.1.x={}",
        c0.x,
        a1.x
    );
    assert!(
        (c0.y - a1.y - 5.0).abs() < eps,
        "C.0.y should be A.1.y + 5 (transitive via B): C.0.y={} A.1.y={}",
        c0.y,
        a1.y
    );
}

/// Test 2b: Inserting a shot at position 0 of the middle section B in a chain
/// A↔B↔C correctly shifts B's station merge indices.
///
/// After the insert the merge edge that pointed to B.0 (from A.1) should now
/// point to B.1, because all existing stations shifted up by one.
#[test]
fn test_insert_before_pos0_shifts_merge_refs_in_chain() {
    let state = state_with_project("ShiftChain");

    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![straight_shot(90.0, 10.0)],
        })
        .section_id
        .unwrap();

    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![straight_shot(0.0, 5.0)],
        })
        .section_id
        .unwrap();

    let c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![straight_shot(90.0, 3.0)],
        })
        .section_id
        .unwrap();

    // Merge A.1 ↔ B.0  and  B.1 ↔ C.0
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    state.execute(Command::MergeStations {
        station_a: StationRef::new(b, 1),
        station_b: StationRef::new(c, 0),
    });

    // Get first shot id of section B so we can insert before it
    let first_b_shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.sections.iter().find(|s| s.id == b).unwrap().shots[0].id
    };

    // Insert a new shot at the very start of section B (before_shot_id = first shot)
    let insert_result = state.execute(Command::CreateShot {
        section_id: b,
        shot: straight_shot(270.0, 2.0),
        after_shot_id: None,
        before_shot_id: Some(first_b_shot_id),
    });
    assert!(
        insert_result.success,
        "Insert before pos 0 in B should succeed"
    );

    let proj = state.get_project_state().unwrap();

    // After inserting at pos 0 in B, all original B station indices shift by +1.
    // The merge A.1 ↔ B.0 should now be A.1 ↔ B.1
    // The merge B.1 ↔ C.0 should now be B.2 ↔ C.0
    let merges = &proj.station_merges;
    assert_eq!(merges.len(), 2, "Should still have two merge edges");

    let has_a1_b1 = merges.iter().any(|m| {
        (proj.key_is_ref(&m.station_a, StationRef::new(a, 1))
            && proj.key_is_ref(&m.station_b, StationRef::new(b, 1)))
            || (proj.key_is_ref(&m.station_a, StationRef::new(b, 1))
                && proj.key_is_ref(&m.station_b, StationRef::new(a, 1)))
    });
    assert!(
        has_a1_b1,
        "After insert-before-0 in B, merge should be A.1↔B.1, got: {:?}",
        merges
    );

    let has_b2_c0 = merges.iter().any(|m| {
        (proj.key_is_ref(&m.station_a, StationRef::new(b, 2))
            && proj.key_is_ref(&m.station_b, StationRef::new(c, 0)))
            || (proj.key_is_ref(&m.station_a, StationRef::new(c, 0))
                && proj.key_is_ref(&m.station_b, StationRef::new(b, 2)))
    });
    assert!(
        has_b2_c0,
        "After insert-before-0 in B, merge should be B.2↔C.0, got: {:?}",
        merges
    );

    // The merge points must still be colocated after the shift
    let stations = proj.compute_station_coordinates();
    let coord = |sec: SectionId, idx: u32| -> Coordinates {
        stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(sec, idx))
            .unwrap_or_else(|| panic!("station {}:{} not found", sec, idx))
            .coordinates
    };
    let eps = 1e-4;
    assert!(
        (coord(a, 1).x - coord(b, 1).x).abs() < eps,
        "A.1 and B.1 must be colocated after insert"
    );
    assert!(
        (coord(a, 1).y - coord(b, 1).y).abs() < eps,
        "A.1 and B.1 must be colocated after insert"
    );
    assert!(
        (coord(b, 2).x - coord(c, 0).x).abs() < eps,
        "B.2 and C.0 must be colocated after insert"
    );
    assert!(
        (coord(b, 2).y - coord(c, 0).y).abs() < eps,
        "B.2 and C.0 must be colocated after insert"
    );
}

/// Test 2c: Deleting the middle section B from a chain A↔B↔C removes both
/// merge edges (A↔B and B↔C), leaving A and C isolated.
#[test]
fn test_delete_middle_section_removes_both_merge_edges() {
    let state = state_with_project("DeleteMiddle");

    let a = state
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

    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(3.0)],
        })
        .section_id
        .unwrap();

    let c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![shot(4.0)],
        })
        .section_id
        .unwrap();

    // Build chain A↔B↔C
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    state.execute(Command::MergeStations {
        station_a: StationRef::new(b, 1),
        station_b: StationRef::new(c, 0),
    });
    assert_eq!(state.get_project_state().unwrap().station_merges.len(), 2);

    // Delete the middle section B
    let delete_result = state.execute(Command::DeleteSection { section_id: b });
    assert!(
        delete_result.success,
        "Deleting middle section should succeed"
    );

    let proj = state.get_project_state().unwrap();

    // Both merge edges should be gone
    assert_eq!(
        proj.station_merges.len(),
        0,
        "Both merge edges should be removed when middle section is deleted"
    );

    // A and C should still exist
    assert_eq!(proj.sections.len(), 2);
    let section_ids: Vec<SectionId> = proj.sections.iter().map(|s| s.id).collect();
    assert!(section_ids.contains(&a), "Section A should still exist");
    assert!(section_ids.contains(&c), "Section C should still exist");

    // A and C should be isolated (not connected to each other)
    let connected_a = proj.get_connected_section_ids(a);
    assert_eq!(
        connected_a.len(),
        1,
        "A should only be connected to itself after B deleted"
    );
    assert!(
        !connected_a.contains(&c),
        "A and C should not be connected after B deleted"
    );
}

/// Test 2d: Unmerging one edge in a chain A↔B↔C does not leave B with a
/// stale offset. After unmerging A↔B, B is isolated and should report itself
/// as disconnected from both A and C.  The remaining B↔C edge must still keep
/// B and C colocated at their merge point.
#[test]
fn test_unmerge_in_chain_does_not_leave_stale_offset() {
    let state = state_with_project("UnmergeChain");

    // A: 1 shot east (10m)
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![straight_shot(90.0, 10.0)],
        })
        .section_id
        .unwrap();

    // B: 1 shot north (5m)
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![straight_shot(0.0, 5.0)],
        })
        .section_id
        .unwrap();

    // C: 1 shot east (3m)
    let c = state
        .execute(Command::CreateSection {
            name: Some("C".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![straight_shot(90.0, 3.0)],
        })
        .section_id
        .unwrap();

    // Build chain: A.1↔B.0, B.1↔C.0
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    state.execute(Command::MergeStations {
        station_a: StationRef::new(b, 1),
        station_b: StationRef::new(c, 0),
    });

    // Capture where B.1 and C.0 are (they should be colocated)
    let (b1_before, c0_before) = {
        let proj = state.get_project_state().unwrap();
        let stations = proj.compute_station_coordinates();
        let b1 = stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(b, 1))
            .unwrap()
            .coordinates;
        let c0 = stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(c, 0))
            .unwrap()
            .coordinates;
        (b1, c0)
    };
    let eps = 1e-4;
    assert!(
        (b1_before.x - c0_before.x).abs() < eps,
        "B.1 and C.0 should be colocated before unmerge"
    );

    // Unmerge A↔B (the A.1 station is in the A↔B merge edge)
    let unmerge_result = state.execute(Command::UnmergeStations {
        station_ref: StationRef::new(a, 1),
    });
    assert!(unmerge_result.success, "Unmerge should succeed");

    let proj = state.get_project_state().unwrap();

    // Only one merge edge should remain: B.1↔C.0
    assert_eq!(proj.station_merges.len(), 1, "Only B↔C edge should remain");

    // A should now be isolated (not connected to B or C)
    let connected_a = proj.get_connected_section_ids(a);
    assert_eq!(connected_a.len(), 1, "A should be isolated after unmerge");
    assert!(
        !connected_a.contains(&b),
        "A should not be connected to B after unmerge"
    );

    // B and C must still be colocated at their merge point
    let stations = proj.compute_station_coordinates();
    let b1_after = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 1))
        .unwrap()
        .coordinates;
    let c0_after = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 0))
        .unwrap()
        .coordinates;

    assert!(
        (b1_after.x - c0_after.x).abs() < eps,
        "B.1 and C.0 must remain colocated after unmerging A↔B: B.1=({},{}) C.0=({},{})",
        b1_after.x,
        b1_after.y,
        c0_after.x,
        c0_after.y
    );
    assert!(
        (b1_after.y - c0_after.y).abs() < eps,
        "B.1 and C.0 must remain colocated after unmerging A↔B"
    );
}

/// Test 2e: Datum-aware merge direction - when merging a datum-holding section
/// into a section without a datum, the datum section must not move; only the
/// non-datum section gets an offset.
///
/// Setup:
///   A: 1 shot east (10m), zero datum at A.0 pinning it to absolute world coords.
///   B: 1 shot north (5m), no datum.
///
///   Merge A.1 ↔ B.0  → B must move so B.0 = A.1; A must NOT move.
///
/// Assert A.0 remains at its original coordinates after the merge.
#[test]
fn test_merge_datum_section_does_not_move() {
    let state = state_with_project("DatumAware");

    // A: shot heading east
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![straight_shot(90.0, 10.0)],
        })
        .section_id
        .unwrap();

    // B: shot heading north
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![straight_shot(0.0, 5.0)],
        })
        .section_id
        .unwrap();

    // Pin A.0 to GPS - sets a zero datum on the A group
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });

    // Capture A's coordinates before merge
    let (a0_before, a1_before) = {
        let proj = state.get_project_state().unwrap();
        let stations = proj.compute_station_coordinates();
        let a0 = stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(a, 0))
            .unwrap()
            .coordinates;
        let a1 = stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(a, 1))
            .unwrap()
            .coordinates;
        (a0, a1)
    };

    // Merge B into A: A.1 ↔ B.0
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    assert!(merge_result.success, "Merge should succeed");

    let proj = state.get_project_state().unwrap();
    let stations = proj.compute_station_coordinates();
    let eps = 1e-4;

    let a0_after = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 0))
        .unwrap()
        .coordinates;
    let a1_after = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 1))
        .unwrap()
        .coordinates;
    let b0_after = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 0))
        .unwrap()
        .coordinates;

    // A must not have moved - coordinates must be identical before and after merge
    assert!(
        (a0_after.x - a0_before.x).abs() < eps,
        "A.0.x must not move: before={} after={}",
        a0_before.x,
        a0_after.x
    );
    assert!(
        (a0_after.y - a0_before.y).abs() < eps,
        "A.0.y must not move"
    );
    assert!(
        (a1_after.x - a1_before.x).abs() < eps,
        "A.1.x must not move: before={} after={}",
        a1_before.x,
        a1_after.x
    );
    assert!(
        (a1_after.y - a1_before.y).abs() < eps,
        "A.1.y must not move"
    );

    // B.0 must now be colocated with A.1
    assert!(
        (b0_after.x - a1_after.x).abs() < eps,
        "B.0 must align with A.1 (x): B.0={} A.1={}",
        b0_after.x,
        a1_after.x
    );
    assert!(
        (b0_after.y - a1_after.y).abs() < eps,
        "B.0 must align with A.1 (y)"
    );

    // The datum section A must not have received a non-zero section offset
    // (SetZeroDatum may write a (0,0) entry, but must not shift A during merge)
    if let Some(&(ox, oy)) = proj.section_offsets().get(&a) {
        assert!(
            ox.abs() < eps && oy.abs() < eps,
            "Datum section A must not receive a non-zero offset from merge: offset=({ox},{oy})"
        );
    }
    // B must have received an offset (it moved to align with A)
    assert!(
        proj.section_offsets().contains_key(&b),
        "Non-datum section B must have received a section offset from merge"
    );
}

/// Test 2f: A shot marked as reference (frozen) survives a merge followed by
/// an undo. After undo the shot must still be virtual.
#[test]
fn test_reference_shot_survives_merge_then_undo() {
    let state = state_with_project("RefShot");

    let a = state
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

    let b = state
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

    // Mark the first shot of A as a reference shot
    let first_shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.sections.iter().find(|s| s.id == a).unwrap().shots[0].id
    };
    let ref_result = state.execute(Command::SetShotKind {
        section_id: a,
        shot_id: first_shot_id,
        kind: ShotKind::Jump,
    });
    assert!(ref_result.success, "SetShotKind should succeed");

    // Confirm it's marked before merge
    {
        let proj = state.get_project_state().unwrap();
        let shot = proj
            .sections
            .iter()
            .find(|s| s.id == a)
            .unwrap()
            .shots
            .iter()
            .find(|sh| sh.id == first_shot_id)
            .unwrap();
        assert!(
            shot.kind.is_virtual(),
            "Shot should be virtual before merge"
        );
    }

    // Merge A.2 ↔ B.0
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(merge_result.success, "Merge should succeed");

    // Reference status must survive the merge
    {
        let proj = state.get_project_state().unwrap();
        let shot = proj
            .sections
            .iter()
            .find(|s| s.id == a)
            .unwrap()
            .shots
            .iter()
            .find(|sh| sh.id == first_shot_id)
            .unwrap();
        assert!(
            shot.kind.is_virtual(),
            "Shot must still be reference after merge"
        );
    }

    // Undo the merge
    let undo_result = state.undo();
    assert!(undo_result.success, "Undo should succeed");

    // Reference status must survive the undo
    {
        let proj = state.get_project_state().unwrap();
        let section_a = proj.sections.iter().find(|s| s.id == a).unwrap();
        let shot = section_a
            .shots
            .iter()
            .find(|sh| sh.id == first_shot_id)
            .unwrap();
        assert!(
            shot.kind.is_virtual(),
            "Shot must still be reference after undo of merge"
        );
        // Merge should be gone
        assert!(proj.station_merges.is_empty(), "Merge should be undone");
    }
}

// === Comment system tests ===

#[test]
fn test_set_station_comment_set_update_remove() {
    let state = state_with_project("Comments");
    let sec = state
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
    let station = StationRef::new(sec, 1);
    // Set
    assert!(
        state
            .execute(Command::SetStationComment {
                station_ref: station,
                comment: Some("marker A".to_string()),
            })
            .success
    );
    assert_eq!(
        state
            .get_project_state()
            .unwrap()
            .station_comment_for_ref(station)
            .map(String::as_str),
        Some("marker A")
    );
    // Update
    state.execute(Command::SetStationComment {
        station_ref: station,
        comment: Some("marker B".to_string()),
    });
    assert_eq!(
        state
            .get_project_state()
            .unwrap()
            .station_comment_for_ref(station)
            .map(String::as_str),
        Some("marker B")
    );
    // Remove via None
    state.execute(Command::SetStationComment {
        station_ref: station,
        comment: None,
    });
    assert!(
        !state
            .get_project_state()
            .unwrap()
            .station_comment_for_ref(station)
            .is_some(),
        "None removes comment"
    );
    // Remove via empty string
    state.execute(Command::SetStationComment {
        station_ref: station,
        comment: Some("temp".to_string()),
    });
    state.execute(Command::SetStationComment {
        station_ref: station,
        comment: Some(String::new()),
    });
    assert!(
        !state
            .get_project_state()
            .unwrap()
            .station_comment_for_ref(station)
            .is_some(),
        "empty string removes comment"
    );
}

#[test]
fn test_set_station_comment_invalid_refs_fail() {
    let state = state_with_project("BadComments");
    let sec = state
        .execute(Command::CreateSection {
            name: None,
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(3.0)],
        })
        .section_id
        .unwrap();
    // 1 shot -> stations 0..1, index 3 is OOB
    assert!(
        !state
            .execute(Command::SetStationComment {
                station_ref: StationRef::new(sec, 3),
                comment: Some("oob".to_string()),
            })
            .success,
        "OOB index should fail"
    );
    assert!(
        !state
            .execute(Command::SetStationComment {
                station_ref: StationRef::new(9999, 0),
                comment: Some("ghost".to_string()),
            })
            .success,
        "nonexistent section should fail"
    );
}

#[test]
fn test_station_comment_survives_unrelated_undo_redo() {
    let state = state_with_project("CommentUndo");
    let sec = state
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
    let station = StationRef::new(sec, 1);
    state.execute(Command::SetStationComment {
        station_ref: station,
        comment: Some("keep me".to_string()),
    });
    // Unrelated op
    state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(2.0)],
    });
    assert!(state.undo().success);
    assert_eq!(
        state
            .get_project_state()
            .unwrap()
            .station_comment_for_ref(station)
            .map(String::as_str),
        Some("keep me"),
        "survives undo"
    );
    assert!(state.redo().success);
    assert_eq!(
        state
            .get_project_state()
            .unwrap()
            .station_comment_for_ref(station)
            .map(String::as_str),
        Some("keep me"),
        "survives redo"
    );
}

#[test]
fn test_station_comment_persists_across_save_open() {
    let temp_dir = TempDir::new().unwrap();
    let save_path = temp_dir.path().join("comments.stalx");
    let state = state_with_project("CommentPersist");
    let sec = state
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
    let station = StationRef::new(sec, 1);
    state.execute(Command::SetStationComment {
        station_ref: station,
        comment: Some("persisted note".to_string()),
    });
    assert!(state.save_project(Some(save_path.clone())).success);
    let state2 = AppState::new().unwrap();
    assert!(state2.open_project(save_path).success);
    assert_eq!(
        state2
            .get_project_state()
            .unwrap()
            .station_comment_for_ref(station)
            .map(String::as_str),
        Some("persisted note"),
        "comment survives save/open"
    );
}

#[test]
fn test_move_comment_transfers_between_stations() {
    use crate::core::command::CommentHolder;
    let state = state_with_project("MoveComment");
    let sec = state
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
    let src = StationRef::new(sec, 0);
    let tgt = StationRef::new(sec, 2);
    state.execute(Command::SetStationComment {
        station_ref: src,
        comment: Some("source text".to_string()),
    });
    let r = state.execute(Command::MoveComment {
        source: CommentHolder::Station { station_ref: src },
        target: CommentHolder::Station { station_ref: tgt },
        swap: false,
    });
    assert!(r.success, "MoveComment should succeed");
    let proj = state.get_project_state().unwrap();
    assert!(
        !proj.station_comment_for_ref(src).is_some(),
        "source has no comment after move"
    );
    assert_eq!(
        proj.station_comment_for_ref(tgt).map(String::as_str),
        Some("source text"),
        "target has comment"
    );
}

#[test]
fn test_move_comment_swap_exchanges_comments() {
    use crate::core::command::CommentHolder;
    let state = state_with_project("SwapComment");
    let sec = state
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
    let s1 = StationRef::new(sec, 0);
    let s2 = StationRef::new(sec, 2);
    state.execute(Command::SetStationComment {
        station_ref: s1,
        comment: Some("alpha".to_string()),
    });
    state.execute(Command::SetStationComment {
        station_ref: s2,
        comment: Some("beta".to_string()),
    });
    assert!(
        state
            .execute(Command::MoveComment {
                source: CommentHolder::Station { station_ref: s1 },
                target: CommentHolder::Station { station_ref: s2 },
                swap: true,
            })
            .success
    );
    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.station_comment_for_ref(s1).map(String::as_str),
        Some("beta")
    );
    assert_eq!(
        proj.station_comment_for_ref(s2).map(String::as_str),
        Some("alpha")
    );
}

/// What a same-holder `MoveComment` could disturb, read at one instant.
#[derive(Debug, PartialEq)]
struct MoveCommentObservables {
    station_comment: Option<String>,
    shot_comment: Option<String>,
    modified_at: chrono::DateTime<chrono::Utc>,
    undo_len: usize,
    redo_len: usize,
    dirty: bool,
}

fn move_comment_observables(
    state: &AppState,
    station: StationRef,
    sid: SectionId,
    shid: ShotId,
) -> MoveCommentObservables {
    let project = state.get_project_state().unwrap();
    let (undo_len, redo_len) = {
        let stack = state.undo_stack.lock();
        (stack.len(), stack.redo_len())
    };
    MoveCommentObservables {
        station_comment: project.station_comment_for_ref(station).cloned(),
        shot_comment: project
            .get_section(sid)
            .unwrap()
            .get_shot(shid)
            .unwrap()
            .comment
            .clone(),
        modified_at: project.modified_at,
        undo_len,
        redo_len,
        dirty: dirty_bit(state),
    }
}

/// A16 (#576): moving a comment onto its OWN holder must be a true no-op.
///
/// The map's Move Comment tool opens the swap dialog whenever the clicked target
/// already has a comment, without comparing it to the picked-up source, so
/// clicking the same station (or shot) twice and choosing Overwrite sends
/// `MoveComment { source: X, target: X, swap: false }`. That wrote X's text onto
/// X, then cleared the "source" - X - and the comment was gone. Swap on the same
/// holder changed nothing, yet still pushed an undo entry (dropping the redo
/// branch) and stamped `has_unpushed_edits` on a remote copy.
///
/// Each case runs on a stamped-remote, editable fixture carrying a station
/// comment and a shot comment, with a redo branch to lose and a clean dirty bit,
/// so the no-op is pinned on every observable, not just the comment text.
#[test]
fn move_comment_onto_its_own_holder_is_a_true_noop() {
    use crate::core::command::CommentHolder;

    let mut failures = Vec::new();
    for holder_kind in ["station", "shot"] {
        for swap in [false, true] {
            let (state, sid, shid, _temp) = remote_fixture_editable();
            let station = StationRef::new(sid, 1);
            {
                let mut guard = state.project.lock();
                let project = guard.as_mut().unwrap();
                let key = project.key_for_ref(station).unwrap();
                project.station_comments.insert(key, "sump".to_string());
                project
                    .get_section_mut(sid)
                    .unwrap()
                    .get_shot_mut(shid)
                    .unwrap()
                    .comment = Some("low airspace".to_string());
            }
            // Give the redo stack something to lose: a real edit, then undo it.
            assert!(
                state
                    .execute(Command::SetStationComment {
                        station_ref: StationRef::new(sid, 0),
                        comment: Some("scratch".to_string()),
                    })
                    .success
            );
            assert!(state.undo().success);
            // ...and start from a clean remote copy, so a stamp is visible.
            state
                .database
                .lock()
                .as_ref()
                .unwrap()
                .save_speleo_link(Some(&fixture_speleo_link()))
                .unwrap();

            let before = move_comment_observables(&state, station, sid, shid);
            assert_eq!(before.redo_len, 1, "fixture: a redo branch to protect");
            assert!(!before.dirty, "fixture: the remote copy starts clean");

            let holder = if holder_kind == "station" {
                CommentHolder::Station {
                    station_ref: station,
                }
            } else {
                CommentHolder::Shot {
                    section_id: sid,
                    shot_id: shid,
                }
            };
            let result = state.execute(Command::MoveComment {
                source: holder.clone(),
                target: holder,
                swap,
            });
            let after = move_comment_observables(&state, station, sid, shid);

            println!(
                "[A16] {holder_kind} holder onto itself, swap={swap}: success={}\n  \
                 station comment {:?} -> {:?}\n  shot comment    {:?} -> {:?}\n  \
                 undo_len {} -> {}, redo_len {} -> {}, remote dirty {} -> {}, \
                 modified_at unchanged: {}",
                result.success,
                before.station_comment,
                after.station_comment,
                before.shot_comment,
                after.shot_comment,
                before.undo_len,
                after.undo_len,
                before.redo_len,
                after.redo_len,
                before.dirty,
                after.dirty,
                before.modified_at == after.modified_at,
            );
            if !result.success || after != before {
                failures.push(format!(
                    "{holder_kind} swap={swap}: success={} error={:?}\n  before {before:?}\n  after  {after:?}",
                    result.success, result.error
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "a same-holder MoveComment must succeed and change nothing:\n{}",
        failures.join("\n")
    );

    // A holder WITHOUT a comment is not a no-op: it still fails exactly as before.
    let (state, sid, _shid, _temp) = remote_fixture_editable();
    let bare = CommentHolder::Station {
        station_ref: StationRef::new(sid, 0),
    };
    let result = state.execute(Command::MoveComment {
        source: bare.clone(),
        target: bare,
        swap: false,
    });
    assert!(!result.success, "nothing to move must still fail");
    assert_eq!(
        result.error.as_deref(),
        Some("Source has no comment to move")
    );
}
