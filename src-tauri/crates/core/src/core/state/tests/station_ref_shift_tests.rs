//! Station ref shifting; section offsets for insert_before at pos 0.
//!
//! Lines 16103-17010 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project, straight_shot};

// === Station Ref Shifting Tests ===

/// Helper: create a project with two sections, each with 2 shots, merged at station 0 of each.
/// Returns (state, section_a_id, section_b_id).
fn setup_merged_project() -> (AppState, u32, u32) {
    let state = state_with_project("Shift Test");

    // Section A with 2 shots
    let result_a = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0), shot(20.0)],
    });
    let sec_a = result_a.section_id.unwrap();

    // Section B with 2 shots
    let result_b = state.execute(Command::CreateSection {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(15.0), shot(25.0)],
    });
    let sec_b = result_b.section_id.unwrap();

    // Merge station 0 of A with station 0 of B
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 0),
        station_b: StationRef::new(sec_b, 0),
    });
    assert!(merge_result.success, "Merge should succeed");

    (state, sec_a, sec_b)
}

#[test]
fn test_insert_before_shifts_station_merge() {
    let (state, sec_a, sec_b) = setup_merged_project();

    // Verify initial merge is at station 0 of both sections
    let proj = state.get_project_state().unwrap();
    assert_eq!(proj.station_merges.len(), 1);
    assert!(proj.key_is_ref(&proj.station_merges[0].station_a, StationRef::new(sec_a, 0)));
    assert!(proj.key_is_ref(&proj.station_merges[0].station_b, StationRef::new(sec_b, 0)));

    // Get first shot ID of section A
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;

    // Insert shot before first shot of section A
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: shot(5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Verify merge shifted: station_a should now be station 1 of A (not 0)
    let proj = state.get_project_state().unwrap();
    assert_eq!(proj.station_merges.len(), 1);
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_a, StationRef::new(sec_a, 1)),
        "Merge station_a should shift from 0 to 1 after insert_before"
    );
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_b, StationRef::new(sec_b, 0)),
        "Merge station_b (different section) should not shift"
    );
}

#[test]
fn test_insert_before_shifts_zero_datum() {
    let state = state_with_project("ZD Shift Test");

    let result = state.execute(Command::CreateSection {
        name: Some("Section".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0), shot(20.0)],
    });
    let sec_id = result.section_id.unwrap();

    // Set zero datum on station 0
    let zd_result = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_id, 0),
        coordinates: GpsCoordinates::new(45.0, -73.5, 100.0).unwrap(),
    });
    assert!(zd_result.success);

    let proj = state.get_project_state().unwrap();
    assert!(proj.key_is_ref(&proj.zero_datums[0].station_ref, StationRef::new(sec_id, 0)));

    // Get first shot ID
    let first_shot_id = proj.sections[0].shots[0].id;

    // Insert shot before first shot
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_id,
        shot: shot(5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Verify zero datum shifted to station 1
    let proj = state.get_project_state().unwrap();
    assert!(
        proj.key_is_ref(&proj.zero_datums[0].station_ref, StationRef::new(sec_id, 1)),
        "Zero datum should shift from station 0 to station 1 after insert_before"
    );
}

#[test]
fn test_insert_before_shifts_station_offset() {
    let state = state_with_project("Offset Shift Test");

    let result = state.execute(Command::CreateSection {
        name: Some("Section".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0), shot(20.0)],
    });
    let sec_id = result.section_id.unwrap();

    // Directly add a station offset at station 0
    {
        let mut proj_guard = state.project.lock();
        let project = proj_guard.as_mut().unwrap();
        project
            .station_offsets
            .insert(StationRef::new(sec_id, 0).into(), (1.0, 2.0, 3.0));
    }

    let proj = state.get_project_state().unwrap();
    assert!(proj
        .station_offset_for_ref(StationRef::new(sec_id, 0))
        .is_some());

    // Get first shot ID
    let first_shot_id = proj.sections[0].shots[0].id;

    // Insert before first shot
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_id,
        shot: shot(5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Verify station offset key shifted from station 0 to station 1
    let proj = state.get_project_state().unwrap();
    assert!(
        !proj
            .station_offset_for_ref(StationRef::new(sec_id, 0))
            .is_some(),
        "Station offset at old index 0 should be gone"
    );
    assert_eq!(
        proj.station_offset_for_ref(StationRef::new(sec_id, 1)),
        Some((1.0, 2.0, 3.0)),
        "Station offset should have moved to station 1"
    );
}

