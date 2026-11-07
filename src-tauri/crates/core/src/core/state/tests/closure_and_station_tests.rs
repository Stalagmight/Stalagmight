//! CreateLoopClosure undo, move station, CreateSection undo id preservation.
//!
//! Lines 9932-10849 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// === CreateLoopClosure undo/redo tests ===

#[test]
fn test_undo_create_loop_closure() {
    let state = state_with_project("Test");

    // Create two sections
    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0)],
    });
    let section_a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let section_b = result_b.section_id.unwrap();

    // Merge them so they're connected
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(section_a, 1),
        station_b: StationRef::new(section_b, 0),
    });
    assert!(merge_result.success, "Merge should succeed");

    // Create loop closure
    let closure_result = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(section_a, 0),
        station_b: StationRef::new(section_b, 1),
        closure_strategy: ClosureStrategy::Batch,
    });
    assert!(closure_result.success, "CreateLoopClosure should succeed");

    // Verify closure exists
    {
        let project = state.get_project_state().unwrap();
        assert_eq!(project.loop_closures.len(), 1, "Should have 1 closure");
    }

    // Undo: closure should be removed
    let undo_result = state.undo();
    assert!(undo_result.success, "Undo should succeed");

    {
        let project = state.get_project_state().unwrap();
        assert_eq!(
            project.loop_closures.len(),
            0,
            "Closure should be removed after undo"
        );
    }

    // Redo: closure should be re-added
    let redo_result = state.redo();
    assert!(redo_result.success, "Redo should succeed");

    {
        let project = state.get_project_state().unwrap();
        assert_eq!(
            project.loop_closures.len(),
            1,
            "Closure should be restored after redo"
        );
    }
}

#[test]
fn test_undo_create_loop_closure_has_closure_info() {
    let state = state_with_project("Test");

    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0)],
    });
    let section_a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let section_b = result_b.section_id.unwrap();

    state.execute(Command::MergeStations {
        station_a: StationRef::new(section_a, 1),
        station_b: StationRef::new(section_b, 0),
    });

    state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(section_a, 0),
        station_b: StationRef::new(section_b, 1),
        closure_strategy: ClosureStrategy::Batch,
    });

    // Check undo stack has ClosureInfo data
    let proj = state.get_project_state().unwrap();
    let stack = state.undo_stack.lock();
    let undo_data = stack.peek_undo_data().unwrap();
    match undo_data {
        UndoData::ClosureInfo { closure, .. } => {
            assert!(proj.key_is_ref(&closure.station_a, StationRef::new(section_a, 0)));
            assert!(proj.key_is_ref(&closure.station_b, StationRef::new(section_b, 1)));
            assert!(closure.path_length > 0.0);
        }
        other => panic!("Expected ClosureInfo, got {:?}", other.variant_name()),
    }
}

#[test]
fn test_create_loop_closure_deferred_strategy_is_rejected() {
    let state = state_with_project("DistUndo");

    let result = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0), shot(6.0)],
    });
    let section_id = result.section_id.unwrap();

    let before_project = state.get_project_state().unwrap();
    let before_shots = before_project
        .sections
        .iter()
        .find(|s| s.id == section_id)
        .unwrap()
        .shots
        .clone();

    let closure_result = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(section_id, 0),
        station_b: StationRef::new(section_id, 2),
        closure_strategy: ClosureStrategy::ProportionalByLength,
    });
    assert!(!closure_result.success);

    let after_project = state.get_project_state().unwrap();
    let after_shots = after_project
        .sections
        .iter()
        .find(|s| s.id == section_id)
        .unwrap()
        .shots
        .clone();
    assert!((before_shots[0].length - after_shots[0].length).abs() < 1e-6);
}

