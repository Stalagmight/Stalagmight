//! Transitive closures through AppState, move segment, batch rollback.
//!
//! Lines 11730-12906 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// === Full pipeline: transitive closures through AppState ===

#[test]
fn test_transitive_closures_in_computed_stations() {
    // Test the full pipeline: create sections, merge, create closure, then
    // verify closures_with (including transitive) appears in computed stations.
    //
    // Setup: 3 sections A, B, C
    //   Merge: A.1 ↔ B.0 (connects A and B)
    //   Merge: B.1 ↔ C.0 (connects B and C)
    //   Closure: A.0 ↔ C.1 (loop closure between endpoints)
    //
    // Expected closures_with:
    //   A.0: direct closure to C.1 (A.0 is endpoint, not merged)
    //   C.1: direct closure to A.0 (C.1 is endpoint, not merged)
    //   No transitive closures since neither endpoint is in a merge group.
    //
    // Then add ANOTHER closure: A.1 ↔ C.1
    //   A.1 is merged with B.0, so:
    //   A.1: direct closure to C.1
    //   B.0: transitive closure to C.1 (via A.1)
    //   C.1: direct closure to A.1, transitive closure to B.0 (via A.1)
    //   C.0 is merged with B.1, so:
    //   C.1: is endpoint, not in merge group for this closure
    //   Wait - C.1 is NOT merged with anything. C.0 is merged with B.1.
    //   So C.1 gets direct closure to A.1, and transitive closure to B.0 (via A.1).

    let state = state_with_project("TransitiveTest");

    // Create section A with 2 shots (stations: A.0, A.1, A.2)
    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0), shot(8.0)],
    });
    let a = result_a.section_id.unwrap();

    // Create section B with 1 shot (stations: B.0, B.1)
    let result_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let b = result_b.section_id.unwrap();

    // Create section C with 1 shot (stations: C.0, C.1)
    let result_c = state.execute(Command::CreateSection {
        name: Some("C".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![shot(3.0)],
    });
    let c = result_c.section_id.unwrap();

    // Merge A.2 ↔ B.0
    let merge1 = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(merge1.success, "Merge A.2↔B.0 should succeed");

    // Merge B.1 ↔ C.0
    let merge2 = state.execute(Command::MergeStations {
        station_a: StationRef::new(b, 1),
        station_b: StationRef::new(c, 0),
    });
    assert!(merge2.success, "Merge B.1↔C.0 should succeed");

    // Now create a loop closure between A.1 and C.1
    // (A.1 is in merge group with B.0, so B.0 should get transitive closure)
    // Wait - A.1 is NOT merged. A.2 is merged with B.0. Let me fix: use A.2 as endpoint.
    // Actually, let me just inject the closure directly since the CreateLoopClosure command
    // requires specific connectivity conditions.
    {
        let mut proj_guard = state.project.lock();
        let project = proj_guard.as_mut().unwrap();
        let closure_id = project.next_closure_id;
        project.loop_closures.push(LoopClosure {
            id: closure_id,
            station_a: StationRef::new(a, 2).into(), // A.2 (merged with B.0)
            station_b: StationRef::new(c, 1).into(), // C.1 (not merged with anything)
            path_length: 20.0,
            misclosure_distance: 0.5,
            error_percentage: 2.5,
            horizontal_misclosure: 0.4,
            vertical_misclosure: 0.3,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
        project.next_closure_id += 1;
    }

    // Get computed stations through the full pipeline
    let project = state.get_project_state().unwrap();
    let stations = project.compute_station_coordinates();

    // A.2: direct closure to C.1 (A.2 is the endpoint)
    let a2 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 2))
        .unwrap();
    let a2_direct = a2
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(c, 1) && !cr.is_transitive);
    assert!(
        a2_direct.is_some(),
        "A.2 should have direct closure to C.1, got: {:?}",
        a2.closures()
    );

    // B.0: transitive closure to C.1 via A.2 (B.0 is merged with A.2)
    let b0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 0))
        .unwrap();
    let b0_transitive = b0
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(c, 1) && cr.is_transitive);
    assert!(
        b0_transitive.is_some(),
        "B.0 should have transitive closure to C.1 (via A.2), got: {:?}",
        b0.closures()
    );
    assert_eq!(
        b0_transitive.unwrap().via_station,
        Some(StationRef::new(a, 2))
    );

    // C.1: direct closure to A.2
    let c1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 1))
        .unwrap();
    let c1_direct = c1
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(a, 2) && !cr.is_transitive);
    assert!(
        c1_direct.is_some(),
        "C.1 should have direct closure to A.2, got: {:?}",
        c1.closures()
    );

    // C.1: transitive closure to B.0 (via A.2, since B.0 is in A.2's merge group)
    let c1_transitive = c1
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(b, 0) && cr.is_transitive);
    assert!(
        c1_transitive.is_some(),
        "C.1 should have transitive closure to B.0 (via A.2), got: {:?}",
        c1.closures()
    );
    assert_eq!(
        c1_transitive.unwrap().via_station,
        Some(StationRef::new(a, 2))
    );

    // A.0 and A.1: should NOT have any closures (not endpoints, not merged with endpoints)
    let a0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 0))
        .unwrap();
    assert!(
        a0.closures().is_empty(),
        "A.0 should have no closures, got: {:?}",
        a0.closures()
    );
    let a1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 1))
        .unwrap();
    assert!(
        a1.closures().is_empty(),
        "A.1 should have no closures, got: {:?}",
        a1.closures()
    );

    // B.1: NOT in A.2's merge group, IS merged with C.0 but C.0 is not a closure endpoint
    let b1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 1))
        .unwrap();
    assert!(
        b1.closures().is_empty(),
        "B.1 should have no closures, got: {:?}",
        b1.closures()
    );

    // C.0: merged with B.1, but B.1 is not a closure endpoint. C.0 is NOT in C.1's group.
    let c0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 0))
        .unwrap();
    assert!(
        c0.closures().is_empty(),
        "C.0 should have no closures, got: {:?}",
        c0.closures()
    );
}