#[test]
fn test_insert_before_shifts_loop_closure() {
    let (state, sec_a, sec_b) = setup_merged_project();

    // Need a third section connected to form a loop for closure
    let result_c = state.execute(Command::CreateSection {
        name: Some("Section C".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![shot(12.0)],
    });
    let sec_c = result_c.section_id.unwrap();

    // Merge section C station 0 with section A station 2 (end of A)
    let merge2 = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 2),
        station_b: StationRef::new(sec_c, 0),
    });
    assert!(merge2.success);

    // Create loop closure between section B station 2 and section C station 1
    let closure_result = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(sec_b, 2),
        station_b: StationRef::new(sec_c, 1),
        closure_strategy: ClosureStrategy::Batch,
    });
    assert!(closure_result.success);

    let proj = state.get_project_state().unwrap();
    assert_eq!(proj.loop_closures.len(), 1);
    // The closure should reference sec_a station 2 for the merge
    assert_eq!(proj.station_merges.len(), 2);

    // Now insert shot before first shot of section A
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: shot(5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Verify: merge at sec_a station 0 should shift to station 1
    // Verify: merge at sec_a station 2 should shift to station 3
    let proj = state.get_project_state().unwrap();
    let a_merges: Vec<_> = proj
        .station_merges
        .iter()
        .filter(|m| m.station_a.section_id == sec_a || m.station_b.section_id == sec_a)
        .collect();
    assert_eq!(a_merges.len(), 2);

    // Check the merge that was at station 0 of A
    let merge_a0 = proj
        .station_merges
        .iter()
        .find(|m| proj.key_is_ref(&m.station_b, StationRef::new(sec_b, 0)))
        .unwrap();
    assert!(
        proj.key_is_ref(&merge_a0.station_a, StationRef::new(sec_a, 1)),
        "Merge at original station 0 should shift to station 1"
    );

    // Check the merge that was at station 2 of A (now station 3)
    let merge_a2 = proj
        .station_merges
        .iter()
        .find(|m| {
            proj.key_is_ref(&m.station_b, StationRef::new(sec_c, 0))
                || proj.key_is_ref(&m.station_a, StationRef::new(sec_c, 0))
        })
        .unwrap();
    let a_ref = if merge_a2.station_a.section_id == sec_a {
        merge_a2.station_a.clone()
    } else {
        merge_a2.station_b.clone()
    };
    assert!(
        proj.key_is_ref(&a_ref, StationRef::new(sec_a, 3)),
        "Merge at original station 2 should shift to station 3"
    );

    // Loop closure refs should NOT have changed (they reference sec_b and sec_c, not sec_a)
    assert!(proj.key_is_ref(&proj.loop_closures[0].station_a, StationRef::new(sec_b, 2)));
    assert!(proj.key_is_ref(&proj.loop_closures[0].station_b, StationRef::new(sec_c, 1)));
}

#[test]
fn test_insert_before_undo_restores_refs() {
    let (state, sec_a, sec_b) = setup_merged_project();

    let proj = state.get_project_state().unwrap();
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;

    // Insert before first shot of section A
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: shot(5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Verify merge shifted
    let proj = state.get_project_state().unwrap();
    assert!(proj.key_is_ref(&proj.station_merges[0].station_a, StationRef::new(sec_a, 1)));

    // Undo the insert
    let undo_result = state.undo();
    assert!(undo_result.success);

    // Verify merge returned to original position
    let proj = state.get_project_state().unwrap();
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_a, StationRef::new(sec_a, 0)),
        "Merge should return to station 0 after undo"
    );
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_b, StationRef::new(sec_b, 0)),
        "Merge station_b should still be station 0 of B"
    );
}

#[test]
fn test_insert_before_redo_shifts_refs_again() {
    let (state, sec_a, sec_b) = setup_merged_project();

    let proj = state.get_project_state().unwrap();
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;

    // Insert before first shot
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: shot(5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Undo
    let undo_result = state.undo();
    assert!(undo_result.success);

    // Redo
    let redo_result = state.redo();
    assert!(redo_result.success);

    // Verify merge shifted back to station 1
    let proj = state.get_project_state().unwrap();
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_a, StationRef::new(sec_a, 1)),
        "Merge should shift to station 1 after redo"
    );
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_b, StationRef::new(sec_b, 0)),
        "Merge station_b should stay at station 0 of B"
    );
}