#[test]
fn test_undo_unmerge_restores_removed_closures() {
    let state = state_with_project("Test");

    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0)],
    });
    let section_a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let section_b = result_b.section_id.unwrap();

    let merge_station_a = StationRef::new(section_a, 1);
    let merge_station_b = StationRef::new(section_b, 0);

    // Merge A.1 with B.0
    state.execute(Command::MergeStations {
        station_a: merge_station_a,
        station_b: merge_station_b,
    });

    // Inject a closure matching the merge pair directly into project state.
    // (create_loop_closure won't let us create one for merged-pair stations,
    // so we simulate legacy data by injecting directly.)
    {
        let mut proj_guard = state.project.lock();
        let project = proj_guard.as_mut().unwrap();
        project.loop_closures.push(LoopClosure {
            id: 1,
            station_a: merge_station_a.into(),
            station_b: merge_station_b.into(),
            path_length: 15.0,
            misclosure_distance: 0.3,
            error_percentage: 2.0,
            horizontal_misclosure: 0.2,
            vertical_misclosure: 0.1,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
    }

    {
        let project = state.get_project_state().unwrap();
        assert_eq!(
            project.loop_closures.len(),
            1,
            "Should have 1 closure before unmerge"
        );
    }

    // Unmerge the stations - should remove closures matching the merge pair (A.1, B.0)
    let unmerge_result = state.execute(Command::UnmergeStations {
        station_ref: merge_station_a,
    });
    assert!(unmerge_result.success, "Unmerge should succeed");

    {
        let project = state.get_project_state().unwrap();
        assert!(project.station_merges.is_empty(), "Merge should be removed");
        assert_eq!(
            project.loop_closures.len(),
            0,
            "Closure matching merge pair should be removed by unmerge"
        );
    }

    // Undo the unmerge - should restore both the merge AND the removed closure
    let undo_result = state.undo();
    assert!(undo_result.success, "Undo unmerge should succeed");

    {
        let project = state.get_project_state().unwrap();
        assert_eq!(project.station_merges.len(), 1, "Merge should be restored");
        assert_eq!(
            project.loop_closures.len(),
            1,
            "Closure should be restored after undo of unmerge"
        );
        let restored_closure = &project.loop_closures[0];
        assert!(project.key_is_ref(&restored_closure.station_a, merge_station_a));
        assert!(project.key_is_ref(&restored_closure.station_b, merge_station_b));
        assert!((restored_closure.path_length - 15.0).abs() < 1e-6);
    }
}

// === Move Station Tests ===

#[test]
fn test_move_station_updates_shot_data() {
    let state = state_with_project("Test");

    // Create section with a shot pointing due east (azimuth 90°), length 10
    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        }],
    });

    let section_id = create_result.section_id.unwrap();
    let station_ref = StationRef::new(section_id, 1); // last station

    // Move station 1 by (5, 0, 0) - extend eastward
    // Shot 0: vector was (10, 0, 0), now (15, 0, 0)
    let result = state.execute(Command::MoveStation {
        station_ref,
        offset: Coordinates::new(5.0, 0.0, 0.0),
    });
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    let shot = &project.get_section(section_id).unwrap().shots[0];
    assert!(
        (shot.length - 15.0).abs() < 1e-6,
        "Length should be 15.0, got {}",
        shot.length
    );
    assert!(
        (shot.azimuth_in - 90.0).abs() < 1e-6,
        "Azimuth should still be 90°"
    );
    assert!(
        !project.station_offset_for_ref(station_ref).is_some(),
        "Station offset should be cleared"
    );

    // Second move: (0, 10, 3) - shot vector becomes (15, 10, 3)
    let result = state.execute(Command::MoveStation {
        station_ref,
        offset: Coordinates::new(0.0, 10.0, 3.0),
    });
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    let shot = &project.get_section(section_id).unwrap().shots[0];
    let expected_len = (15.0_f64.powi(2) + 10.0_f64.powi(2) + 3.0_f64.powi(2)).sqrt(); // slope length
    assert!(
        (shot.length - expected_len).abs() < 1e-6,
        "Length should be {}, got {}",
        expected_len,
        shot.length
    );
    let expected_azi = (15.0_f64).atan2(10.0).to_degrees(); // atan2(dx, dy)
    assert!(
        (shot.azimuth_in - expected_azi).abs() < 1e-6,
        "Azimuth should be {:.1}°, got {:.1}°",
        expected_azi,
        shot.azimuth_in
    );
    assert!(
        (shot.depth_out - 3.0).abs() < 1e-6,
        "Depth out should be 3.0, got {}",
        shot.depth_out
    );
}

