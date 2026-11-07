//! CreateShot undo id preservation; zero datum merge validation.
//!
//! Lines 10850-11729 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// === CreateShot undo/redo ID preservation tests ===

#[test]
fn test_create_shot_undo_redo_preserves_id() {
    let state = state_with_project("Test");

    // Create a section first
    let section_result = state.execute(Command::CreateSection {
        name: Some("Shot Test".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let section_id = section_result.section_id.unwrap();

    // Get the initial shot ID
    let project = state.get_project_state().unwrap();
    let initial_shot_id = project.sections[0].shots[0].id;

    // Create a new shot
    state.execute(Command::CreateShot {
        section_id,
        shot: shot(10.0),
        after_shot_id: None,
        before_shot_id: None,
    });

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 2);
    let new_shot_id = project.sections[0].shots[1].id;
    assert_ne!(new_shot_id, initial_shot_id);

    // Undo the create shot
    let undo_result = state.undo();
    assert!(undo_result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 1);

    // Redo the create shot - should preserve original shot ID
    let redo_result = state.redo();
    assert!(redo_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 2);
    assert_eq!(
        project.sections[0].shots[1].id, new_shot_id,
        "Shot ID must be preserved after undo/redo, expected {} got {}",
        new_shot_id, project.sections[0].shots[1].id
    );
    assert_eq!(project.sections[0].shots[1].length, 10.0);
}

#[test]
fn test_create_shot_mid_section_undo_redo_preserves_position() {
    let state = state_with_project("Test");

    // Create a section with 3 shots
    let section_result = state.execute(Command::CreateSection {
        name: Some("Mid Insert".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(1.0), shot(2.0), shot(3.0)],
    });
    let section_id = section_result.section_id.unwrap();

    let project = state.get_project_state().unwrap();
    let first_shot_id = project.sections[0].shots[0].id;
    let original_ids: Vec<_> = project.sections[0].shots.iter().map(|s| s.id).collect();
    assert_eq!(original_ids.len(), 3);

    // Insert a shot after the first shot (mid-section insert)
    state.execute(Command::CreateShot {
        section_id,
        shot: shot(99.0),
        after_shot_id: Some(first_shot_id),
        before_shot_id: None,
    });

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 4);
    // The new shot should be at position 1 (after first)
    let inserted_shot_id = project.sections[0].shots[1].id;
    assert_eq!(project.sections[0].shots[1].length, 99.0);

    // Verify order: [original[0], new, original[1], original[2]]
    assert_eq!(project.sections[0].shots[0].id, original_ids[0]);
    assert_eq!(project.sections[0].shots[1].id, inserted_shot_id);
    assert_eq!(project.sections[0].shots[2].id, original_ids[1]);
    assert_eq!(project.sections[0].shots[3].id, original_ids[2]);

    // Undo the mid-section insert
    let undo_result = state.undo();
    assert!(undo_result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 3);
    // Should be back to original order
    let after_undo_ids: Vec<_> = project.sections[0].shots.iter().map(|s| s.id).collect();
    assert_eq!(after_undo_ids, original_ids);

    // Redo - should restore at position 1 with original ID
    let redo_result = state.redo();
    assert!(redo_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 4);
    assert_eq!(
        project.sections[0].shots[1].id, inserted_shot_id,
        "Shot ID must be preserved after mid-section undo/redo"
    );
    assert_eq!(project.sections[0].shots[1].length, 99.0);

    // Verify correct position restored: [original[0], new, original[1], original[2]]
    assert_eq!(project.sections[0].shots[0].id, original_ids[0]);
    assert_eq!(project.sections[0].shots[2].id, original_ids[1]);
    assert_eq!(project.sections[0].shots[3].id, original_ids[2]);
}

#[test]
fn test_create_shot_double_undo_redo_cycle() {
    let state = state_with_project("Test");

    let section_result = state.execute(Command::CreateSection {
        name: Some("Double Cycle".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let section_id = section_result.section_id.unwrap();

    // Create a shot
    state.execute(Command::CreateShot {
        section_id,
        shot: shot(20.0),
        after_shot_id: None,
        before_shot_id: None,
    });

    let project = state.get_project_state().unwrap();
    let original_shot_id = project.sections[0].shots[1].id;

    // First cycle: undo -> redo
    assert!(state.undo().success);
    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots.len(),
        1
    );
    assert!(state.redo().success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots[1].id, original_shot_id);

    // Second cycle: undo -> redo
    assert!(state.undo().success);
    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots.len(),
        1
    );
    assert!(state.redo().success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections[0].shots[1].id, original_shot_id,
        "Shot ID must be stable through double undo/redo cycle"
    );
    assert_eq!(project.sections[0].shots[1].length, 20.0);
}

#[test]
fn test_create_shot_undo_redo_then_edit() {
    let state = state_with_project("Test");

    let section_result = state.execute(Command::CreateSection {
        name: Some("Edit After Redo".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });
    let section_id = section_result.section_id.unwrap();

    // Create a shot
    state.execute(Command::CreateShot {
        section_id,
        shot: shot(15.0),
        after_shot_id: None,
        before_shot_id: None,
    });

    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    // Undo and redo
    assert!(state.undo().success);
    assert!(state.redo().success);

    // Verify we can edit the restored shot
    let update_result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(42.0),
            ..Default::default()
        },
    });
    assert!(update_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots[0].length, 42.0);
}