#[test]
fn test_transitive_closures_serialization_preserves_data() {
    // Test that closures_with survives JSON serialization (the Tauri IPC path).
    // This tests that `#[serde(default, skip_serializing_if =
    // "closures_absent_or_empty")]` skips ONLY the empty case and never costs a
    // non-empty list any data.
    use crate::models::Station;

    let state = state_with_project("SerTest");

    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0)],
    });
    let a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let b = result_b.section_id.unwrap();

    // Merge A.1 ↔ B.0
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });

    // Inject a closure between A.1 and... we need a third section
    let result_c = state.execute(Command::CreateSection {
        name: Some("C".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![shot(3.0)],
    });
    let c = result_c.section_id.unwrap();

    // Merge B.1 ↔ C.0 to connect everything
    state.execute(Command::MergeStations {
        station_a: StationRef::new(b, 1),
        station_b: StationRef::new(c, 0),
    });

    // Inject closure A.1 ↔ C.1
    {
        let mut proj_guard = state.project.lock();
        let project = proj_guard.as_mut().unwrap();
        project.loop_closures.push(LoopClosure {
            id: project.next_closure_id,
            station_a: StationRef::new(a, 1).into(),
            station_b: StationRef::new(c, 1).into(),
            path_length: 15.0,
            misclosure_distance: 0.3,
            error_percentage: 2.0,
            horizontal_misclosure: 0.2,
            vertical_misclosure: 0.2,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
        project.next_closure_id += 1;
    }

    // Get stations through the normal pipeline
    let project = state.get_project_state().unwrap();
    let stations = project.compute_station_coordinates();

    // Verify B.0 has transitive closure (it's merged with A.1)
    let b0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 0))
        .unwrap();
    assert!(
        !b0.closures().is_empty(),
        "B.0 should have transitive closures"
    );

    // Now serialize to JSON and deserialize back (simulating Tauri IPC)
    let json = serde_json::to_string(&stations).unwrap();
    let deserialized: Vec<Station> = serde_json::from_str(&json).unwrap();

    // Find B.0 in deserialized
    let b0_de = deserialized
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 0))
        .unwrap();
    assert_eq!(
        b0_de.closures().len(),
        b0.closures().len(),
        "closures_with should survive serialization round-trip"
    );
    assert!(
        b0_de.closures().iter().any(|cr| cr.is_transitive),
        "Transitive closure should survive serialization"
    );

    // Verify A.1 has direct closure
    let a1_de = deserialized
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 1))
        .unwrap();
    assert!(
        a1_de.closures().iter().any(|cr| !cr.is_transitive),
        "Direct closure on A.1 should survive serialization"
    );

    // Verify C.1 has closures (direct + transitive)
    let c1_de = deserialized
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 1))
        .unwrap();
    assert!(
        c1_de.closures().iter().any(|cr| !cr.is_transitive),
        "C.1 direct closure should survive serialization"
    );
    assert!(
        c1_de.closures().iter().any(|cr| cr.is_transitive),
        "C.1 transitive closure should survive serialization"
    );

    // #441, the other half of the wire contract: a station with NO closures omits
    // the key entirely, so the generated type has to declare it optional. As a
    // plain `Vec` it generated as a required `Array<ClosureRef>` and `tsc` could
    // not see a bare `station.closures_with.map` on the absent shape.
    let wire: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
    let (station, closure_free) = stations
        .iter()
        .zip(&wire)
        .find(|(s, _)| s.closures().is_empty())
        .expect("this fixture has stations with no closures");
    // The zip pairs by position; prove the pairing rather than assume it, or a
    // reordering serializer would let this assert about the wrong station.
    assert_eq!(
        closure_free.get("station_ref"),
        Some(&serde_json::to_value(station.station_ref).unwrap()),
        "the JSON element must be the station we tested for emptiness"
    );
    assert!(
        !closure_free
            .as_object()
            .unwrap()
            .contains_key("closures_with"),
        "a closure-free station must omit the key, not send []: {closure_free}"
    );
    assert!(
        <Station as ts_rs::TS>::decl().contains("closures_with?:"),
        "the key is absent above, so the generated declaration must mark it optional: {}",
        <Station as ts_rs::TS>::decl()
    );
}