#[test]
fn test_move_station_zero_updates_section_offset() {
    let state = state_with_project("Test");

    // Create section with a shot pointing due east (azimuth 90°), length 10
    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        }],
    });

    let section_id = create_result.section_id.unwrap();
    let station_0 = StationRef::new(section_id, 0);
    let station_1 = StationRef::new(section_id, 1);

    let before = state.get_project_state().unwrap();
    let before_station_1 = before.compute_station_raw_coords(station_1).unwrap();

    // Move station 0 by (5, 0, 0) - should shift section offset and keep station 1 fixed
    let result = state.execute(Command::MoveStation {
        station_ref: station_0,
        offset: Coordinates::new(5.0, 0.0, 0.0),
    });
    assert!(result.success);

    let after = state.get_project_state().unwrap();
    let after_station_0 = after.compute_station_raw_coords(station_0).unwrap();
    let after_station_1 = after.compute_station_raw_coords(station_1).unwrap();

    assert!(
        (after_station_0.x - 5.0).abs() < 1e-6,
        "Station 0 x should be 5.0"
    );
    assert!(
        (after_station_0.y).abs() < 1e-6,
        "Station 0 y should remain 0.0"
    );
    assert!(
        (after_station_1.x - before_station_1.x).abs() < 1e-6,
        "Station 1 x should remain unchanged"
    );
    assert!(
        (after_station_1.y - before_station_1.y).abs() < 1e-6,
        "Station 1 y should remain unchanged"
    );

    let offset = after
        .section_offsets()
        .get(&section_id)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!(
        (offset.0 - 5.0).abs() < 1e-6,
        "Section offset x should be 5.0"
    );
    assert!(
        (offset.1).abs() < 1e-6,
        "Section offset y should remain 0.0"
    );
}

#[test]
fn test_move_station_undo() {
    let state = state_with_project("Test");

    // Due east shot, length 10
    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        }],
    });

    let section_id = create_result.section_id.unwrap();
    let station_ref = StationRef::new(section_id, 0); // first station

    // Move station 0 by (5, 0, 0) - following shot vector -= (5, 0, 0) → (5, 0, 0)
    state.execute(Command::MoveStation {
        station_ref,
        offset: Coordinates::new(5.0, 0.0, 0.0),
    });

    let project = state.get_project_state().unwrap();
    let shot = &project.get_section(section_id).unwrap().shots[0];
    assert!(
        (shot.length - 5.0).abs() < 1e-6,
        "Shot length after move should be 5.0"
    );

    // Undo should restore original shot data
    let undo_result = state.undo();
    assert!(undo_result.success);

    let project = state.get_project_state().unwrap();
    let shot = &project.get_section(section_id).unwrap().shots[0];
    assert!(
        (shot.length - 10.0).abs() < 1e-6,
        "Undo should restore length to 10.0, got {}",
        shot.length
    );
    assert!(
        (shot.azimuth_in - 90.0).abs() < 1e-6,
        "Undo should restore azimuth_in to 90.0"
    );
    assert!(
        (shot.azimuth_out - 90.0).abs() < 1e-6,
        "Undo should restore azimuth_out to 90.0"
    );
    assert!(
        (shot.depth_in - 0.0).abs() < 1e-6,
        "Undo should restore depth_in to 0.0"
    );
    assert!(
        (shot.depth_out - 0.0).abs() < 1e-6,
        "Undo should restore depth_out to 0.0"
    );
}