#[test]
fn test_insert_after_only_shifts_refs_beyond() {
    let (state, sec_a, _sec_b) = setup_merged_project();

    // Also add a station offset at station 1 of section A
    {
        let mut proj_guard = state.project.lock();
        let project = proj_guard.as_mut().unwrap();
        project
            .station_offsets
            .insert(StationRef::new(sec_a, 1).into(), (5.0, 6.0, 7.0));
    }

    let proj = state.get_project_state().unwrap();
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;

    // Insert after first shot (position 0) - new shot at position 1
    // This should shift station refs >= station 2 (pos+2=2)
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: shot(7.0),
        after_shot_id: Some(first_shot_id),
        before_shot_id: None,
    });
    assert!(insert_result.success);

    let proj = state.get_project_state().unwrap();

    // Station merge at sec_a station 0 should NOT shift (0 < 2)
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_a, StationRef::new(sec_a, 0)),
        "Merge at station 0 should not shift for insert_after shot 0"
    );

    // Station offset at station 1 should NOT shift (1 < 2)
    assert!(
        proj.station_offset_for_ref(StationRef::new(sec_a, 1))
            .is_some(),
        "Station offset at station 1 should not shift for insert_after shot 0"
    );
}

#[test]
fn test_delete_shot_undo_redo_with_snapshot() {
    let (state, sec_a, _sec_b) = setup_merged_project();

    // Also set a zero datum at station 2 of section A
    let zd_result = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_a, 2),
        coordinates: GpsCoordinates::new(45.0, -73.5, 100.0).unwrap(),
    });
    assert!(zd_result.success);

    let proj = state.get_project_state().unwrap();
    let sec_a_shots: Vec<_> = proj
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots
        .iter()
        .collect();
    let first_shot_id = sec_a_shots[0].id;

    // Delete first shot - station refs should shift
    let delete_result = state.execute(Command::DeleteShot {
        section_id: sec_a,
        shot_id: first_shot_id,
    });
    assert!(delete_result.success);

    let proj = state.get_project_state().unwrap();
    // After deleting shot 0: merge referencing deleted station 0 should be removed
    assert!(
        proj.station_merges.is_empty(),
        "Merges referencing deleted station should be removed"
    );
    // Zero datum was at station 2, should shift to station 1
    assert!(
        proj.key_is_ref(&proj.zero_datums[0].station_ref, StationRef::new(sec_a, 1)),
        "Zero datum should shift from station 2 to station 1 after delete shot 0"
    );

    // Undo the delete - should restore via ProjectSnapshotPair
    let undo_result = state.undo();
    assert!(undo_result.success);

    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.sections
            .iter()
            .find(|s| s.id == sec_a)
            .unwrap()
            .shots
            .len(),
        2
    );
    assert!(
        proj.key_is_ref(&proj.station_merges[0].station_a, StationRef::new(sec_a, 0)),
        "Merge should be at station 0 after undo"
    );
    assert!(
        proj.key_is_ref(&proj.zero_datums[0].station_ref, StationRef::new(sec_a, 2)),
        "Zero datum should be at station 2 after undo"
    );

    // Redo the delete
    let redo_result = state.redo();
    assert!(redo_result.success);

    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.sections
            .iter()
            .find(|s| s.id == sec_a)
            .unwrap()
            .shots
            .len(),
        1
    );
    assert!(
        proj.key_is_ref(&proj.zero_datums[0].station_ref, StationRef::new(sec_a, 1)),
        "Zero datum should be at station 1 after redo"
    );
}

// === Section offset adjustment tests for insert_before at pos=0 ===

