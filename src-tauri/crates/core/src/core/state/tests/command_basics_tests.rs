//! Batch, station merge, zero datum, move section, validation, multi-step undo.
//!
//! Lines 7264-7762 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// === Batch Command Tests ===

#[test]
fn test_batch_create_sections() {
    let state = state_with_project("Test");

    let result = state.execute(Command::Batch {
        commands: vec![
            Command::CreateSection {
                name: Some("First".to_string()),
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots: vec![shot(5.0)],
            },
            Command::CreateSection {
                name: Some("Second".to_string()),
                surveyor: None,
                date: None,
                color: "#00ff00".to_string(),
                direction: None,
                shots: vec![shot(7.0)],
            },
        ],
    });

    assert!(result.success);
    assert_eq!(result.section_ids.len(), 2);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 2);
}

#[test]
fn test_batch_fails_on_first_error() {
    let state = state_with_project("Test");

    // First command succeeds, second fails (delete nonexistent section)
    let result = state.execute(Command::Batch {
        commands: vec![
            Command::CreateSection {
                name: Some("First".to_string()),
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots: vec![],
            },
            Command::DeleteSection { section_id: 999 },
        ],
    });

    assert!(!result.success);
}

// === Station Merge Tests ===

#[test]
fn test_merge_stations_same_section_fails() {
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0)],
    });

    let section_id = create_result.section_id.unwrap();

    // Try to merge stations from the same section
    let result = state.execute(Command::MergeStations {
        station_a: StationRef::new(section_id, 0),
        station_b: StationRef::new(section_id, 1),
    });

    assert!(!result.success);
    assert!(result
        .error
        .unwrap()
        .contains("Cannot merge stations from the same section"));
}

// === Zero Datum Tests ===

#[test]
fn test_set_and_remove_zero_datum() {
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
    let station_ref = StationRef::new(section_id, 0);

    // Set zero datum
    let result = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.zero_datums.len(), 1);
    assert_eq!(project.zero_datums[0].gps.latitude, 45.0);

    // Remove zero datum
    let result = state.execute(Command::RemoveZeroDatum { station_ref });
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert!(project.zero_datums.is_empty());
}

#[test]
fn test_remove_nonexistent_zero_datum() {
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

    // Try to remove a datum that was never set
    let result = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(section_id, 0),
    });

    assert!(!result.success);
}

// === Move Section Tests ===

#[test]
fn test_move_section() {
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

    // First move
    let result = state.execute(Command::MoveSection {
        section_id,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.section_offsets().get(&section_id),
        Some(&(10.0, 20.0)),
        "First move should set offset to (10, 20)"
    );

    // Second move should accumulate
    let result = state.execute(Command::MoveSection {
        section_id,
        offset: Coordinates::new(5.0, -10.0, 0.0),
    });
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.section_offsets().get(&section_id),
        Some(&(15.0, 10.0)),
        "Second move should accumulate to (15, 10)"
    );
}

#[test]
fn test_move_section_undo_redo() {
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

    // Move section
    state.execute(Command::MoveSection {
        section_id,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.section_offsets().get(&section_id),
        Some(&(10.0, 20.0))
    );

    // Undo should reverse the move
    let undo_result = state.undo();
    assert!(undo_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project
            .section_offsets()
            .get(&section_id)
            .copied()
            .unwrap_or((0.0, 0.0)),
        (0.0, 0.0),
        "Undo should restore offset to origin"
    );

    // Redo should re-apply
    let redo_result = state.redo();
    assert!(redo_result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.section_offsets().get(&section_id),
        Some(&(10.0, 20.0)),
        "Redo should re-apply the move"
    );
}

#[test]
fn move_section_moves_the_whole_merged_group_and_sticks() {
    // A merged section is rigidly bound to its partner: "Adjust Position"
    // (MoveSection) must move the WHOLE merge group together so the merge stays
    // aligned and the move isn't snapped back by the post-command reconcile - not
    // shove one member off the other (finding #6).
    let state = state_with_project("Merged");

    let mk = |name: &str, color: &str| Command::CreateSection {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: color.to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    };
    let a = state.execute(mk("A", "#ff0000")).section_id.unwrap();
    let b = state.execute(mk("B", "#00ff00")).section_id.unwrap();

    // Merge A:0 with B:0 → A and B are now one rigid group.
    let merge = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 0),
    });
    assert!(merge.success, "merge failed: {:?}", merge.error);
    assert!(
        state
            .get_project_state()
            .unwrap()
            .get_rigid_connected_section_ids(a)
            .contains(&b),
        "A and B must be one rigid group after the merge"
    );

    // Offsets right after the merge (the merge/reconcile may have set them to align).
    let off = |st: &AppState, id| {
        st.get_project_state()
            .unwrap()
            .section_offsets()
            .get(&id)
            .copied()
            .unwrap_or((0.0, 0.0))
    };
    let a_before = off(&state, a);
    let b_before = off(&state, b);

    // Move section A - the whole group must translate by the same offset and STICK.
    let r = state.execute(Command::MoveSection {
        section_id: a,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });
    assert!(r.success);

    assert_eq!(
        off(&state, a),
        (a_before.0 + 10.0, a_before.1 + 20.0),
        "the moved section translated and the move stuck (not reverted by reconcile)"
    );
    assert_eq!(
        off(&state, b),
        (b_before.0 + 10.0, b_before.1 + 20.0),
        "the merge partner translated by the same offset (moved as a rigid group)"
    );

    // Undo restores both; redo re-applies to both.
    assert!(state.undo().success);
    assert_eq!(off(&state, a), a_before, "undo restores the moved section");
    assert_eq!(
        off(&state, b),
        b_before,
        "undo restores the merge partner too"
    );

    assert!(state.redo().success);
    assert_eq!(off(&state, a), (a_before.0 + 10.0, a_before.1 + 20.0));
    assert_eq!(off(&state, b), (b_before.0 + 10.0, b_before.1 + 20.0));
}