// === Move Segment Tests ===

#[test]
fn test_move_segment() {
    let state = state_with_project("Test");

    // Create two sections and merge them so they form a segment
    let r1 = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let sid_a = r1.section_id.unwrap();

    let r2 = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(3.0)],
    });
    let sid_b = r2.section_id.unwrap();

    // Merge station A.1 with B.0
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(sid_a, 1),
        station_b: StationRef::new(sid_b, 0),
    });
    assert!(merge_result.success);

    // Record offsets after merge (merge may set non-zero offsets)
    let project = state.get_project_state().unwrap();
    let a_before = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_before = project
        .section_offsets()
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    // Move segment starting from section A
    let result = state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });
    assert!(result.success);

    // Both sections should have been moved by the offset delta
    let project = state.get_project_state().unwrap();
    let a_after = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_after = project
        .section_offsets()
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    assert!(
        (a_after.0 - a_before.0 - 10.0).abs() < 0.001,
        "Section A x should increase by 10"
    );
    assert!(
        (a_after.1 - a_before.1 - 20.0).abs() < 0.001,
        "Section A y should increase by 20"
    );
    assert!(
        (b_after.0 - b_before.0 - 10.0).abs() < 0.001,
        "Section B x (connected via merge) should increase by 10"
    );
    assert!(
        (b_after.1 - b_before.1 - 20.0).abs() < 0.001,
        "Section B y (connected via merge) should increase by 20"
    );
}

#[test]
fn test_move_segment_undo_redo() {
    let state = state_with_project("Test");

    // Create two sections and merge
    let r1 = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let sid_a = r1.section_id.unwrap();

    let r2 = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(3.0)],
    });
    let sid_b = r2.section_id.unwrap();

    state.execute(Command::MergeStations {
        station_a: StationRef::new(sid_a, 1),
        station_b: StationRef::new(sid_b, 0),
    });

    // Record offsets after merge (merge may set non-zero offsets)
    let project = state.get_project_state().unwrap();
    let a_pre_move = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_pre_move = project
        .section_offsets()
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    // Move segment
    state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });

    // Verify both moved by delta
    let project = state.get_project_state().unwrap();
    let a_post_move = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_post_move = project
        .section_offsets()
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!((a_post_move.0 - a_pre_move.0 - 10.0).abs() < 0.001);
    assert!((a_post_move.1 - a_pre_move.1 - 20.0).abs() < 0.001);
    assert!((b_post_move.0 - b_pre_move.0 - 10.0).abs() < 0.001);
    assert!((b_post_move.1 - b_pre_move.1 - 20.0).abs() < 0.001);

    // Undo should restore exact pre-move offsets
    let undo_result = state.undo();
    assert!(undo_result.success);

    let project = state.get_project_state().unwrap();
    let a_after_undo = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_after_undo = project
        .section_offsets()
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!(
        (a_after_undo.0 - a_pre_move.0).abs() < 0.001,
        "Undo should restore A's exact pre-move x offset"
    );
    assert!(
        (a_after_undo.1 - a_pre_move.1).abs() < 0.001,
        "Undo should restore A's exact pre-move y offset"
    );
    assert!(
        (b_after_undo.0 - b_pre_move.0).abs() < 0.001,
        "Undo should restore B's exact pre-move x offset"
    );
    assert!(
        (b_after_undo.1 - b_pre_move.1).abs() < 0.001,
        "Undo should restore B's exact pre-move y offset"
    );

    // Redo should reapply the delta
    let redo_result = state.redo();
    assert!(redo_result.success);

    let project = state.get_project_state().unwrap();
    let a_after_redo = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_after_redo = project
        .section_offsets()
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!(
        (a_after_redo.0 - a_after_undo.0 - 10.0).abs() < 0.001,
        "Redo should add back the x delta to section A"
    );
    assert!(
        (a_after_redo.1 - a_after_undo.1 - 20.0).abs() < 0.001,
        "Redo should add back the y delta to section A"
    );
    assert!(
        (b_after_redo.0 - b_after_undo.0 - 10.0).abs() < 0.001,
        "Redo should add back the x delta to section B"
    );
    assert!(
        (b_after_redo.1 - b_after_undo.1 - 20.0).abs() < 0.001,
        "Redo should add back the y delta to section B"
    );
}