#[test]
fn test_insert_before_pos0_adjusts_section_offsets() {
    let state = state_with_project("Offset Test");

    // Section with 2 shots heading north (azimuth 0)
    let result = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![straight_shot(0.0, 10.0), straight_shot(0.0, 5.0)],
    });
    let sec_a = result.section_id.unwrap();

    // Capture initial coordinates
    let orig_coords: Vec<_> = {
        let proj = state.get_project_state().unwrap();
        let stations = proj.compute_station_coordinates();
        stations
            .iter()
            .filter(|s| s.station_ref.section_id == sec_a)
            .map(|s| {
                (
                    s.station_ref.station_index,
                    s.coordinates.x,
                    s.coordinates.y,
                )
            })
            .collect()
    };
    // station 0: (0, 0), station 1: (0, 10), station 2: (0, 15)
    assert!((orig_coords[0].1).abs() < 1e-6); // x=0
    assert!((orig_coords[0].2).abs() < 1e-6); // y=0
    assert!((orig_coords[1].2 - 10.0).abs() < 1e-6); // y=10
    assert!((orig_coords[2].2 - 15.0).abs() < 1e-6); // y=15

    // Insert shot heading east (azimuth 90, length 3) before first shot
    let proj = state.get_project_state().unwrap();
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: straight_shot(90.0, 3.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // After insert: old station 0 is now station 1.
    // New shot displacement: dx=3*sin(90)=3, dy=3*cos(90)=0
    // Section offset adjusted by (-3, 0)
    // Station 1 (old 0) should be at (0, 0) - unchanged
    // Station 2 (old 1) should be at (0, 10) - unchanged
    // Station 3 (old 2) should be at (0, 15) - unchanged
    let proj = state.get_project_state().unwrap();
    let stations = proj.compute_station_coordinates();

    let s0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_a, 0))
        .unwrap();
    let s1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_a, 1))
        .unwrap();
    let s2 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_a, 2))
        .unwrap();
    let s3 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_a, 3))
        .unwrap();

    // Station 0 (new) at section_offset = (-3, 0)
    assert!(
        (s0.coordinates.x - (-3.0)).abs() < 1e-6,
        "New station 0 x={}",
        s0.coordinates.x
    );
    assert!(
        (s0.coordinates.y).abs() < 1e-6,
        "New station 0 y={}",
        s0.coordinates.y
    );

    // Station 1 (was station 0): (0, 0) - same as original station 0
    assert!(
        (s1.coordinates.x).abs() < 1e-6,
        "Station 1 (was 0) x={}",
        s1.coordinates.x
    );
    assert!(
        (s1.coordinates.y).abs() < 1e-6,
        "Station 1 (was 0) y={}",
        s1.coordinates.y
    );

    // Station 2 (was station 1): (0, 10) - same as original station 1
    assert!(
        (s2.coordinates.x).abs() < 1e-6,
        "Station 2 (was 1) x={}",
        s2.coordinates.x
    );
    assert!(
        (s2.coordinates.y - 10.0).abs() < 1e-6,
        "Station 2 (was 1) y={}",
        s2.coordinates.y
    );

    // Station 3 (was station 2): (0, 15) - same as original station 2
    assert!(
        (s3.coordinates.x).abs() < 1e-6,
        "Station 3 (was 2) x={}",
        s3.coordinates.x
    );
    assert!(
        (s3.coordinates.y - 15.0).abs() < 1e-6,
        "Station 3 (was 2) y={}",
        s3.coordinates.y
    );
}

#[test]
fn test_insert_before_pos0_adjusts_merged_section_offsets() {
    let state = state_with_project("Merged Offset Test");

    // Section A: 2 shots north
    let result_a = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![straight_shot(0.0, 10.0), straight_shot(0.0, 5.0)],
    });
    let sec_a = result_a.section_id.unwrap();

    // Section B: 1 shot east
    let result_b = state.execute(Command::CreateSection {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![straight_shot(90.0, 8.0)],
    });
    let sec_b = result_b.section_id.unwrap();

    // Merge station 0 of A with station 0 of B
    state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 0),
        station_b: StationRef::new(sec_b, 0),
    });

    // Capture section B station coordinates before insert
    let (b0_before, b1_before) = {
        let proj = state.get_project_state().unwrap();
        let stations = proj.compute_station_coordinates();
        let b0 = stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(sec_b, 0))
            .unwrap()
            .coordinates;
        let b1 = stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(sec_b, 1))
            .unwrap()
            .coordinates;
        (b0, b1)
    };

    // Insert shot heading east before first shot of section A
    let proj = state.get_project_state().unwrap();
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: straight_shot(90.0, 3.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Section B is in the same merged group - its stations should NOT have moved
    let proj = state.get_project_state().unwrap();
    let stations = proj.compute_station_coordinates();
    let b0_after = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_b, 0))
        .unwrap()
        .coordinates;
    let b1_after = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_b, 1))
        .unwrap()
        .coordinates;

    assert!(
        (b0_after.x - b0_before.x).abs() < 1e-6,
        "B station 0 x moved: {} -> {}",
        b0_before.x,
        b0_after.x
    );
    assert!(
        (b0_after.y - b0_before.y).abs() < 1e-6,
        "B station 0 y moved: {} -> {}",
        b0_before.y,
        b0_after.y
    );
    assert!(
        (b1_after.x - b1_before.x).abs() < 1e-6,
        "B station 1 x moved: {} -> {}",
        b1_before.x,
        b1_after.x
    );
    assert!(
        (b1_after.y - b1_before.y).abs() < 1e-6,
        "B station 1 y moved: {} -> {}",
        b1_before.y,
        b1_after.y
    );
}

