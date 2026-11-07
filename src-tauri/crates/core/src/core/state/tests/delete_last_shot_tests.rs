//! Delete_last_shot_and_section and section deletion cleanup.
//!
//! Lines 15424-16102 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// === delete_last_shot_and_section tests ===

#[test]
fn test_delete_last_shot_removes_section() {
    let state = state_with_project("Delete Last Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("Single".to_string()),
        surveyor: Some("Tester".to_string()),
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    assert!(create_result.success);
    let section_id = create_result.section_id.unwrap();

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[0].id
    };

    let result = state.execute_delete_last_shot_and_section(section_id, shot_id);
    assert!(
        result.success,
        "Delete last shot should succeed: {:?}",
        result.error
    );

    // Verify section is gone
    let proj = state.get_project_state().unwrap();
    assert!(
        proj.get_section(section_id).is_none(),
        "Section should be removed"
    );
}

#[test]
fn test_delete_last_shot_fails_if_multiple_shots() {
    let state = state_with_project("Multi Shot Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("Multi".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0)],
    });
    assert!(create_result.success);
    let section_id = create_result.section_id.unwrap();

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[0].id
    };

    let result = state.execute_delete_last_shot_and_section(section_id, shot_id);
    assert!(!result.success, "Should fail with multiple shots");
    assert!(result.error.unwrap().contains("2 shots"));
}

#[test]
fn test_delete_last_shot_undo_restores() {
    let state = state_with_project("Undo Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("WillDelete".to_string()),
        surveyor: Some("UndoTester".to_string()),
        date: None,
        color: "#aabbcc".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let section_id = create_result.section_id.unwrap();

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[0].id
    };

    let result = state.execute_delete_last_shot_and_section(section_id, shot_id);
    assert!(result.success);

    // Verify deleted
    {
        let proj = state.get_project_state().unwrap();
        assert!(proj.get_section(section_id).is_none());
    }

    // Undo
    let undo_result = state.undo();
    assert!(
        undo_result.success,
        "Undo should succeed: {:?}",
        undo_result.error
    );

    // Verify restored
    let proj = state.get_project_state().unwrap();
    let section = proj
        .get_section(section_id)
        .expect("Section should be restored");
    assert_eq!(section.name, Some("WillDelete".to_string()));
    assert_eq!(section.surveyor, Some("UndoTester".to_string()));
    assert_eq!(section.shots.len(), 1);
    assert_eq!(section.shots[0].id, shot_id);
    assert_eq!(section.shots[0].length, 5.0);
}

#[test]
fn test_delete_last_shot_cleans_up_offsets() {
    let state = state_with_project("Offset Cleanup Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("WithOffset".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let section_id = create_result.section_id.unwrap();

    // Manually set a section offset
    {
        let mut proj_guard = state.project.lock();
        let project = proj_guard.as_mut().unwrap();
        project.set_section_offset(section_id, (10.0, 20.0));
    }

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[0].id
    };

    let result = state.execute_delete_last_shot_and_section(section_id, shot_id);
    assert!(result.success);

    // Verify section offset is cleaned up
    let proj = state.get_project_state().unwrap();
    assert!(
        !proj.section_offsets().contains_key(&section_id),
        "Section offset should be removed"
    );
}