#[test]
fn test_move_station_redo() {
    let state = state_with_project("Test");

    // Due east shot, length 10
    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        }],
    });

    let section_id = create_result.section_id.unwrap();
    let station_ref = StationRef::new(section_id, 1); // last station

    // Move station 1 by (5, 0, 0) - preceding shot vector += (5, 0, 0) → (15, 0, 0)
    state.execute(Command::MoveStation {
        station_ref,
        offset: Coordinates::new(5.0, 0.0, 0.0),
    });

    let project = state.get_project_state().unwrap();
    let shot = &project.get_section(section_id).unwrap().shots[0];
    assert!(
        (shot.length - 15.0).abs() < 1e-6,
        "After move: length should be 15.0"
    );

    // Undo
    let undo_result = state.undo();
    assert!(undo_result.success);

    let project = state.get_project_state().unwrap();
    let shot = &project.get_section(section_id).unwrap().shots[0];
    assert!(
        (shot.length - 10.0).abs() < 1e-6,
        "After undo: length should be 10.0"
    );

    // Redo should re-apply the move
    let redo_result = state.redo();
    assert!(redo_result.success);

    let project = state.get_project_state().unwrap();
    let shot = &project.get_section(section_id).unwrap().shots[0];
    assert!(
        (shot.length - 15.0).abs() < 1e-6,
        "After redo: length should be 15.0, got {}",
        shot.length
    );
    assert!(
        (shot.azimuth_in - 90.0).abs() < 1e-6,
        "After redo: azimuth should be 90°"
    );
}

#[test]
fn test_station_offset_applied_in_coordinates() {
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });

    let section_id = create_result.section_id.unwrap();
    let station_ref = StationRef::new(section_id, 1); // end station of the shot

    // Compute coordinates before any move
    let coords_before = {
        let project = state.get_project_state().unwrap();
        project.compute_station_raw_coords(station_ref).unwrap()
    };

    // Move the station
    let move_offset = Coordinates::new(100.0, 200.0, 50.0);
    let result = state.execute(Command::MoveStation {
        station_ref,
        offset: move_offset,
    });
    assert!(result.success);

    // Compute coordinates after move
    let coords_after = {
        let project = state.get_project_state().unwrap();
        project.compute_station_raw_coords(station_ref).unwrap()
    };

    // The station coordinates should differ by exactly the move offset
    let dx = coords_after.x - coords_before.x;
    let dy = coords_after.y - coords_before.y;
    let dz = coords_after.z - coords_before.z;

    assert!(
        (dx - 100.0).abs() < 1e-6,
        "X coordinate should shift by 100.0, got delta {dx}"
    );
    assert!(
        (dy - 200.0).abs() < 1e-6,
        "Y coordinate should shift by 200.0, got delta {dy}"
    );
    assert!(
        (dz - 50.0).abs() < 1e-6,
        "Z coordinate should shift by 50.0, got delta {dz}"
    );
}

// === CreateSection undo/redo ID preservation tests ===

#[test]
fn test_create_section_undo_redo_preserves_id() {
    let state = state_with_project("Test");

    // Create a section with shots
    let create_result = state.execute(Command::CreateSection {
        name: Some("ID Preservation".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(10.0)],
    });

    let original_section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(project.sections[0].id, original_section_id);
    let original_shot_ids: Vec<_> = project.sections[0].shots.iter().map(|s| s.id).collect();
    assert_eq!(original_shot_ids.len(), 2);

    // Undo the create
    let undo_result = state.undo();
    assert!(undo_result.success);
    assert!(state.get_project_state().unwrap().sections.is_empty());

    // Redo the create - should preserve original section ID
    let redo_result = state.redo();
    assert!(redo_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(
        project.sections[0].id, original_section_id,
        "Section ID must be preserved after undo/redo, expected {} got {}",
        original_section_id, project.sections[0].id
    );
    assert_eq!(
        project.sections[0].name,
        Some("ID Preservation".to_string())
    );

    // Verify shot IDs are preserved too
    let restored_shot_ids: Vec<_> = project.sections[0].shots.iter().map(|s| s.id).collect();
    assert_eq!(
        restored_shot_ids, original_shot_ids,
        "Shot IDs must be preserved after undo/redo"
    );
}

#[test]
fn test_create_section_double_undo_redo_cycle() {
    let state = state_with_project("Test");

    // Create a section with shots
    let create_result = state.execute(Command::CreateSection {
        name: Some("Double Cycle".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(3.0), shot(7.0), shot(11.0)],
    });

    let original_section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    let original_shot_ids: Vec<_> = project.sections[0].shots.iter().map(|s| s.id).collect();

    // First cycle: undo -> redo
    assert!(state.undo().success);
    assert!(state.get_project_state().unwrap().sections.is_empty());
    assert!(state.redo().success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].id, original_section_id);
    let shot_ids_after_first: Vec<_> = project.sections[0].shots.iter().map(|s| s.id).collect();
    assert_eq!(shot_ids_after_first, original_shot_ids);

    // Second cycle: undo -> redo
    assert!(state.undo().success);
    assert!(state.get_project_state().unwrap().sections.is_empty());
    assert!(state.redo().success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections[0].id, original_section_id,
        "Section ID must be stable through double undo/redo cycle"
    );
    let shot_ids_after_second: Vec<_> = project.sections[0].shots.iter().map(|s| s.id).collect();
    assert_eq!(
        shot_ids_after_second, original_shot_ids,
        "Shot IDs must be stable through double undo/redo cycle"
    );
}