#[test]
fn test_move_segment_nonexistent_section() {
    let state = state_with_project("Test");

    let result = state.execute(Command::MoveSegment {
        section_id: 999,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });
    assert!(
        !result.success,
        "MoveSegment should fail for nonexistent section"
    );
}

#[test]
fn test_move_segment_unaligned_closure_independent() {
    // Sections connected only via an unaligned loop closure should move independently
    let state = state_with_project("Test");

    let r1 = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let sid_a = r1.section_id.unwrap();

    let r2 = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(3.0)],
    });
    let sid_b = r2.section_id.unwrap();

    // Inject a loop closure directly (no merge needed) connecting A.1 ↔ B.0
    {
        let mut proj_guard = state.project.lock();
        let project = proj_guard.as_mut().unwrap();
        let closure_id = project.next_closure_id;
        project.loop_closures.push(LoopClosure {
            id: closure_id,
            station_a: StationRef::new(sid_a, 1).into(),
            station_b: StationRef::new(sid_b, 0).into(),
            path_length: 10.0,
            misclosure_distance: 0.1,
            error_percentage: 1.0,
            horizontal_misclosure: 0.08,
            vertical_misclosure: 0.06,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
        project.next_closure_id += 1;
    }

    // Record offsets before move
    let project = state.get_project_state().unwrap();
    let a_before = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_before = project
        .section_offsets()
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    // Move segment starting from section A
    let result = state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(5.0, 15.0, 0.0),
    });
    assert!(result.success);

    // Only the starting section should move (closure is not aligned yet)
    let project = state.get_project_state().unwrap();
    let a_after = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_after = project
        .section_offsets()
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    assert!(
        (a_after.0 - a_before.0 - 5.0).abs() < 0.001,
        "Section A x should increase by 5"
    );
    assert!(
        (a_after.1 - a_before.1 - 15.0).abs() < 0.001,
        "Section A y should increase by 15"
    );
    assert!(
        (b_after.0 - b_before.0).abs() < 0.001,
        "Section B x should remain unchanged before closure alignment"
    );
    assert!(
        (b_after.1 - b_before.1).abs() < 0.001,
        "Section B y should remain unchanged before closure alignment"
    );
}

