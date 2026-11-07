//! Undo/redo (fn-35-f4v.1) and entry preservation on failure (fn-28-rpu.2).
//!
//! Lines 8867-9931 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// === Undo/Redo Tests for fn-35-f4v.1 ===

#[test]
fn test_undo_delete_shot() {
    // Verify undoing DeleteShot restores the shot
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(10.0)],
    });

    let section_id = create_result.section_id.unwrap();
    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    // Delete the shot
    state.execute(Command::DeleteShot {
        section_id,
        shot_id,
    });

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 1);
    assert_eq!(project.sections[0].shots[0].length, 10.0);

    // Undo the delete
    let result = state.undo();
    assert!(result.success);

    // Shot should be restored
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].shots.len(), 2);
    assert!(project.sections[0].shots.iter().any(|s| s.length == 5.0));
}

#[test]
fn test_undo_merge_stations() {
    // Verify undoing MergeStations removes the merge
    let state = state_with_project("Test");

    // Create two sections
    let result_a = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0)],
    });
    let section_a_id = result_a.section_id.unwrap();

    let result_b = state.execute(Command::CreateSection {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0), shot(6.0)],
    });
    let section_b_id = result_b.section_id.unwrap();

    // Verify no merges exist before
    let project_before = state.get_project_state().unwrap();
    assert!(project_before.station_merges.is_empty());

    // Merge stations
    let station_a = StationRef::new(section_a_id, 1);
    let station_b = StationRef::new(section_b_id, 0);

    let result = state.execute(Command::MergeStations {
        station_a,
        station_b,
    });
    assert!(result.success);

    // Verify merge was created
    let project_after = state.get_project_state().unwrap();
    assert_eq!(project_after.station_merges.len(), 1);
    assert!(project_after.key_is_ref(&project_after.station_merges[0].station_a, station_a));
    assert!(project_after.key_is_ref(&project_after.station_merges[0].station_b, station_b));

    // Undo the merge
    let result = state.undo();
    assert!(result.success);

    // Verify merge is gone
    let project_undone = state.get_project_state().unwrap();
    assert!(project_undone.station_merges.is_empty());
}