#[test]
fn test_delete_last_shot_cleans_up_merges() {
    let state = state_with_project("Merge Cleanup Test");

    // Create two sections
    let create_a = state.execute(Command::CreateSection {
        name: Some("ToDelete".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let section_a_id = create_a.section_id.unwrap();

    let create_b = state.execute(Command::CreateSection {
        name: Some("Other".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let section_b_id = create_b.section_id.unwrap();

    // Merge station 1 of A with station 0 of B
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(section_a_id, 1),
        station_b: StationRef::new(section_b_id, 0),
    });
    assert!(merge_result.success);

    // Verify merge exists
    {
        let proj = state.get_project_state().unwrap();
        assert!(
            !proj.station_merges.is_empty(),
            "Should have merge before delete"
        );
    }

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_a_id).unwrap().shots[0].id
    };

    let result = state.execute_delete_last_shot_and_section(section_a_id, shot_id);
    assert!(result.success);

    // Verify merge is cleaned up
    let proj = state.get_project_state().unwrap();
    let remaining_merges: Vec<_> = proj
        .station_merges
        .iter()
        .filter(|m| {
            m.station_a.section_id == section_a_id || m.station_b.section_id == section_a_id
        })
        .collect();
    assert!(
        remaining_merges.is_empty(),
        "Merges referencing deleted section should be removed"
    );
}

#[test]
fn test_delete_last_shot_cleans_up_station_offsets() {
    let state = state_with_project("Station Offset Cleanup Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("WithStationOffset".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let section_id = create_result.section_id.unwrap();

    // Manually set a station offset
    {
        let mut proj_guard = state.project.lock();
        let project = proj_guard.as_mut().unwrap();
        let station_ref = StationRef::new(section_id, 0);
        project
            .station_offsets
            .insert(station_ref.into(), (1.0, 2.0, 3.0));
    }

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[0].id
    };

    let result = state.execute_delete_last_shot_and_section(section_id, shot_id);
    assert!(result.success);

    // Verify station offsets for this section are cleaned up
    let proj = state.get_project_state().unwrap();
    let remaining_station_offsets: Vec<_> = proj
        .station_offsets
        .keys()
        .filter(|sr| sr.section_id == section_id)
        .collect();
    assert!(
        remaining_station_offsets.is_empty(),
        "Station offsets referencing deleted section should be removed"
    );
}

#[test]
fn test_delete_last_shot_cleans_up_closures() {
    let state = state_with_project("Closure Cleanup Test");

    // Create two sections
    let create_a = state.execute(Command::CreateSection {
        name: Some("ToDelete".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let section_a_id = create_a.section_id.unwrap();

    let create_b = state.execute(Command::CreateSection {
        name: Some("Other".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0), shot(3.0)],
    });
    let section_b_id = create_b.section_id.unwrap();

    // Manually insert a loop closure referencing section A
    {
        let mut proj_guard = state.project.lock();
        let project = proj_guard.as_mut().unwrap();
        project.loop_closures.push(LoopClosure {
            id: project.next_closure_id,
            station_a: StationRef::new(section_a_id, 0).into(),
            station_b: StationRef::new(section_b_id, 1).into(),
            path_length: 10.0,
            misclosure_distance: 0.5,
            error_percentage: 5.0,
            horizontal_misclosure: 0.4,
            vertical_misclosure: 0.3,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
        project.next_closure_id += 1;
    }

    // Verify closure exists
    {
        let proj = state.get_project_state().unwrap();
        assert_eq!(
            proj.loop_closures.len(),
            1,
            "Should have 1 closure before delete"
        );
    }

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_a_id).unwrap().shots[0].id
    };

    let result = state.execute_delete_last_shot_and_section(section_a_id, shot_id);
    assert!(result.success);

    // Verify closure is cleaned up
    let proj = state.get_project_state().unwrap();
    let remaining_closures: Vec<_> = proj
        .loop_closures
        .iter()
        .filter(|lc| {
            lc.station_a.section_id == section_a_id || lc.station_b.section_id == section_a_id
        })
        .collect();
    assert!(
        remaining_closures.is_empty(),
        "Closures referencing deleted section should be removed"
    );
}

#[test]
fn test_delete_last_shot_undo_restores_references() {
    let state = state_with_project("Undo Refs Test");

    // Create two sections
    let create_a = state.execute(Command::CreateSection {
        name: Some("ToDelete".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let section_a_id = create_a.section_id.unwrap();

    let create_b = state.execute(Command::CreateSection {
        name: Some("Other".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let section_b_id = create_b.section_id.unwrap();

    // Merge station 1 of A with station 0 of B
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(section_a_id, 1),
        station_b: StationRef::new(section_b_id, 0),
    });
    assert!(merge_result.success);

    // Manually add a section offset and station offset for section A
    {
        let mut proj_guard = state.project.lock();
        let project = proj_guard.as_mut().unwrap();
        // Note: merge already sets section_offsets, so add station offset
        project
            .station_offsets
            .insert(StationRef::new(section_a_id, 0).into(), (1.0, 2.0, 3.0));
        // Add a closure referencing section A
        project.loop_closures.push(LoopClosure {
            id: project.next_closure_id,
            station_a: StationRef::new(section_a_id, 0).into(),
            station_b: StationRef::new(section_b_id, 1).into(),
            path_length: 10.0,
            misclosure_distance: 0.5,
            error_percentage: 5.0,
            horizontal_misclosure: 0.4,
            vertical_misclosure: 0.3,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
        project.next_closure_id += 1;
    }

    // Capture counts before deletion
    let (merges_before, closures_before, station_offsets_a_before) = {
        let proj = state.get_project_state().unwrap();
        let merges = proj.station_merges.len();
        let closures = proj.loop_closures.len();
        let so = proj
            .station_offsets
            .keys()
            .filter(|sr| sr.section_id == section_a_id)
            .count();
        (merges, closures, so)
    };
    assert!(merges_before > 0);
    assert!(closures_before > 0);
    assert!(station_offsets_a_before > 0);

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_a_id).unwrap().shots[0].id
    };

    // Delete section A
    let result = state.execute_delete_last_shot_and_section(section_a_id, shot_id);
    assert!(result.success);

    // Verify everything cleaned up
    {
        let proj = state.get_project_state().unwrap();
        assert!(proj.get_section(section_a_id).is_none());
        let a_merges: Vec<_> = proj
            .station_merges
            .iter()
            .filter(|m| {
                m.station_a.section_id == section_a_id || m.station_b.section_id == section_a_id
            })
            .collect();
        assert!(a_merges.is_empty());
        let a_closures: Vec<_> = proj
            .loop_closures
            .iter()
            .filter(|lc| {
                lc.station_a.section_id == section_a_id || lc.station_b.section_id == section_a_id
            })
            .collect();
        assert!(a_closures.is_empty());
    }

    // Undo
    let undo_result = state.undo();
    assert!(
        undo_result.success,
        "Undo should succeed: {:?}",
        undo_result.error
    );

    // Verify all references restored
    let proj = state.get_project_state().unwrap();
    assert!(
        proj.get_section(section_a_id).is_some(),
        "Section should be restored"
    );
    assert_eq!(
        proj.station_merges.len(),
        merges_before,
        "Merges should be restored"
    );
    assert_eq!(
        proj.loop_closures.len(),
        closures_before,
        "Closures should be restored"
    );
    let so_after = proj
        .station_offsets
        .keys()
        .filter(|sr| sr.section_id == section_a_id)
        .count();
    assert_eq!(
        so_after, station_offsets_a_before,
        "Station offsets should be restored"
    );
}

// === Section deletion cleanup undo/redo tests (fn-47-ok1.7) ===

#[test]
fn test_delete_section_undo_restores_relationships() {
    // Delete a section with merges, closures, offsets, zero datums.
    // Undo should restore all of them.
    let state = state_with_project("Cleanup Undo Test");

    // Create two sections
    let sec_a_result = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let sec_a = sec_a_result.section_id.unwrap();

    let sec_b_result = state.execute(Command::CreateSection {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(3.0)],
    });
    let sec_b = sec_b_result.section_id.unwrap();

    // Merge sec_a:1 -> sec_b:0
    let merge_result = state.execute(Command::MergeStations {
        station_a: StationRef::new(sec_a, 1),
        station_b: StationRef::new(sec_b, 0),
    });
    assert!(merge_result.success, "Merge should succeed");

    // Set a zero datum on sec_a:0 so cleanup removes it
    let datum_result = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_a, 0),
        coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });
    assert!(datum_result.success, "SetZeroDatum should succeed");

    // Directly insert a station offset for sec_a:0 (MoveStation now modifies shot data directly)
    {
        let mut proj = state.get_project_state().unwrap();
        proj.station_offsets
            .insert(StationRef::new(sec_a, 0).into(), (1.0, 2.0, 0.0));
        state.set_project_for_tests(proj);
    }

    // Capture state before deletion
    let proj_before = state.get_project_state().unwrap();
    let merges_before = proj_before.station_merges.len();
    let zero_datums_before = proj_before.zero_datums.len();
    let station_offsets_a_before = proj_before
        .station_offsets
        .keys()
        .filter(|sr| sr.section_id == sec_a)
        .count();
    assert!(merges_before >= 1, "Should have at least 1 merge");
    assert!(zero_datums_before >= 1, "Should have at least 1 zero datum");
    assert!(
        station_offsets_a_before >= 1,
        "Should have at least 1 station offset for sec_a"
    );

    // Delete section A (triggers cleanup of merges, offsets, zero datums)
    let del_result = state.execute(Command::DeleteSection { section_id: sec_a });
    assert!(del_result.success, "Delete should succeed");

    // Verify cleanup happened: merges, zero datums, station offsets
    let proj_after = state.get_project_state().unwrap();
    assert!(
        proj_after.get_section(sec_a).is_none(),
        "Section A should be gone"
    );
    assert_eq!(
        proj_after
            .station_merges
            .iter()
            .filter(|m| m.station_a.section_id == sec_a || m.station_b.section_id == sec_a)
            .count(),
        0,
        "No merges should reference deleted section A"
    );
    assert_eq!(
        proj_after
            .zero_datums
            .iter()
            .filter(|zd| zd.station_ref.section_id == sec_a)
            .count(),
        0,
        "No zero datums should reference deleted section A"
    );
    assert_eq!(
        proj_after
            .station_offsets
            .keys()
            .filter(|sr| sr.section_id == sec_a)
            .count(),
        0,
        "No station offsets should reference deleted section A"
    );

    // Undo should restore everything
    let undo_result = state.undo();
    assert!(undo_result.success, "Undo should succeed");

    let proj_restored = state.get_project_state().unwrap();
    assert!(
        proj_restored.get_section(sec_a).is_some(),
        "Section A should be restored"
    );
    assert_eq!(
        proj_restored.station_merges.len(),
        merges_before,
        "Merges should be fully restored"
    );
    assert_eq!(
        proj_restored.zero_datums.len(),
        zero_datums_before,
        "Zero datums should be fully restored"
    );
    assert_eq!(
        proj_restored
            .station_offsets
            .keys()
            .filter(|sr| sr.section_id == sec_a)
            .count(),
        station_offsets_a_before,
        "Station offsets should be fully restored"
    );

    // Redo should re-delete and re-cleanup
    let redo_result = state.redo();
    assert!(redo_result.success, "Redo should succeed");

    let proj_redone = state.get_project_state().unwrap();
    assert!(
        proj_redone.get_section(sec_a).is_none(),
        "Section A should be gone after redo"
    );
    assert_eq!(
        proj_redone
            .station_merges
            .iter()
            .filter(|m| m.station_a.section_id == sec_a || m.station_b.section_id == sec_a)
            .count(),
        0,
        "No merges should reference deleted section after redo"
    );
    assert_eq!(
        proj_redone
            .zero_datums
            .iter()
            .filter(|zd| zd.station_ref.section_id == sec_a)
            .count(),
        0,
        "No zero datums should reference deleted section after redo"
    );
    assert_eq!(
        proj_redone
            .station_offsets
            .keys()
            .filter(|sr| sr.section_id == sec_a)
            .count(),
        0,
        "No station offsets should reference deleted section after redo"
    );
}