#[test]
fn test_move_segment_aligned_closure_connectivity() {
    // Sections connected via an aligned loop closure should move together
    let state = state_with_project("Test");

    let r1 = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let sid_a = r1.section_id.unwrap();

    let r2 = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(3.0)],
    });
    let sid_b = r2.section_id.unwrap();

    // Inject an aligned loop closure directly (no merge needed) connecting A.1 ↔ B.0
    {
        let mut proj_guard = state.project.lock();
        let project = proj_guard.as_mut().unwrap();
        let closure_id = project.next_closure_id;
        project.loop_closures.push(LoopClosure {
            id: closure_id,
            station_a: StationRef::new(sid_a, 1).into(),
            station_b: StationRef::new(sid_b, 0).into(),
            path_length: 10.0,
            misclosure_distance: 0.1,
            error_percentage: 1.0,
            horizontal_misclosure: 0.08,
            vertical_misclosure: 0.06,
            strategy: ClosureStrategy::default(),
            is_aligned: true,
        });
        project.next_closure_id += 1;
    }

    // Record offsets before move
    let project = state.get_project_state().unwrap();
    let a_before = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_before = project
        .section_offsets()
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    // Move segment starting from section A
    let result = state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(5.0, 15.0, 0.0),
    });
    assert!(result.success);

    // Both sections should have moved (connected via aligned closure)
    let project = state.get_project_state().unwrap();
    let a_after = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_after = project
        .section_offsets()
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    assert!(
        (a_after.0 - a_before.0 - 5.0).abs() < 0.001,
        "Section A x should increase by 5"
    );
    assert!(
        (a_after.1 - a_before.1 - 15.0).abs() < 0.001,
        "Section A y should increase by 15"
    );
    assert!(
        (b_after.0 - b_before.0 - 5.0).abs() < 0.001,
        "Section B x (aligned closure) should increase by 5"
    );
    assert!(
        (b_after.1 - b_before.1 - 15.0).abs() < 0.001,
        "Section B y (aligned closure) should increase by 15"
    );
}

#[test]
fn test_move_segment_blocked_by_datum_over_a_closure_only_link() {
    // SYNTHETIC STATE - read this before trusting the test. The closure-only link
    // built below cannot arise through the app: `convert_orphan_closures_to_merges`
    // (models/project/offsets.rs) runs after every command and promotes a
    // sole-bridge closure to a merge, skipping only the both-endpoints-anchored
    // case, which the narrow guard would catch anyway. So this pins HARDENING, not
    // a reachable bug: it fails if the guard ever stops covering the set
    // move_segment actually offsets. The closure is pushed directly for exactly
    // that reason - the command path would convert it away.
    let state = state_with_project("ClosureDatum");

    let sid_a = state
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
    let sid_b = state
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

    // A <-> B joined ONLY by an aligned closure: no merge edge, so
    // get_merged_section_ids(B) is {B} while get_rigid_connected_section_ids(B)
    // is {A, B}. That divergence is what the widened guard covers.
    {
        let mut proj_guard = state.project.lock();
        let project = proj_guard.as_mut().unwrap();
        let closure_id = project.next_closure_id;
        project.loop_closures.push(LoopClosure {
            id: closure_id,
            station_a: StationRef::new(sid_a, 1).into(),
            station_b: StationRef::new(sid_b, 0).into(),
            path_length: 10.0,
            misclosure_distance: 0.1,
            error_percentage: 1.0,
            horizontal_misclosure: 0.08,
            vertical_misclosure: 0.06,
            strategy: ClosureStrategy::default(),
            is_aligned: true,
        });
        project.next_closure_id += 1;
    }

    let datum = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sid_a, 0),
        coordinates: GpsCoordinates::new(24.55, -81.78, 0.0).unwrap(),
    });
    assert!(datum.success, "datum setup must succeed");

    let before = state.get_project_state().unwrap();
    let a_before = before
        .section_offsets()
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));

    // Dragging B would drag A - the datum holder - along with it.
    let result = state.execute(Command::MoveSegment {
        section_id: sid_b,
        offset: Coordinates::new(5.0, 15.0, 0.0),
    });
    assert!(
        !result.success,
        "moving a segment whose closure-connected group holds a datum must be refused"
    );

    let after = state.get_project_state().unwrap();
    let a_after = after
        .section_offsets()
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!(
        (a_after.0 - a_before.0).abs() < 1e-9 && (a_after.1 - a_before.1).abs() < 1e-9,
        "the datum-bearing section must not have moved"
    );
}

#[test]
fn test_move_segment_isolated() {
    // An unconnected section should NOT be moved when moving a segment
    let state = state_with_project("Test");

    // Create two sections WITHOUT merging them
    let r1 = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let sid_a = r1.section_id.unwrap();

    let r2 = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(3.0)],
    });
    let sid_b = r2.section_id.unwrap();

    // Record offsets before move
    let project = state.get_project_state().unwrap();
    let a_before = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let b_before = project
        .section_offsets()
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    // Move segment starting from section A (B is NOT connected)
    let result = state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });
    assert!(result.success);

    // Section A should have moved (entry must exist after MoveSegment)
    let project = state.get_project_state().unwrap();
    let a_after = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .expect("Section A offset should exist after MoveSegment");
    // Section B was not moved; entry may or may not exist
    let b_after = project
        .section_offsets()
        .get(&sid_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    assert!(
        (a_after.0 - a_before.0 - 10.0).abs() < 0.001,
        "Section A x should increase by 10"
    );
    assert!(
        (a_after.1 - a_before.1 - 20.0).abs() < 0.001,
        "Section A y should increase by 20"
    );

    // Section B should NOT have moved (not connected)
    assert!(
        (b_after.0 - b_before.0).abs() < 0.001,
        "Isolated Section B x should NOT change"
    );
    assert!(
        (b_after.1 - b_before.1).abs() < 0.001,
        "Isolated Section B y should NOT change"
    );
}