#[test]
fn test_multiple_undo_sequence() {
    // Verify multiple undos in LIFO order
    let state = state_with_project("Test");

    // Create sections in sequence
    state.execute(Command::CreateSection {
        name: Some("First".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    state.execute(Command::CreateSection {
        name: Some("Second".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![],
    });

    state.execute(Command::CreateSection {
        name: Some("Third".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![],
    });

    assert_eq!(state.get_project_state().unwrap().sections.len(), 3);

    // Undo should remove Third first (LIFO)
    let result = state.undo();
    assert!(result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 2);
    assert!(!project
        .sections
        .iter()
        .any(|s| s.name == Some("Third".to_string())));

    // Undo should remove Second
    let result = state.undo();
    assert!(result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert!(!project
        .sections
        .iter()
        .any(|s| s.name == Some("Second".to_string())));

    // Undo should remove First
    let result = state.undo();
    assert!(result.success);
    assert!(state.get_project_state().unwrap().sections.is_empty());
}

#[test]
fn test_redo_clears_on_new_command() {
    // Verify new command clears redo stack
    let state = state_with_project("Test");

    // Create a section
    state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    // Undo it
    state.undo();
    assert!(state.can_redo());

    // Execute a new command (should clear redo stack)
    state.execute(Command::CreateSection {
        name: Some("New".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![],
    });

    // Redo should no longer be available
    assert!(!state.can_redo());

    // Verify we have the new section, not the original
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(project.sections[0].name, Some("New".to_string()));
}

#[test]
fn test_undo_stack_limits() {
    // Verify stack limits and overflow handling
    // Use a small limit to test overflow behavior
    let state = AppState::new().unwrap();

    // Set a small undo history limit via config
    state.config.lock().general.undo_history_limit = 3;

    // Reinitialize undo stack with new limit
    state.undo_stack.lock().set_history_limit(3);

    state.execute(Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    });

    // Create 5 sections (more than the limit of 3)
    for i in 0..5 {
        state.execute(Command::CreateSection {
            name: Some(format!("Section {i}")),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![],
        });
    }

    assert_eq!(state.get_project_state().unwrap().sections.len(), 5);

    // We should only be able to undo 3 times (the limit)
    // The first 2 section creates should have been dropped from the stack
    let result1 = state.undo();
    assert!(result1.success);
    assert_eq!(state.get_project_state().unwrap().sections.len(), 4);

    let result2 = state.undo();
    assert!(result2.success);
    assert_eq!(state.get_project_state().unwrap().sections.len(), 3);

    let result3 = state.undo();
    assert!(result3.success);
    assert_eq!(state.get_project_state().unwrap().sections.len(), 2);

    // Fourth undo should fail (stack exhausted)
    let result4 = state.undo();
    assert!(!result4.success);

    // Verify we still have the first 2 sections (they couldn't be undone)
    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 2);
    assert!(project
        .sections
        .iter()
        .any(|s| s.name == Some("Section 0".to_string())));
    assert!(project
        .sections
        .iter()
        .any(|s| s.name == Some("Section 1".to_string())));
}

// === Undo/Redo Entry Preservation on Failure Tests (fn-28-rpu.2) ===

#[test]
fn test_undo_failure_preserves_entry_on_undo_stack() {
    // When undo() fails (e.g., invalid JSON in PreviousValues), the entry
    // must be pushed back to the top of the undo stack, not lost.
    let state = AppState::new().unwrap();

    // Push an entry with invalid undo data that will fail during undo
    let mut stack = state.undo_stack.lock();
    stack.push_with_data(
        Command::UpdateShot {
            section_id: 1,
            shot_id: 1,
            updates: ShotUpdates::default(),
        },
        UndoData::PreviousValues {
            section_id: 1,
            shot_id: Some(1),
            shot_uuid: None,
            values_json: "not-valid-json".to_string(),
            previous_offsets: HashMap::new(),
        },
    );
    assert_eq!(stack.len(), 1);
    assert!(stack.can_undo());
    assert!(!stack.can_redo());
    drop(stack);

    // Attempt undo - should fail because of invalid JSON
    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to parse undo data"));

    // Entry must still be on the undo stack (preserved, not lost)
    let stack = state.undo_stack.lock();
    assert_eq!(
        stack.len(),
        1,
        "Entry must remain on undo stack after failure"
    );
    assert!(
        stack.can_undo(),
        "can_undo must be true - entry was preserved"
    );
    // Entry must NOT have moved to redo stack
    assert!(!stack.can_redo(), "Entry must not appear on redo stack");
}

#[test]
fn test_redo_failure_preserves_entry_on_redo_stack() {
    // When redo() fails, the entry must be pushed back to the redo stack.
    let state = AppState::new().unwrap();

    // Push an entry onto the redo stack that will fail when re-executed
    let mut stack = state.undo_stack.lock();
    let entry = UndoEntry {
        replayed_at: None,
        command_bytes: None,
        command: Command::CloseProject,
        undo_data: UndoData::ProjectSnapshotPair {
            before: "{}".to_string(),
            after: "not-valid-json".to_string(),
        },
        timestamp: None,
    };
    stack.push_redo(entry);
    assert_eq!(stack.redo_len(), 1);
    assert!(!stack.can_undo());
    drop(stack);

    // Attempt redo - should fail because "after" is invalid JSON
    let result = state.redo();
    assert!(!result.success);
    assert_eq!(
        result.error.as_deref(),
        Some("Failed to parse project snapshot")
    );

    // Entry must still be on the redo stack (preserved, not lost)
    let stack = state.undo_stack.lock();
    assert_eq!(
        stack.redo_len(),
        1,
        "Entry must remain on redo stack after failure"
    );
    assert!(
        stack.can_redo(),
        "can_redo must be true - entry was preserved"
    );
    // Entry must NOT have moved to undo stack
    assert!(!stack.can_undo(), "Entry must not appear on undo stack");
}

#[test]
fn test_undo_failure_preserves_entry_at_top_lifo_order() {
    // After a failed undo, the entry must be at the TOP of the undo stack.
    // Push two entries: a valid one at the bottom and an invalid one at the top.
    // After the failed undo, the invalid entry must still be on top (LIFO preserved).
    let state = state_with_project("Test");

    // Push a valid section create (will be at bottom of undo stack)
    state.execute(Command::CreateSection {
        name: Some("Valid Section".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    // Push an invalid entry on top (will fail on undo)
    {
        let mut stack = state.undo_stack.lock();
        stack.push_with_data(
            Command::UpdateShot {
                section_id: 999,
                shot_id: 999,
                updates: ShotUpdates::default(),
            },
            UndoData::PreviousValues {
                section_id: 999,
                shot_id: Some(999),
                shot_uuid: None,
                values_json: "not-json".to_string(),
                previous_offsets: HashMap::new(),
            },
        );
        // undo stack: [CreateSection, InvalidUpdateShot] (top = InvalidUpdateShot)
        assert_eq!(stack.len(), 2);
    }

    // Attempt undo - the top entry (invalid) should fail
    let result = state.undo();
    assert!(!result.success);

    // The failed entry must be pushed back to the TOP of the undo stack
    let stack = state.undo_stack.lock();
    assert_eq!(stack.len(), 2, "Both entries must still be on the stack");

    // Peek at the top: it should be the invalid UpdateShot (LIFO preserved)
    let top_cmd = stack.peek_undo().unwrap();
    assert!(
        matches!(
            top_cmd,
            Command::UpdateShot {
                section_id: 999,
                ..
            }
        ),
        "Failed entry must be at TOP of undo stack (LIFO order preserved)"
    );
}

#[test]
fn test_redo_failure_preserves_entry_at_top_lifo_order() {
    // After a failed redo, the entry must be at the TOP of the redo stack.
    let state = AppState::new().unwrap();

    // Push two entries to redo stack: valid at bottom, invalid at top
    let mut stack = state.undo_stack.lock();

    // Valid redo entry (bottom)
    stack.push_redo(UndoEntry {
        replayed_at: None,
        command_bytes: None,
        command: Command::CreateSection {
            name: Some("Valid".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![],
        },
        undo_data: UndoData::None,
        timestamp: None,
    });

    // Invalid redo entry (top) - will fail because "after" JSON is invalid
    stack.push_redo(UndoEntry {
        replayed_at: None,
        command_bytes: None,
        command: Command::CloseProject,
        undo_data: UndoData::ProjectSnapshotPair {
            before: "{}".to_string(),
            after: "invalid-json".to_string(),
        },
        timestamp: None,
    });

    assert_eq!(stack.redo_len(), 2);
    drop(stack);

    // Attempt redo - the top entry (invalid) should fail
    let result = state.redo();
    assert!(!result.success);

    // The failed entry must be pushed back to the TOP of the redo stack
    let stack = state.undo_stack.lock();
    assert_eq!(
        stack.redo_len(),
        2,
        "Both entries must still be on redo stack"
    );

    // Peek at the top: it should be the invalid ProjectSnapshotPair
    let top_cmd = stack.peek_redo().unwrap();
    assert!(
        matches!(top_cmd, Command::CloseProject),
        "Failed entry must be at TOP of redo stack (LIFO order preserved)"
    );
}

#[test]
fn test_undo_to_partial_completion_stops_at_failed_step() {
    // undo_to() calls undo() in a loop. If an intermediate step fails,
    // it should stop and return an enhanced error message with step count.
    let state = state_with_project("Test");

    // Create 3 valid sections (3 undo entries)
    for i in 0..3 {
        state.execute(Command::CreateSection {
            name: Some(format!("Section {i}")),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![],
        });
    }

    // Push an invalid entry that will fail when undone
    // This goes on top of the stack, so it's the FIRST to be undone
    // But we want it to fail at step 2 or 3, so we need to place it lower.
    // Strategy: undo the top entry to make room, then insert the invalid entry,
    // then redo to restore the valid one on top.
    //
    // Actually, let's structure it differently:
    // Stack bottom-to-top: [Section0, InvalidEntry, Section2]
    // undo_to(2) will try 3 undos: Section2 (ok), InvalidEntry (fail at step 2)
    {
        // Remove the top two entries, insert invalid, put one back
        let mut stack = state.undo_stack.lock();
        let top = stack.pop_undo().unwrap(); // Section 2
        let _middle = stack.pop_undo(); // Section 1

        // Push invalid entry at position where Section 1 was
        stack.push_undo(UndoEntry {
            replayed_at: None,
            command_bytes: None,
            command: Command::UpdateShot {
                section_id: 999,
                shot_id: 999,
                updates: ShotUpdates::default(),
            },
            undo_data: UndoData::PreviousValues {
                section_id: 999,
                shot_id: Some(999),
                shot_uuid: None,
                values_json: "invalid-json".to_string(),
                previous_offsets: HashMap::new(),
            },
            timestamp: None,
        });

        // Push Section 2 back on top
        stack.push_undo(top);
    }

    // Undo stack is now: [Section0, InvalidEntry, Section2]
    // undo_to(2) means execute 3 undos:
    //   Step 1: undo Section2 -> success
    //   Step 2: undo InvalidEntry -> failure
    // Should stop and return enhanced message format
    let result = state.undo_to(2);
    assert!(!result.success);

    let error_msg = result.error.as_deref().unwrap();
    // Verify enhanced message format: "Undo failed at step {current} of {total}: {reason}"
    assert!(
        error_msg.contains("Undo failed at step 2 of 3"),
        "Expected enhanced message with step count, got: {error_msg}"
    );
    assert!(
        error_msg.contains("Failed to parse undo data"),
        "Expected original failure reason in message, got: {error_msg}"
    );

    // Verify partial completion: Section 2 was undone (step 1 succeeded),
    // but Section 0 was not undone (step 2 failed before reaching it).
    // The invalid entry should still be on the undo stack (preserved).
    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections.len(),
        2,
        "Only Section 2 should have been undone (step 1); Section 0 remains"
    );
    assert!(project
        .sections
        .iter()
        .any(|s| s.name == Some("Section 0".to_string())));

    // Verify the failed entry was preserved on the undo stack
    let stack = state.undo_stack.lock();
    assert_eq!(
        stack.len(),
        2,
        "Section 0 and invalid entry must remain on undo stack"
    );
    let top_cmd = stack.peek_undo().unwrap();
    assert!(
        matches!(
            top_cmd,
            Command::UpdateShot {
                section_id: 999,
                ..
            }
        ),
        "Failed invalid entry must be at top of undo stack after partial undo_to"
    );
}

#[test]
fn test_redo_to_partial_completion_stops_at_failed_step() {
    // redo_to() calls redo() in a loop. If an intermediate step fails,
    // it should stop and return an enhanced error message with step count.
    let state = state_with_project("Test");

    // Create 2 sections, then undo both so they're on the redo stack
    state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });
    state.execute(Command::CreateSection {
        name: Some("Section B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![],
    });

    // Undo both: redo stack bottom-to-top = [SectionB, SectionA]
    // (B undone first, then A; LIFO means A is on top for redo)
    state.undo();
    state.undo();
    assert!(state.get_project_state().unwrap().sections.is_empty());

    // Now manipulate the redo stack: insert an invalid entry between the two valid ones
    // Redo stack: [SectionA, InvalidEntry, SectionB]
    {
        let mut stack = state.undo_stack.lock();
        let top = stack.pop_redo().unwrap(); // SectionA (last undone = first to redo)
                                             // Push invalid entry
        stack.push_redo(UndoEntry {
            replayed_at: None,
            command_bytes: None,
            command: Command::CloseProject,
            undo_data: UndoData::ProjectSnapshotPair {
                before: "{}".to_string(),
                after: "not-json".to_string(),
            },
            timestamp: None,
        });
        // Put SectionA back on top
        stack.push_redo(top);
        // Redo stack bottom-to-top: [SectionB, InvalidEntry, SectionA]
        assert_eq!(stack.redo_len(), 3);
    }

    // redo_to(2) means execute 3 redos:
    //   Step 1: redo SectionA (top) -> success
    //   Step 2: redo InvalidEntry -> failure
    // Should stop and return enhanced message
    let result = state.redo_to(2);
    assert!(!result.success);

    let error_msg = result.error.as_deref().unwrap();
    assert!(
        error_msg.contains("Redo failed at step 2 of 3"),
        "Expected enhanced message with step count, got: {error_msg}"
    );
    assert!(
        error_msg.contains("Failed to parse project snapshot"),
        "Expected original failure reason in message, got: {error_msg}"
    );

    // Verify partial completion: SectionA was re-created (step 1 succeeded)
    let project = state.get_project_state().unwrap();
    assert_eq!(
        project.sections.len(),
        1,
        "Only SectionA should have been redone (step 1)"
    );
    assert_eq!(project.sections[0].name, Some("Section A".to_string()));

    // Verify the failed entry was preserved on the redo stack
    let stack = state.undo_stack.lock();
    assert_eq!(
        stack.redo_len(),
        2,
        "Invalid entry and SectionB must remain on redo stack"
    );
    let top_cmd = stack.peek_redo().unwrap();
    assert!(
        matches!(top_cmd, Command::CloseProject),
        "Failed invalid entry must be at top of redo stack after partial redo_to"
    );
}

#[test]
fn test_undo_failure_with_project_snapshot_preserves_entry() {
    // Verify that a ProjectSnapshot undo failure (invalid JSON) preserves the entry
    let state = AppState::new().unwrap();

    let mut stack = state.undo_stack.lock();
    stack.push_with_data(
        Command::Batch {
            commands: Vec::new(),
        },
        UndoData::ProjectSnapshot("not-valid-json".to_string()),
    );
    assert_eq!(stack.len(), 1);
    drop(stack);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(
        result.error.as_deref(),
        Some("Failed to parse project snapshot")
    );

    // Entry must still be on the undo stack
    let stack = state.undo_stack.lock();
    assert_eq!(stack.len(), 1, "Entry must be preserved on undo stack");
    assert!(!stack.can_redo(), "Entry must not appear on redo stack");
}

#[test]
fn test_undo_failure_with_project_lock_preserves_entry() {
    // When the project mutex is poisoned, undo fails but the entry
    // must still be pushed back to the undo stack.
    let state = AppState::new().unwrap();

    let snapshot = serde_json::to_string(&Project::new("Test".to_string(), None)).unwrap();
    let mut stack = state.undo_stack.lock();
    stack.push_with_data(
        Command::Batch {
            commands: Vec::new(),
        },
        UndoData::ProjectSnapshot(snapshot),
    );
    assert_eq!(stack.len(), 1);
    drop(stack);

    // Poison the project mutex
    poison_mutex(&state.project);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error, Some(refused_after_crash("Undo")));

    // Entry must still be preserved on the undo stack
    let stack = state.undo_stack.lock();
    assert_eq!(
        stack.len(),
        1,
        "Entry must be preserved on undo stack after project lock failure"
    );
    assert!(!stack.can_redo(), "Entry must not appear on redo stack");
}

#[test]
fn test_redo_failure_with_project_lock_preserves_entry() {
    // When the project mutex is poisoned, redo fails but the entry
    // must still be pushed back to the redo stack.
    let state = AppState::new().unwrap();

    let before = serde_json::to_string(&Project::new("Before".to_string(), None)).unwrap();
    let after = serde_json::to_string(&Project::new("After".to_string(), None)).unwrap();

    let entry = UndoEntry {
        replayed_at: None,
        command_bytes: None,
        command: Command::CloseProject,
        undo_data: UndoData::ProjectSnapshotPair { before, after },
        timestamp: None,
    };

    let mut stack = state.undo_stack.lock();
    stack.push_redo(entry);
    assert_eq!(stack.redo_len(), 1);
    drop(stack);

    // Poison the project mutex
    poison_mutex(&state.project);

    let result = state.redo();
    assert!(!result.success);
    assert_eq!(result.error, Some(refused_after_crash("Redo")));

    // Entry must still be preserved on the redo stack
    let stack = state.undo_stack.lock();
    assert_eq!(
        stack.redo_len(),
        1,
        "Entry must be preserved on redo stack after project lock failure"
    );
    assert!(!stack.can_undo(), "Entry must not appear on undo stack");
}

#[test]
fn test_set_zero_datum_update_captures_previous_gps_and_offsets() {
    // Verify that calling SetZeroDatum on an existing datum captures previous_gps = Some(old)
    // and previous_offsets correctly in the undo data.
    let state = state_with_project("Test");

    // Create a section with shots
    let result_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(10.0)],
    });
    let section_a = result_a.section_id.unwrap();

    let station_ref = StationRef::new(section_a, 0);

    // Set initial zero datum with GPS-A
    let gps_a = GpsCoordinates::new(25.0, -80.0, 50.0).unwrap();
    let result = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps_a,
    });
    assert!(result.success, "Initial SetZeroDatum should succeed");

    // Capture the section offsets before the update for later assertion
    let offsets_before_update = {
        let proj = state.project.lock();
        let project = proj.as_ref().unwrap();
        project.section_offsets().clone()
    };

    // Clear undo stack to focus on the update
    state.undo_stack.lock().clear();

    // Now update the datum with GPS-B
    let gps_b = GpsCoordinates::new(26.0, -81.0, 55.0).unwrap();
    let result = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps_b,
    });
    assert!(result.success, "Update SetZeroDatum should succeed");

    // Inspect the undo entry's undo_data
    let stack = state.undo_stack.lock();
    assert!(stack.can_undo(), "Should have an undo entry");

    let undo_data = stack.peek_undo_data().expect("Should have undo data");
    match undo_data {
        UndoData::ZeroDatumInfo {
            station_ref: ref_in_entry,
            previous_gps,
            previous_offsets,
            ..
        } => {
            assert_eq!(*ref_in_entry, station_ref, "Station ref should match");
            assert!(
                previous_gps.is_some(),
                "previous_gps must be Some(old) when updating existing datum"
            );
            let prev = previous_gps.unwrap();
            assert!(
                (prev.latitude - 25.0).abs() < f64::EPSILON,
                "previous_gps latitude should be GPS-A's latitude (25.0), got {}",
                prev.latitude
            );
            assert!(
                (prev.longitude - (-80.0)).abs() < f64::EPSILON,
                "previous_gps longitude should be GPS-A's longitude (-80.0), got {}",
                prev.longitude
            );
            // Assert previous_offsets contains the datum section and matches the pre-update value
            assert!(
                previous_offsets.contains_key(&section_a),
                "previous_offsets must contain the datum section ({section_a})"
            );
            let captured_offset = previous_offsets.get(&section_a).unwrap();
            let expected_offset = offsets_before_update.get(&section_a).copied();
            assert_eq!(
                *captured_offset, expected_offset,
                "previous_offsets for datum section should match pre-update offset"
            );
        }
        other => panic!(
            "Expected UndoData::ZeroDatumInfo, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

#[test]
fn test_set_zero_datum_update_undo_restores_previous_gps_and_offsets() {
    // After updating a zero datum, undo should restore the previous GPS and section offsets.
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

    let station_ref = StationRef::new(section_a, 0);

    // Set initial zero datum with GPS-A
    let gps_a = GpsCoordinates::new(25.0, -80.0, 50.0).unwrap();
    state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps_a,
    });

    // Capture state after initial set (GPS-A applied)
    let (offsets_after_a, gps_after_a) = {
        let proj = state.project.lock();
        let project = proj.as_ref().unwrap();
        let offsets = project.section_offsets().clone();
        let gps = project
            .zero_datums
            .iter()
            .find(|zd| project.key_is_ref(&zd.station_ref, station_ref))
            .map(|zd| zd.gps);
        (offsets, gps)
    };
    assert!(gps_after_a.is_some(), "GPS-A should be set");
    assert!(
        (gps_after_a.unwrap().latitude - 25.0).abs() < f64::EPSILON,
        "GPS should be A"
    );

    // Clear undo stack so we only track the update
    state.undo_stack.lock().clear();

    // Update datum with GPS-B
    let gps_b = GpsCoordinates::new(26.0, -81.0, 55.0).unwrap();
    let result = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps_b,
    });
    assert!(result.success);

    // Verify GPS-B is now active
    {
        let proj = state.project.lock();
        let project = proj.as_ref().unwrap();
        let current_gps = project
            .zero_datums
            .iter()
            .find(|zd| project.key_is_ref(&zd.station_ref, station_ref))
            .map(|zd| zd.gps)
            .unwrap();
        assert!(
            (current_gps.latitude - 26.0).abs() < f64::EPSILON,
            "GPS should now be B"
        );
    }

    // Undo the update
    let undo_result = state.undo();
    assert!(undo_result.success, "Undo should succeed");

    // Verify GPS is restored to A
    let (restored_offsets, restored_gps) = {
        let proj = state.project.lock();
        let project = proj.as_ref().unwrap();
        let offsets = project.section_offsets().clone();
        let gps = project
            .zero_datums
            .iter()
            .find(|zd| project.key_is_ref(&zd.station_ref, station_ref))
            .map(|zd| zd.gps);
        (offsets, gps)
    };

    let restored = restored_gps.expect("GPS should be restored after undo");
    assert!(
        (restored.latitude - 25.0).abs() < f64::EPSILON,
        "GPS latitude should be restored to GPS-A (25.0), got {}",
        restored.latitude
    );
    assert!(
        (restored.longitude - (-80.0)).abs() < f64::EPSILON,
        "GPS longitude should be restored to GPS-A (-80.0), got {}",
        restored.longitude
    );

    // Verify section offsets are restored
    assert_eq!(
        restored_offsets, offsets_after_a,
        "Section offsets should be restored to pre-update values"
    );
}

#[test]
fn test_set_zero_datum_no_change_still_creates_undo_entry() {
    // The no-change guard is frontend-only (lat/lon comparison in EditStationDialog).
    // The backend always executes SetZeroDatum and always creates an undo entry.
    // This test confirms that calling SetZeroDatum with the same GPS coordinates
    // still produces an undo entry (the guard must be tested in E2E).
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

    let station_ref = StationRef::new(section_a, 0);
    let gps = GpsCoordinates::new(25.0, -80.0, 50.0).unwrap();

    // Set initial datum
    state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: gps,
    });

    // Clear undo stack
    state.undo_stack.lock().clear();

    // Call SetZeroDatum again with identical GPS
    let same_gps = GpsCoordinates::new(25.0, -80.0, 50.0).unwrap();
    let result = state.execute(Command::SetZeroDatum {
        station_ref,
        coordinates: same_gps,
    });
    assert!(result.success, "SetZeroDatum with same GPS should succeed");

    // Backend still creates an undo entry (no backend-level no-change guard)
    let stack = state.undo_stack.lock();
    assert!(
        stack.can_undo(),
        "Backend should still create an undo entry even with identical GPS; \
         the no-change guard is frontend-only"
    );
}
