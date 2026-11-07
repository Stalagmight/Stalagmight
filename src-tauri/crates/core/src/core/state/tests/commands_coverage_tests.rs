//! Coverage gap tests: batch, delete_shot shift-blocked, auto-save, move_station.
//!
//! Lines 19294-20070 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// ============================================================================
// Coverage gap tests: commands.rs - batch edge cases
// ============================================================================

#[test]
fn test_batch_empty_is_noop() {
    let state = state_with_project("Test");
    let result = state.execute(Command::Batch { commands: vec![] });
    assert!(result.success);
}

#[test]
fn test_batch_disallowed_command_save_project() {
    let state = state_with_project("Test");
    let result = state.execute(Command::Batch {
        commands: vec![Command::SaveProject {
            path: Some(PathBuf::from("/tmp/test.stalx")),
        }],
    });
    assert!(!result.success);
    assert!(result
        .error
        .unwrap_or_default()
        .contains("not allowed in batch"));
}

#[test]
fn test_batch_partial_failure_rolls_back() {
    let state = state_with_project("Test");

    let sections_before = state.get_project_state().unwrap().sections.len();

    // First command succeeds (create section), second fails (merge nonexistent)
    let result = state.execute(Command::Batch {
        commands: vec![
            Command::CreateSection {
                name: Some("NewSec".to_string()),
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots: vec![shot(5.0)],
            },
            Command::MergeStations {
                station_a: StationRef::new(9999, 0),
                station_b: StationRef::new(9998, 0),
            },
        ],
    });
    assert!(!result.success);

    // Rollback should restore original state
    let sections_after = state.get_project_state().unwrap().sections.len();
    assert_eq!(
        sections_before, sections_after,
        "batch rollback should restore section count"
    );
}

/// Withdrawing an abandoned import (#394) removes N sections as ONE undo entry.
///
/// The DMP wizard imports eagerly - the zero-datum anchor lands on mount, each
/// reachable segment as it is placed - so "Cancel, and remove what was already
/// imported" names a whole list. Deleting them one command at a time would leave
/// the user N undo steps to walk back through, so `DeleteSection` is batch-allowed
/// and the frontend sends one `execute_batch`. The undo has to bring back ALL of
/// them, not the last one.
#[test]
fn batched_section_deletes_are_one_undo_entry() {
    let state = state_with_project("Withdraw");

    let make = |name: &str| {
        state
            .execute(Command::CreateSection {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots: vec![shot(5.0)],
            })
            .section_id
            .unwrap()
    };
    let a = make("A");
    let b = make("B");
    let c = make("C");
    assert_eq!(state.get_project_state().unwrap().sections.len(), 3);

    let removed = state.execute(Command::Batch {
        commands: vec![
            Command::DeleteSection { section_id: a },
            Command::DeleteSection { section_id: b },
        ],
    });
    assert!(removed.success, "batched delete: {:?}", removed.error);
    let after: Vec<_> = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(after, vec![c], "both named sections should be gone");

    // ONE undo entry, not two: a single undo restores both.
    assert!(state.undo().success);
    let restored: Vec<_> = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(
        restored,
        vec![a, b, c],
        "one undo must restore every section the batch removed"
    );
}

/// The parity argument that lets `DeleteSection` into a batch, pinned.
///
/// `Command::reversed_section_ids` PARITY-REDUCES a batch's reverse log, and the
/// doc comment justifies that by saying a batch cannot change a surviving
/// section's shot count. Admitting `DeleteSection` puts one shape in reach that
/// the comment now has to answer for: reverse a section and then delete it in
/// the same batch. The claim is that `settle_reversed_sections` drops a section
/// that is gone (it resolves each id through `get_section`), so the reverse is
/// reported to nobody and there is nothing to fix up on a section that no longer
/// exists. This is that claim as a test rather than as prose.
#[test]
fn a_batch_may_reverse_a_section_and_then_delete_it() {
    let state = state_with_project("Mixed");

    let doomed = state
        .execute(Command::CreateSection {
            name: Some("Doomed".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0), shot(7.0)],
        })
        .section_id
        .unwrap();
    let keeper = state
        .execute(Command::CreateSection {
            name: Some("Keeper".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(9.0)],
        })
        .section_id
        .unwrap();

    let result = state.execute(Command::Batch {
        commands: vec![
            Command::ReverseSection {
                section_id: doomed,
                swap_left_right: false,
            },
            Command::DeleteSection { section_id: doomed },
        ],
    });

    assert!(result.success, "mixed batch: {:?}", result.error);
    // The reverse is reported to NOBODY: the section it names is gone, so there
    // is no shot count to divide by and nothing for the frontend to mirror.
    assert!(
        result
            .reversed_sections
            .as_ref()
            .is_none_or(|entries| entries.iter().all(|e| e.section_id != doomed)),
        "a section deleted in the same batch must not be reported as reversed"
    );
    let after: Vec<_> = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(after, vec![keeper]);

    // And it all comes back together, still one undo entry.
    assert!(state.undo().success);
    let restored: Vec<_> = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(restored, vec![doomed, keeper]);
}