#[test]
fn test_create_section_undo_redo_then_edit() {
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("Editable".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });

    let section_id = create_result.section_id.unwrap();

    // Undo and redo
    assert!(state.undo().success);
    assert!(state.redo().success);

    // Verify we can edit the restored section
    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    let update_result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(99.0),
            ..Default::default()
        },
    });
    assert!(update_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots[0].length, 99.0);
}

#[test]
fn test_create_section_undo_redo_then_delete() {
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("Deletable".to_string()),
        surveyor: None,
        date: None,
        color: "#ff00ff".to_string(),
        direction: None,
        shots: vec![],
    });

    let section_id = create_result.section_id.unwrap();

    // Undo and redo
    assert!(state.undo().success);
    assert!(state.redo().success);

    // Delete the restored section
    let delete_result = state.execute(Command::DeleteSection { section_id });
    assert!(delete_result.success);
    assert!(state.get_project_state().unwrap().sections.is_empty());
}

#[test]
fn test_create_section_created_snapshot_distinct_from_section_snapshot() {
    // Verify that CreateSection uses CreatedSectionSnapshot while
    // DeleteSection still uses SectionSnapshot
    let state = state_with_project("Test");

    // Create a section
    let create_result = state.execute(Command::CreateSection {
        name: Some("Check Variant".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(1.0)],
    });
    let section_id = create_result.section_id.unwrap();

    // Check that CreateSection uses CreatedSectionSnapshot
    {
        let stack = state.undo_stack.lock();
        let undo_data = stack.peek_undo_data().unwrap();
        assert_eq!(
            undo_data.variant_name(),
            "CreatedSectionSnapshot",
            "CreateSection should use CreatedSectionSnapshot, got {}",
            undo_data.variant_name()
        );
    }

    // Now delete the section and verify DeleteSection uses ProjectSnapshotPair
    // (changed from SectionSnapshot to support cleanup of orphaned merges/closures/offsets)
    state.execute(Command::DeleteSection { section_id });

    {
        let stack = state.undo_stack.lock();
        let undo_data = stack.peek_undo_data().unwrap();
        assert_eq!(
            undo_data.variant_name(),
            "ProjectSnapshotPair",
            "DeleteSection should use ProjectSnapshotPair, got {}",
            undo_data.variant_name()
        );
    }
}

#[test]
fn test_delete_section_undo_redo_still_works() {
    // Regression test: ensure DeleteSection undo/redo behavior is unchanged
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("Delete Test".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });

    let section_id = create_result.section_id.unwrap();

    // Delete the section
    state.execute(Command::DeleteSection { section_id });
    assert!(state.get_project_state().unwrap().sections.is_empty());

    // Undo should restore it
    assert!(state.undo().success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(project.sections[0].id, section_id);
    assert_eq!(project.sections[0].name, Some("Delete Test".to_string()));

    // Redo should delete it again
    assert!(state.redo().success);
    assert!(state.get_project_state().unwrap().sections.is_empty());

    // Undo again to restore
    assert!(state.undo().success);
    assert_eq!(state.get_project_state().unwrap().sections.len(), 1);
}