#[test]
fn test_create_shot_undo_redo_then_delete() {
    let state = state_with_project("Test");

    let section_result = state.execute(Command::CreateSection {
        name: Some("Delete After Redo".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });
    let section_id = section_result.section_id.unwrap();

    // Create a shot
    state.execute(Command::CreateShot {
        section_id,
        shot: shot(15.0),
        after_shot_id: None,
        before_shot_id: None,
    });

    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    // Undo and redo
    assert!(state.undo().success);
    assert!(state.redo().success);

    // Delete the restored shot
    let delete_result = state.execute(Command::DeleteShot {
        section_id,
        shot_id,
    });
    assert!(delete_result.success);
    assert!(state.get_project_state().unwrap().sections[0]
        .shots
        .is_empty());
}

#[test]
fn test_create_shot_created_snapshot_distinct_from_shot_snapshot() {
    // Verify that CreateShot uses CreatedShotSnapshot while
    // DeleteShot uses ProjectSnapshotPair
    let state = state_with_project("Test");

    let section_result = state.execute(Command::CreateSection {
        name: Some("Variant Check".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let section_id = section_result.section_id.unwrap();

    // Create a shot
    state.execute(Command::CreateShot {
        section_id,
        shot: shot(10.0),
        after_shot_id: None,
        before_shot_id: None,
    });

    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[1].id;

    // Check that CreateShot uses CreatedShotSnapshot
    {
        let stack = state.undo_stack.lock();
        let undo_data = stack.peek_undo_data().unwrap();
        assert_eq!(
            undo_data.variant_name(),
            "CreatedShotSnapshot",
            "CreateShot should use CreatedShotSnapshot, got {}",
            undo_data.variant_name()
        );
    }

    // Now delete the shot and verify DeleteShot uses ProjectSnapshotPair
    // (changed from ShotSnapshot to handle station ref shifting correctly)
    state.execute(Command::DeleteShot {
        section_id,
        shot_id,
    });

    {
        let stack = state.undo_stack.lock();
        let undo_data = stack.peek_undo_data().unwrap();
        assert_eq!(
            undo_data.variant_name(),
            "ProjectSnapshotPair",
            "DeleteShot should use ProjectSnapshotPair, got {}",
            undo_data.variant_name()
        );
    }
}

#[test]
fn test_insert_shot_before_first_position_and_undo_redo() {
    let state = state_with_project("Test");

    let section_result = state.execute(Command::CreateSection {
        name: Some("Before Insert".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0), shot(20.0)],
    });
    let section_id = section_result.section_id.unwrap();

    let project = state.get_project_state().unwrap();
    let first_shot_id = project.sections[0].shots[0].id;

    // Insert before the first shot (position 0 / prepend)
    let result = state.execute(Command::CreateShot {
        section_id,
        shot: shot(5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(result.success);
    assert!(
        result.shot_id.is_some(),
        "shot_id must be returned on success"
    );
    let new_shot_id = result.shot_id.unwrap();

    // Verify insertion at position 0
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 3);
    assert_eq!(
        project.sections[0].shots[0].length, 5.0,
        "Prepended shot should be first"
    );
    assert_eq!(project.sections[0].shots[0].id, new_shot_id);
    assert_eq!(project.sections[0].shots[1].length, 10.0);
    assert_eq!(project.sections[0].shots[2].length, 20.0);

    // Undo the prepend
    let undo_result = state.undo();
    assert!(undo_result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 2);
    assert_eq!(project.sections[0].shots[0].length, 10.0);
    assert_eq!(project.sections[0].shots[1].length, 20.0);

    // Redo the prepend - should restore at position 0 with same ID
    let redo_result = state.redo();
    assert!(redo_result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 3);
    assert_eq!(project.sections[0].shots[0].length, 5.0);
    assert_eq!(
        project.sections[0].shots[0].id, new_shot_id,
        "Shot ID must be preserved after undo/redo"
    );
}

#[test]
fn test_prepend_shot_undo_preserves_geometry() {
    let state = state_with_project("Test");

    let section_result = state.execute(Command::CreateSection {
        name: Some("Prepend Geometry".to_string()),
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
    let section_id = section_result.section_id.unwrap();

    let before = state.get_project_state().unwrap();
    let before_station_0 = before
        .compute_station_raw_coords(StationRef::new(section_id, 0))
        .unwrap();
    let before_station_1 = before
        .compute_station_raw_coords(StationRef::new(section_id, 1))
        .unwrap();
    let first_shot_id = before.sections[0].shots[0].id;

    let result = state.execute(Command::CreateShot {
        section_id,
        shot: ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 5.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        },
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(result.success);

    let after_prepend = state.get_project_state().unwrap();
    let station_1_after = after_prepend
        .compute_station_raw_coords(StationRef::new(section_id, 1))
        .unwrap();
    let station_2_after = after_prepend
        .compute_station_raw_coords(StationRef::new(section_id, 2))
        .unwrap();
    assert_coords_close(station_1_after, before_station_0);
    assert_coords_close(station_2_after, before_station_1);

    let undo_result = state.undo();
    assert!(undo_result.success);
    let after_undo = state.get_project_state().unwrap();
    let undo_station_0 = after_undo
        .compute_station_raw_coords(StationRef::new(section_id, 0))
        .unwrap();
    let undo_station_1 = after_undo
        .compute_station_raw_coords(StationRef::new(section_id, 1))
        .unwrap();
    assert_coords_close(undo_station_0, before_station_0);
    assert_coords_close(undo_station_1, before_station_1);
}

#[test]
fn test_create_shot_rejects_both_after_and_before() {
    let state = state_with_project("Test");

    let section_result = state.execute(Command::CreateSection {
        name: Some("Reject Both".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0), shot(20.0)],
    });
    let section_id = section_result.section_id.unwrap();

    let result = state.execute(Command::CreateShot {
        section_id,
        shot: shot(5.0),
        after_shot_id: Some(0),
        before_shot_id: Some(1),
    });
    assert!(!result.success);
    assert!(result
        .error
        .as_deref()
        .unwrap()
        .contains("Cannot specify both"));
}

#[test]
fn test_delete_shot_undo_redo_still_works() {
    // Regression test: ensure DeleteShot undo/redo behavior is unchanged
    let state = state_with_project("Test");

    let section_result = state.execute(Command::CreateSection {
        name: Some("Delete Shot Regression".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(10.0)],
    });
    let section_id = section_result.section_id.unwrap();

    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    // Delete the first shot
    state.execute(Command::DeleteShot {
        section_id,
        shot_id,
    });
    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots.len(),
        1
    );

    // Undo should restore it at its original position
    assert!(state.undo().success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 2);
    assert_eq!(project.sections[0].shots[0].id, shot_id);
    assert_eq!(project.sections[0].shots[0].length, 5.0);

    // Redo should delete it again
    assert!(state.redo().success);
    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots.len(),
        1
    );

    // Undo again to restore
    assert!(state.undo().success);
    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots.len(),
        2
    );
}

// === Zero datum merge validation tests ===

#[test]
fn test_check_merge_connectivity_both_zero_datums_blocked() {
    // Both sections have zero datum → can_merge=false
    let state = state_with_project("BothZD");

    let result_a = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0)],
    });
    let a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let b = result_b.section_id.unwrap();

    // Set zero datum on both sections
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(b, 0),
        coordinates: GpsCoordinates::new(46.0, -91.0, 0.0).unwrap(),
    });

    let result = state
        .check_merge_connectivity(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    assert!(
        !result.can_merge,
        "Both sections with zero datums should not be mergeable"
    );
    assert!(
        !result.connected,
        "Unconnected sections should not be connected"
    );
    assert_eq!(
        result.reason.as_deref(),
        Some("Both sections have fixed coordinates"),
    );
}