/// The other order is a clean REFUSAL, not a corruption: the section is already
/// gone when the reverse runs, so the sub-command fails and `execute_batch`
/// rolls the whole thing back. Pinned because "the batch rejects it" is half of
/// why the parity argument holds - a batch cannot reverse a section twice around
/// a delete if the delete makes every later reference fail.
#[test]
fn a_batch_that_reverses_a_section_after_deleting_it_rolls_back() {
    let state = state_with_project("Mixed");

    let section_id = state
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

    let result = state.execute(Command::Batch {
        commands: vec![
            Command::DeleteSection { section_id },
            Command::ReverseSection {
                section_id,
                swap_left_right: false,
            },
        ],
    });

    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("not found"));
    let after: Vec<_> = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(after, vec![section_id], "the delete must have rolled back");
}

/// A batch that adds two zero datums places the SECOND against the origin the
/// FIRST one established, not against wherever the survey happened to sit when
/// the batch started.
///
/// `execute_batch` runs its sub-commands through `execute_without_undo` and
/// applies `renormalize_frame_if_changed` ONCE, after the last of them - so the
/// establishing re-frame has to happen inside `Project::set_zero_datum` itself.
/// Left to the batch epilogue, sub-command two reads "the frame datum's station
/// sits at (0, 0)" while it does not yet, places B against that origin, and the
/// epilogue then translates the already-misplaced B along with everything else:
/// B lands off by the whole establishing delta and the two datums no longer sit
/// at their true GPS separation.
///
/// Non-vacuous by construction. A's datum station is 10 m north of its section's
/// start, so establishing the frame is a real translation rather than nothing;
/// the two GPS fixes are hundreds of metres apart, so the expected offset is
/// decades above `Project::OFFSET_DUST_M`; and the sections are separate
/// components, which is required rather than incidental - `set_zero_datum`
/// refuses a second datum inside one merge group.
#[test]
fn test_batch_second_zero_datum_lands_on_the_origin_the_first_established() {
    let state = state_with_project("Test");

    let north = |length: f64| ShotData {
        azimuth_in: 0.0,
        azimuth_out: 0.0,
        length,
        depth_in: 0.0,
        depth_out: 0.0,
        ..Default::default()
    };
    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![north(10.0)],
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
            shots: vec![north(4.0)],
        })
        .section_id
        .unwrap();

    let gps_a = GpsCoordinates::new(20.4, -87.3, 0.0).unwrap();
    let gps_b = GpsCoordinates::new(20.4, -87.295, 0.0).unwrap();
    let expected_b = crate::models::gps_to_local(&gps_b, &gps_a);
    assert!(
        expected_b.0.hypot(expected_b.1) > 100.0,
        "setup: the two datums have to sit far enough apart for the expected offset to be \
         unmistakable, got {expected_b:?}"
    );
    let datum_a_before = state
        .get_project_state()
        .unwrap()
        .compute_station_raw_coords(StationRef::new(sec_a, 1))
        .expect("A's datum station has no raw coordinates");
    assert!(
        (datum_a_before.y - 10.0).abs() < 1e-9,
        "setup: A's datum station has to start away from the origin, got {datum_a_before:?}"
    );

    let result = state.execute(Command::Batch {
        commands: vec![
            Command::SetZeroDatum {
                station_ref: StationRef::new(sec_a, 1),
                coordinates: gps_a,
            },
            Command::SetZeroDatum {
                station_ref: StationRef::new(sec_b, 0),
                coordinates: gps_b,
            },
        ],
    });
    assert!(result.success, "batch failed: {:?}", result.error);

    let project = state.get_project_state().unwrap();
    let a1 = project
        .compute_station_raw_coords(StationRef::new(sec_a, 1))
        .expect("A's datum station has no raw coordinates");
    assert!(
        a1.x.abs() < 1e-6 && a1.y.abs() < 1e-6,
        "the batch's FIRST datum establishes the frame, so its station has to sit at local \
         (0, 0), got ({}, {})",
        a1.x,
        a1.y
    );
    let b0 = project
        .compute_station_raw_coords(StationRef::new(sec_b, 0))
        .expect("B's datum station has no raw coordinates");
    assert!(
        (b0.x - expected_b.0).abs() < 1e-6 && (b0.y - expected_b.1).abs() < 1e-6,
        "the batch's SECOND datum has to sit at its GPS offset from the first, {expected_b:?}, \
         got ({}, {})",
        b0.x,
        b0.y
    );
}