#[test]
fn test_move_nonexistent_section() {
    let state = state_with_project("Test");

    let result = state.execute(Command::MoveSection {
        section_id: 999,
        offset: Coordinates::new(10.0, 20.0, 0.0),
    });

    assert!(!result.success);
}

// === Validation Tests ===

#[test]
fn test_create_section_rejects_zero_length_shot() {
    let state = state_with_project("Test");

    // Create a section with a zero-length shot (should be rejected)
    let result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 45.0,
            azimuth_out: 225.0,
            length: 0.0, // Zero length triggers warning
            depth_in: 10.0,
            depth_out: 10.0,
            ..Default::default()
        }],
    });

    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("zero length"));
}

// === Undo/Redo with Shot Restoration ===

#[test]
fn test_undo_delete_shot_restores_at_correct_position() {
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(9.0)],
    });

    let section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    let middle_shot_id = project.sections[0].shots[1].id;

    // Delete the middle shot
    state.execute(Command::DeleteShot {
        section_id,
        shot_id: middle_shot_id,
    });

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 2);

    // Undo the delete
    let result = state.undo();
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 3);
    // The shot should be restored at position 1
    assert_eq!(project.sections[0].shots[1].length, 7.0);
}

// === Multiple Undo/Redo Sequence ===

#[test]
fn test_multiple_undo_redo_sequence() {
    let state = state_with_project("Test");

    // Create 3 sections
    for i in 1..=3 {
        state.execute(Command::CreateSection {
            name: Some(format!("Section {i}")),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![],
        });
    }

    assert_eq!(state.get_project_state().unwrap().sections.len(), 3);

    // Undo all 3 creates
    state.undo();
    state.undo();
    state.undo();

    assert!(state.get_project_state().unwrap().sections.is_empty());

    // Redo 2 of them
    state.redo();
    state.redo();

    assert_eq!(state.get_project_state().unwrap().sections.len(), 2);
}

/// [#576 FZ-DET-2] `get_segment_geometry` lists the segment's sections in
/// project order. It walked a `HashSet` built per call, so the same project,
/// asked twice with nothing changed in between, could list its sections in
/// another order each time.
#[test]
fn segment_geometry_lists_sections_in_project_order() {
    let state = state_with_project("CaveA");
    let create = |index: usize| Command::CreateSection {
        name: Some(format!("Section{index}")),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(6.0)],
    };
    let ids: Vec<SectionId> = (0..8)
        .map(|index| state.execute(create(index)).section_id.unwrap())
        .collect();
    for pair in ids.windows(2) {
        let merged = state.execute(Command::MergeStations {
            station_a: StationRef::new(pair[0], 2),
            station_b: StationRef::new(pair[1], 0),
        });
        assert!(merged.success, "merge failed: {:?}", merged.error);
    }
    let listings: Vec<Vec<SectionId>> = (0..8)
        .map(|_| {
            state
                .get_segment_geometry(ids[0])
                .unwrap()
                .sections
                .iter()
                .map(|section| section.section_id)
                .collect()
        })
        .collect();
    let differing = listings.iter().filter(|listed| **listed != ids).count();
    assert_eq!(
        differing, 0,
        "{differing} of 8 calls listed the segment's sections out of project order: {listings:?}"
    );
}