#[test]
fn test_check_merge_connectivity_one_zero_datum_allowed() {
    // Only one section has zero datum → can_merge=true
    let state = state_with_project("OneZD");

    let result_a = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0)],
    });
    let a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let b = result_b.section_id.unwrap();

    // Set zero datum on section A only
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });

    let result = state
        .check_merge_connectivity(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    assert!(
        result.can_merge,
        "One section with zero datum should allow merge"
    );
    assert!(!result.connected);
    assert!(result.reason.is_none());
}

#[test]
fn test_check_merge_connectivity_neither_zero_datum_no_loop() {
    // Neither section has zero datum, no loop → can_merge=true
    let state = state_with_project("NoZD");

    let result_a = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0)],
    });
    let a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let b = result_b.section_id.unwrap();

    let result = state
        .check_merge_connectivity(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    assert!(
        result.can_merge,
        "Neither having zero datum and no loop should allow merge"
    );
    assert!(!result.connected);
    assert!(result.reason.is_none());
}

#[test]
fn test_check_merge_connectivity_connected_creates_loop() {
    // Connected sections (neither has zero datum, loop exists) → can_merge=false
    let state = state_with_project("Loop");

    let result_a = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0)],
    });
    let a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let b = result_b.section_id.unwrap();

    // Merge A.1 to B.0 to make them connected
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });

    // Now check connectivity between A.0 and B.1 - they are connected via merge
    let result = state
        .check_merge_connectivity(StationRef::new(a, 0), StationRef::new(b, 1))
        .unwrap();

    assert!(
        !result.can_merge,
        "Connected sections should not be mergeable (creates loop)"
    );
    assert!(result.connected, "Merged sections should be connected");
    assert_eq!(result.reason.as_deref(), Some("Creates survey loop"),);
    assert!(
        result.preview.is_some(),
        "Connected sections should have closure preview"
    );
}