// ============================================================================
// Coverage gap tests: commands.rs - delete_shot with shift-blocked
// ============================================================================

#[test]
fn test_create_section_rejects_negative_length_shot() {
    let state = state_with_project("Test");
    let result = state.execute(Command::CreateSection {
        name: Some("Bad".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            length: -5.0,
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        }],
    });
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("negative length"));
}

#[test]
fn test_create_shot_rejects_zero_length() {
    let state = state_with_project("Test");
    let section_id = {
        let result = state.execute(Command::CreateSection {
            name: None,
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0)],
        });
        result.section_id.unwrap()
    };

    let result = state.execute(Command::CreateShot {
        section_id,
        shot: ShotData {
            length: 0.0,
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        },
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("zero length"));
}

#[test]
fn test_update_shot_rejects_zero_resulting_length() {
    let state = state_with_project("Test");
    let section_id = {
        let result = state.execute(Command::CreateSection {
            name: None,
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0)],
        });
        result.section_id.unwrap()
    };
    let shot_id = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .find(|s| s.id == section_id)
        .unwrap()
        .shots[0]
        .id;

    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(0.0),
            ..Default::default()
        },
    });
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("zero length"));
}

/// A negative length is refused the same way, and refusing it is still keyed on
/// the update SETTING one.
#[test]
fn test_update_shot_rejects_an_explicitly_negative_length() {
    let state = state_with_project("Test");
    let section_id = state
        .execute(Command::CreateSection {
            name: None,
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();
    let shot_id = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .find(|s| s.id == section_id)
        .unwrap()
        .shots[0]
        .id;

    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(-1.0),
            ..Default::default()
        },
    });
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("negative length"));
}

/// Plant a zero-length shot at index 1 of a fresh two-shot section and return
/// `(section_id, [normal_shot_id, zero_length_shot_id])`.
///
/// Written onto the model directly because no command path can produce it -
/// `create_shot` and `update_shot` both refuse the value. It is a state that
/// LOADS, though: `Validator::validate_shot` reports ZERO_LENGTH as a validation
/// error on a legacy or collaborator file, it does not refuse the file.
fn section_holding_a_zero_length_shot(state: &AppState) -> (SectionId, [ShotId; 2]) {
    let section_id = state
        .execute(Command::CreateSection {
            name: None,
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0), shot(4.0)],
        })
        .section_id
        .unwrap();
    let mut guard = state.project.lock();
    let project = guard.as_mut().unwrap();
    let section = project.get_section_mut(section_id).unwrap();
    section.shots[1].length = 0.0;
    (section_id, [section.shots[0].id, section.shots[1].id])
}

/// The stored length is not the update's length: an edit that does not touch it
/// must not be judged by it.
///
/// Falling back to the stored value made every edit on such a shot fail, which
/// locked the invalid length out of the very edits that could correct it (#413).
#[test]
fn test_update_shot_allows_a_kind_only_edit_on_a_zero_length_shot() {
    let state = state_with_project("Test");
    let (section_id, [_, zero_length]) = section_holding_a_zero_length_shot(&state);

    let result = state.execute(Command::UpdateShot {
        section_id,
        shot_id: zero_length,
        updates: ShotUpdates {
            kind: Some(ShotKind::Jump),
            ..Default::default()
        },
    });
    assert!(
        result.success,
        "a kind-only edit must not be refused for a length it does not set: {:?}",
        result.error
    );
}