#[test]
fn test_move_segment_accumulates() {
    // Multiple MoveSegment calls should accumulate offsets
    let state = state_with_project("Test");

    let r1 = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let sid_a = r1.section_id.unwrap();

    // Record initial offset
    let project = state.get_project_state().unwrap();
    let initial = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .unwrap_or((0.0, 0.0));

    // First move
    let result1 = state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(10.0, 5.0, 0.0),
    });
    assert!(result1.success);

    let project = state.get_project_state().unwrap();
    let after_first = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .expect("Offset should exist after first MoveSegment");
    assert!(
        (after_first.0 - initial.0 - 10.0).abs() < 0.001,
        "After first move, x should increase by 10"
    );
    assert!(
        (after_first.1 - initial.1 - 5.0).abs() < 0.001,
        "After first move, y should increase by 5"
    );

    // Second move
    let result2 = state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(3.0, 7.0, 0.0),
    });
    assert!(result2.success);

    let project = state.get_project_state().unwrap();
    let after_second = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .expect("Offset should exist after second MoveSegment");
    assert!(
        (after_second.0 - initial.0 - 13.0).abs() < 0.001,
        "After second move, x should be initial + 10 + 3 = +13"
    );
    assert!(
        (after_second.1 - initial.1 - 12.0).abs() < 0.001,
        "After second move, y should be initial + 5 + 7 = +12"
    );

    // Third move with negative offset
    let result3 = state.execute(Command::MoveSegment {
        section_id: sid_a,
        offset: Coordinates::new(-5.0, -2.0, 0.0),
    });
    assert!(result3.success);

    let project = state.get_project_state().unwrap();
    let after_third = project
        .section_offsets()
        .get(&sid_a)
        .copied()
        .expect("Offset should exist after third MoveSegment");
    assert!(
        (after_third.0 - initial.0 - 8.0).abs() < 0.001,
        "After third move, x should be initial + 13 - 5 = +8"
    );
    assert!(
        (after_third.1 - initial.1 - 10.0).abs() < 0.001,
        "After third move, y should be initial + 12 - 2 = +10"
    );
}

// === Batch Tests ===

#[test]
fn test_batch_rollback_on_failure() {
    let state = state_with_project("Test");

    // Create a section first
    let r1 = state.execute(Command::CreateSection {
        name: Some("Existing".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let _existing_id = r1.section_id.unwrap();

    // Capture state before batch
    let project_before = state.get_project_state().unwrap();
    let sections_before = project_before.sections.len();

    // Execute batch with valid command followed by invalid command
    let result = state.execute(Command::Batch {
        commands: vec![
            Command::CreateSection {
                name: Some("New".to_string()),
                surveyor: None,
                date: None,
                color: "#00ff00".to_string(),
                direction: None,
                shots: vec![],
            },
            // Move a nonexistent section - should fail
            Command::MoveSection {
                section_id: 99999,
                offset: Coordinates::new(1.0, 1.0, 0.0),
            },
        ],
    });
    assert!(!result.success, "Batch should fail");

    // Verify rollback: project should be exactly as before
    let project_after = state.get_project_state().unwrap();
    assert_eq!(
        project_after.sections.len(),
        sections_before,
        "Batch rollback should restore exact section count"
    );
}

#[test]
fn test_batch_disallowed_command() {
    let state = state_with_project("Test");

    // Try to open a project inside a batch (should be rejected by whitelist)
    let result = state.execute(Command::Batch {
        commands: vec![Command::OpenProject {
            path: PathBuf::from("/nonexistent"),
        }],
    });
    assert!(
        !result.success,
        "Batch should reject non-whitelisted commands"
    );
    assert!(
        result
            .error
            .unwrap_or_default()
            .contains("not allowed in batch"),
        "Error should mention batch whitelist"
    );
}