#[test]
fn test_check_merge_connectivity_both_zd_and_connected() {
    // When sections are connected AND both have zero datums, zero datum
    // takes priority for reason, but connected/preview remain correct.
    let state = state_with_project("BothZDConnected");

    // Create sections A (with ZD), C (no ZD, will bridge), B (with ZD)
    let result_a = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0)],
    });
    let a = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let b = result_b.section_id.unwrap();

    let result_c = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![shot(3.0)],
    });
    let c = result_c.section_id.unwrap();

    // Set ZD on A
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });

    // Set ZD on B (separate group)
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(b, 0),
        coordinates: GpsCoordinates::new(46.0, -91.0, 0.0).unwrap(),
    });

    // Merge C.0 into A.1 (C joins A's merge group)
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(c, 0),
    });

    // Connect C.1 to B.0 via an injected closure (can't merge - different ZD groups,
    // and can't create closure through command because they're not yet connected).
    {
        let mut proj_guard = state.project.lock();
        let project = proj_guard.as_mut().unwrap();
        project.loop_closures.push(LoopClosure {
            id: project.next_closure_id,
            station_a: StationRef::new(c, 1).into(),
            station_b: StationRef::new(b, 0).into(),
            path_length: 8.0,
            misclosure_distance: 0.2,
            error_percentage: 2.5,
            horizontal_misclosure: 0.2,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
        project.next_closure_id += 1;
    }

    // Now: A-C merged (A has ZD), B has ZD, connected via closure C.1-B.0
    let result = state
        .check_merge_connectivity(StationRef::new(a, 0), StationRef::new(b, 1))
        .unwrap();

    // connected must be true (they ARE connected via merge+closure)
    assert!(
        result.connected,
        "Sections connected via merge+closure should report connected=true"
    );
    assert!(
        result.preview.is_some(),
        "Connected stations should have closure preview"
    );
    // can_merge must be false (both have ZD - zero datum takes priority)
    assert!(
        !result.can_merge,
        "Both sections with ZD should not be mergeable"
    );
    assert_eq!(
        result.reason.as_deref(),
        Some("Creates survey loop"),
        "Connected sections should report loop reason"
    );
}