/// ...and the reason that matters most: `SetShotKind` records `PreviousValues`,
/// whose undo re-enters `update_shot` with only `kind` set. While the guard
/// judged that by the stored length, the undo failed forever - `undo_inner`'s
/// epilogue pushes a failed entry straight back, so the step never retires and
/// every older entry underneath it is sealed off.
#[test]
fn test_undo_of_a_kind_change_on_a_zero_length_shot_leaves_the_history_usable() {
    let state = state_with_project("Test");
    let (section_id, [normal, zero_length]) = section_holding_a_zero_length_shot(&state);

    // The older entry, so a wedge is visible as the step BENEATH the failed one.
    assert!(
        state
            .execute(Command::SetShotKind {
                section_id,
                shot_id: normal,
                kind: ShotKind::Jump,
            })
            .success
    );
    assert!(
        state
            .execute(Command::SetShotKind {
                section_id,
                shot_id: zero_length,
                kind: ShotKind::Reference,
            })
            .success
    );

    let kind_of = |shot_id: ShotId| {
        state
            .get_project_state()
            .unwrap()
            .sections
            .iter()
            .find(|s| s.id == section_id)
            .unwrap()
            .shots
            .iter()
            .find(|s| s.id == shot_id)
            .unwrap()
            .kind
    };

    let undone = state.undo();
    assert!(
        undone.success,
        "undo of a kind change on a zero-length shot failed: {:?}",
        undone.error
    );
    assert_eq!(kind_of(zero_length), ShotKind::Normal);

    let older = state.undo();
    assert!(
        older.success,
        "the entry beneath it must still be undoable: {:?}",
        older.error
    );
    assert_eq!(kind_of(normal), ShotKind::Normal);

    // ...all the way down: the `CreateSection` under both of them drains too.
    assert!(state.undo().success, "the whole history must drain");
    assert!(!state.can_undo());
}

// ============================================================================
// Coverage gap tests: commands.rs - auto-save
// ============================================================================

// test_auto_save_triggers_on_modifying_command was removed:
// per-command auto-save was removed in favor of frontend debounced auto-save

// ============================================================================
// Coverage gap tests: commands.rs - move_station edge cases
// ============================================================================

#[test]
fn test_move_station_rejects_zero_length_result() {
    let state = state_with_project("Test");
    let result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 1.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        }],
    });
    let section_id = result.section_id.unwrap();

    // Move station 1 backwards by exactly the shot length - should create zero-length shot
    let result = state.execute(Command::MoveStation {
        station_ref: StationRef::new(section_id, 1),
        offset: Coordinates {
            x: 0.0,
            y: -1.0,
            z: 0.0,
        },
    });
    assert!(
        !result.success,
        "should reject move that creates zero-length shot"
    );
    assert!(result.error.unwrap_or_default().contains("zero-length"));
}

#[test]
fn test_move_station_last_station() {
    let state = state_with_project("Test");
    let result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(3.0)],
    });
    let section_id = result.section_id.unwrap();

    // Move last station (station index 2 for 2 shots)
    let result = state.execute(Command::MoveStation {
        station_ref: StationRef::new(section_id, 2),
        offset: Coordinates {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        },
    });
    assert!(
        result.success,
        "move last station failed: {:?}",
        result.error
    );

    // Verify only the previous shot was modified (not a next shot)
    let project = state.get_project_state().unwrap();
    let section = project
        .sections
        .iter()
        .find(|s| s.id == section_id)
        .unwrap();
    assert_eq!(section.shots.len(), 2);
}

#[test]
fn test_move_station_invalid_index() {
    let state = state_with_project("Test");
    let result = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let section_id = result.section_id.unwrap();

    let result = state.execute(Command::MoveStation {
        station_ref: StationRef::new(section_id, 999),
        offset: Coordinates {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        },
    });
    assert!(!result.success);
    assert!(result
        .error
        .unwrap_or_default()
        .contains("Invalid station index"));
}