#[test]
fn test_insert_before_pos0_undo_restores_offsets() {
    let state = state_with_project("Undo Offset Test");

    let result = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![straight_shot(0.0, 10.0)],
    });
    let sec_a = result.section_id.unwrap();

    // Capture coordinates before insert
    let orig_coords: Vec<_> = {
        let proj = state.get_project_state().unwrap();
        let stations = proj.compute_station_coordinates();
        stations
            .iter()
            .filter(|s| s.station_ref.section_id == sec_a)
            .map(|s| (s.station_ref, s.coordinates))
            .collect()
    };

    // Insert shot heading east before first shot
    let proj = state.get_project_state().unwrap();
    let first_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[0].id;
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: straight_shot(90.0, 5.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot_id),
    });
    assert!(insert_result.success);

    // Verify offset was adjusted
    {
        let proj = state.get_project_state().unwrap();
        let offset = proj
            .section_offsets()
            .get(&sec_a)
            .copied()
            .unwrap_or((0.0, 0.0));
        assert!(
            (offset.0 - (-5.0)).abs() < 1e-6,
            "Section offset x should be -5, got {}",
            offset.0
        );
    }

    // Undo
    let undo_result = state.undo();
    assert!(undo_result.success);

    // Coordinates should be exactly as before (section has 2 stations again)
    let proj = state.get_project_state().unwrap();
    let stations = proj.compute_station_coordinates();
    for (ref_orig, coord_orig) in &orig_coords {
        let station = stations
            .iter()
            .find(|s| s.station_ref == *ref_orig)
            .unwrap();
        assert!(
            (station.coordinates.x - coord_orig.x).abs() < 1e-6,
            "Station {:?} x should match after undo: {} vs {}",
            ref_orig,
            station.coordinates.x,
            coord_orig.x
        );
        assert!(
            (station.coordinates.y - coord_orig.y).abs() < 1e-6,
            "Station {:?} y should match after undo: {} vs {}",
            ref_orig,
            station.coordinates.y,
            coord_orig.y
        );
    }

    // Redo
    let redo_result = state.redo();
    assert!(redo_result.success);

    // Old stations should be at the same position again (not moved)
    let proj = state.get_project_state().unwrap();
    let stations = proj.compute_station_coordinates();
    // Old station 0 is now station 1
    let s1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_a, 1))
        .unwrap();
    assert!(
        (s1.coordinates.x - orig_coords[0].1.x).abs() < 1e-6,
        "Station 1 (was 0) x after redo: {} vs {}",
        s1.coordinates.x,
        orig_coords[0].1.x
    );
    assert!(
        (s1.coordinates.y - orig_coords[0].1.y).abs() < 1e-6,
        "Station 1 (was 0) y after redo: {} vs {}",
        s1.coordinates.y,
        orig_coords[0].1.y
    );
    // Old station 1 is now station 2
    let s2 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_a, 2))
        .unwrap();
    assert!(
        (s2.coordinates.x - orig_coords[1].1.x).abs() < 1e-6,
        "Station 2 (was 1) x after redo: {} vs {}",
        s2.coordinates.x,
        orig_coords[1].1.x
    );
    assert!(
        (s2.coordinates.y - orig_coords[1].1.y).abs() < 1e-6,
        "Station 2 (was 1) y after redo: {} vs {}",
        s2.coordinates.y,
        orig_coords[1].1.y
    );
}

#[test]
fn test_insert_before_pos_gt0_no_offset_adjustment() {
    let state = state_with_project("No Offset Test");

    let result = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![straight_shot(0.0, 10.0), straight_shot(0.0, 5.0)],
    });
    let sec_a = result.section_id.unwrap();

    // Get the section offset before insert
    let offset_before = {
        let proj = state.get_project_state().unwrap();
        proj.section_offsets()
            .get(&sec_a)
            .copied()
            .unwrap_or((0.0, 0.0))
    };

    // Insert before the SECOND shot (pos=1, not pos=0)
    let proj = state.get_project_state().unwrap();
    let second_shot_id = proj.sections.iter().find(|s| s.id == sec_a).unwrap().shots[1].id;
    let insert_result = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: straight_shot(90.0, 3.0),
        after_shot_id: None,
        before_shot_id: Some(second_shot_id),
    });
    assert!(insert_result.success);

    // Section offset should NOT have changed (only pos=0 triggers adjustment)
    let proj = state.get_project_state().unwrap();
    let offset_after = proj
        .section_offsets()
        .get(&sec_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!(
        (offset_after.0 - offset_before.0).abs() < 1e-6,
        "Section offset x should not change for pos>0"
    );
    assert!(
        (offset_after.1 - offset_before.1).abs() < 1e-6,
        "Section offset y should not change for pos>0"
    );
}
