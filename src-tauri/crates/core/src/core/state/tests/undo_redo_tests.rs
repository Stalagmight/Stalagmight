//! #41 delete-shift repros, undo/redo basics and state transitions.
//!
//! Lines 5579-6501 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// === #41 synthetic repro: deleting a shot must not shift unrelated/upstream geometry ===
//
// "On macOS, deleting a shot sometimes causes an erroneous geometry shift - the
// entire map moves." The investigation pinned this on coordinate-frame
// re-anchoring (section offsets + `cascade_connection_offsets`), not a viewport
// recenter. These exercise the real `Command::DeleteShot` path and assert on
// absolute coordinates so any global shift fails the test deterministically -
// no UI clicking required.

/// Deleting a shot in one connected component must NEVER move a station in an
/// unrelated component. The strongest "whole map jumps" sentinel: two sections,
/// each its own zero datum, sharing no merge/closure.
///
/// The delete here is the first shot of A, which removes A's station 0 and with
/// it the zero datum keyed `StationAnchor::SectionStart` - and that datum was the
/// PRIMARY one. So this command also changes the georeference frame, and the
/// oracle has to be stated in the frame-independent terms it always meant (#469,
/// item 1): B does not move IN THE WORLD. Local (0, 0) means `zero_datums[0]`'s
/// GPS, so when B's datum is promoted to first the whole survey is translated so
/// that B's datum station lands on the origin, and B's real-world position - what
/// the map draws and what the exports write - is unchanged precisely BECAUSE its
/// local coordinates moved by that one vector.
///
/// The assertion is therefore "B translated by exactly the promoted datum's own
/// former position, and by nothing else", which is strictly sharper than the
/// byte-for-byte form it replaces: a spurious shift of any size still fails, and
/// so does a translation of the wrong magnitude. Section A's own stations are not
/// comparable across the delete (a shot went away), which is why the sentinel was
/// always read off B.
#[test]
fn delete_shot_does_not_shift_independent_component() {
    let state = state_with_project("T");

    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0), shot(7.0)],
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
            shots: vec![shot(4.0), shot(6.0)],
        })
        .section_id
        .unwrap();

    // Independent datums → each component has a fixed absolute frame.
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(a, 0),
                coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
            })
            .success
    );
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(b, 0),
                coordinates: GpsCoordinates::new(46.0, -74.0, 110.0).unwrap(),
            })
            .success
    );

    let before = abs_coord_map(&state.get_project_state().unwrap());

    let a_shot0 = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .find(|s| s.id == a)
        .unwrap()
        .shots[0]
        .id;
    assert!(
        state
            .execute(Command::DeleteShot {
                section_id: a,
                shot_id: a_shot0,
            })
            .success
    );

    let after = abs_coord_map(&state.get_project_state().unwrap());

    // The delete took A's station 0, so A's datum went with it and B's was
    // promoted to primary. B's datum station is the new local origin.
    let promoted = StationRef::new(b, 0);
    let promoted_now = after[&promoted];
    assert!(
        promoted_now.x.abs() < 1e-6 && promoted_now.y.abs() < 1e-6,
        "the promoted datum's station must sit at the local origin, got {promoted_now:?}: local \
         (0, 0) means zero_datums[0]'s GPS, so a promotion that leaves the survey where it was \
         re-georeferences it by the whole inter-datum vector"
    );

    // Section B (unrelated component) is unmoved in the WORLD: every station
    // translated by exactly the vector that put the promoted datum on the origin,
    // and z (which section offsets do not carry) is untouched.
    let (dx, dy) = (before[&promoted].x, before[&promoted].y);
    assert!(
        dx.abs() > 1000.0,
        "setup: the two datums must sit kilometres apart, got ({dx}, {dy})"
    );
    for idx in 0..=2 {
        let r = StationRef::new(b, idx);
        assert_coords_close(
            after[&r],
            Coordinates {
                x: before[&r].x - dx,
                y: before[&r].y - dy,
                z: before[&r].z,
            },
        );
    }
}

/// Deleting the LAST shot of a section removes only its end station; the section
/// head and a neighbor merged onto that head must not move.
#[test]
fn delete_tail_shot_does_not_shift_merged_head_neighbor() {
    let state = state_with_project("T");

    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0), shot(7.0)],
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
            shots: vec![shot(4.0), shot(6.0)],
        })
        .section_id
        .unwrap();

    // B hangs off A's HEAD (A:0), which is upstream of the tail shot we delete.
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(a, 0),
                coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(a, 0),
                station_b: StationRef::new(b, 0),
            })
            .success
    );

    let before = abs_coord_map(&state.get_project_state().unwrap());

    let a_last = {
        let p = state.get_project_state().unwrap();
        let sec = p.sections.iter().find(|s| s.id == a).unwrap();
        sec.shots[sec.shots.len() - 1].id
    };
    assert!(
        state
            .execute(Command::DeleteShot {
                section_id: a,
                shot_id: a_last,
            })
            .success
    );

    let after = abs_coord_map(&state.get_project_state().unwrap());

    // A's head (A:0) and the merged neighbor B are fully untouched by a tail delete.
    let a0 = StationRef::new(a, 0);
    assert_coords_close(after[&a0], before[&a0]);
    for idx in 0..=2 {
        let r = StationRef::new(b, idx);
        assert_coords_close(after[&r], before[&r]);
    }
    // A:1 becomes the new end station: its X/Y (the map) must not move, though its
    // Z legitimately changes as the deleted shot's depth conflict at this interior
    // station is resolved. The #41 bug is an X/Y shift, so guard X/Y here.
    let a1 = StationRef::new(a, 1);
    let eps = 1e-6;
    assert!(
        (after[&a1].x - before[&a1].x).abs() < eps,
        "A:1 x shifted (map moved)"
    );
    assert!(
        (after[&a1].y - before[&a1].y).abs() < eps,
        "A:1 y shifted (map moved)"
    );
}

/// Deleting the FIRST shot keeps the remaining stations in place (offset is
/// adjusted by the removed shot's displacement); a neighbor merged onto the
/// downstream/tail end must therefore stay put too - i.e. the offset cascade
/// must not over-propagate into a global shift.
#[test]
fn delete_first_shot_keeps_downstream_merged_neighbor_put() {
    let state = state_with_project("T");

    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0), shot(7.0)],
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
            shots: vec![shot(8.0)],
        })
        .section_id
        .unwrap();

    // C hangs off A's TAIL (A:2). Deleting A's first shot must keep A:2 (and thus
    // C) at the same absolute position.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(a, 2),
                station_b: StationRef::new(c, 0),
            })
            .success
    );

    let before = abs_coord_map(&state.get_project_state().unwrap());

    let a_shot0 = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .find(|s| s.id == a)
        .unwrap()
        .shots[0]
        .id;
    assert!(
        state
            .execute(Command::DeleteShot {
                section_id: a,
                shot_id: a_shot0,
            })
            .success
    );

    let after = abs_coord_map(&state.get_project_state().unwrap());

    // C is downstream of the kept side; its absolute position must not shift.
    for idx in 0..=1 {
        let r = StationRef::new(c, idx);
        assert_coords_close(after[&r], before[&r]);
    }
}

/// #27: unreadable-kind diagnostics belong to the load that found them.
///
/// Two loads can overlap - opening B cancels A, but a cancelled load keeps
/// running to its next checkpoint - so a slower A can reach the setter AFTER B
/// stored its own. An unversioned slot would then hand B the section and shot
/// ids of a project the user navigated away from, and applying that resolution
/// would write kinds onto whatever shots in B happen to share those ids.
#[test]
fn unrecognised_shot_kinds_are_scoped_to_their_load() {
    use crate::models::UnrecognisedShotKind;

    let state = AppState::new().unwrap();
    let entry = |section_id: u32| UnrecognisedShotKind {
        section_id,
        shot_id: section_id * crate::models::FEATURE_ID_STRIDE,
        shot_number: 1,
        section_name: Some(format!("s{section_id}")),
        raw_value: "7".to_string(),
    };

    // Load A (id 1), then load B (id 2) - the project the user actually opened.
    state.set_unrecognised_shot_kinds(1, vec![entry(11)]);
    state.set_unrecognised_shot_kinds(2, vec![entry(22)]);

    // The cancelled A finally reaches the setter. It must NOT clobber B.
    state.set_unrecognised_shot_kinds(1, vec![entry(11)]);

    // A asking gets nothing rather than B's list.
    assert!(
        state.take_unrecognised_shot_kinds(1).is_empty(),
        "a stale load must not receive the current load's diagnostics"
    );

    let for_b = state.take_unrecognised_shot_kinds(2);
    assert_eq!(for_b.len(), 1);
    assert_eq!(
        for_b[0].section_id, 22,
        "B must get its OWN diagnostics, not the cancelled load's"
    );

    // One-shot: a second ask returns nothing.
    assert!(state.take_unrecognised_shot_kinds(2).is_empty());
}

#[test]
fn test_set_shot_kind() {
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
    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    // Initially not reference
    assert!(!project.sections[0].shots[0].kind.is_virtual());

    let result = state.execute(Command::SetShotKind {
        section_id,
        shot_id,
        kind: ShotKind::Jump,
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert!(project.sections[0].shots[0].kind.is_virtual());
}

// === Undo/Redo Tests ===

#[test]
fn test_undo_create_section() {
    let state = state_with_project("Test");

    state.execute(Command::CreateSection {
        name: Some("To Undo".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });

    assert_eq!(state.get_project_state().unwrap().sections.len(), 1);
    assert!(state.can_undo());

    let result = state.undo();
    assert!(result.success);

    assert!(state.get_project_state().unwrap().sections.is_empty());
}

#[test]
fn test_undo_delete_section() {
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("Will Delete".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });

    let section_id = create_result.section_id.unwrap();

    state.execute(Command::DeleteSection { section_id });
    assert!(state.get_project_state().unwrap().sections.is_empty());

    // Undo the delete
    let result = state.undo();
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(project.sections[0].name, Some("Will Delete".to_string()));
}

#[test]
fn test_undo_update_shot() {
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
    let project = state.get_project_state().unwrap();
    let shot_id = project.sections[0].shots[0].id;

    state.execute(Command::UpdateShot {
        section_id,
        shot_id,
        updates: ShotUpdates {
            length: Some(15.0),
            ..Default::default()
        },
    });

    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots[0].length,
        15.0
    );

    let result = state.undo();
    assert!(result.success);

    assert_eq!(
        state.get_project_state().unwrap().sections[0].shots[0].length,
        5.0
    );
}

#[test]
fn test_redo_create_section() {
    let state = state_with_project("Test");

    state.execute(Command::CreateSection {
        name: Some("Redo Me".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    state.undo();
    assert!(state.get_project_state().unwrap().sections.is_empty());
    assert!(state.can_redo());

    let result = state.redo();
    assert!(result.success);

    assert_eq!(state.get_project_state().unwrap().sections.len(), 1);
}

#[test]
fn test_undo_nothing() {
    let state = state_with_project("Test");

    assert!(!state.can_undo());

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Nothing to undo"));
}

#[test]
fn test_redo_nothing() {
    let state = state_with_project("Test");

    assert!(!state.can_redo());

    let result = state.redo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Nothing to redo"));
}

#[test]
fn test_redo_cleared_on_new_command() {
    let state = state_with_project("Test");

    state.execute(Command::CreateSection {
        name: Some("First".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    state.undo();
    assert!(state.can_redo());

    // New command clears redo stack
    state.execute(Command::CreateSection {
        name: Some("Different".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![],
    });

    assert!(!state.can_redo());
}

#[test]
fn test_undo_fails_when_undo_stack_poisoned() {
    let state = AppState::new().unwrap();
    poison_mutex(&state.undo_stack);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error, Some(refused_after_crash("Undo")));
}

#[test]
fn test_redo_fails_when_undo_stack_poisoned() {
    let state = AppState::new().unwrap();
    poison_mutex(&state.undo_stack);

    let result = state.redo();
    assert!(!result.success);
    assert_eq!(result.error, Some(refused_after_crash("Redo")));
}

#[test]
fn test_undo_fails_for_non_undoable_entry() {
    let state = AppState::new().unwrap();
    let mut stack = state.undo_stack.lock();
    stack.push_with_data(Command::CloseProject, UndoData::None);
    drop(stack);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(
        result.error.as_deref(),
        Some("This command cannot be undone")
    );
    assert!(!state.can_redo());
}

#[test]
fn test_project_snapshot_pair_undo_redo_roundtrip() {
    let state = AppState::new().unwrap();

    let mut before = Project::new("Before".to_string(), None);
    before.add_section(SectionData {
        name: Some("Before Section".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: Some(SurveyDirection::In),
        quality: 1.0,
        shots: vec![shot(1.0)],
    });

    let mut after = Project::new("After".to_string(), None);
    after.add_section(SectionData {
        name: Some("After Section".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: Some(SurveyDirection::Out),
        quality: 1.0,
        shots: vec![shot(2.0), shot(3.0)],
    });

    let before_json = serde_json::to_string(&before).unwrap();
    let after_json = serde_json::to_string(&after).unwrap();

    state.set_project_for_tests(after.clone());

    let mut stack = state.undo_stack.lock();
    stack.push_with_data(
        Command::MergeProjects {
            source_paths: Vec::new(),
            output_path: PathBuf::from("out.stalx"),
            output_name: "Merged".to_string(),
            archive_originals: false,
        },
        UndoData::ProjectSnapshotPair {
            before: before_json,
            after: after_json,
        },
    );
    drop(stack);

    assert!(state.can_undo());

    let undo_result = state.undo();
    assert!(undo_result.success);

    let project_after_undo = state.get_project_state().unwrap();
    assert_eq!(project_after_undo.name, "Before");
    assert_eq!(project_after_undo.sections[0].shots.len(), 1);
    assert!(state.can_redo());

    let redo_result = state.redo();
    assert!(redo_result.success);

    let project_after_redo = state.get_project_state().unwrap();
    assert_eq!(project_after_redo.name, "After");
    assert_eq!(project_after_redo.sections[0].shots.len(), 2);
    assert!(state.can_undo());
    assert!(!state.can_redo());
}

#[test]
fn test_undo_project_snapshot_fails_when_project_lock_poisoned() {
    let state = AppState::new().unwrap();
    let snapshot = serde_json::to_string(&Project::new("Test".to_string(), None)).unwrap();

    let mut stack = state.undo_stack.lock();
    stack.push_with_data(
        Command::Batch {
            commands: Vec::new(),
        },
        UndoData::ProjectSnapshot(snapshot),
    );
    drop(stack);

    poison_mutex(&state.project);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error, Some(refused_after_crash("Undo")));
}

#[test]
fn test_undo_project_snapshot_pair_fails_when_project_lock_poisoned() {
    let state = AppState::new().unwrap();
    let before = serde_json::to_string(&Project::new("Before".to_string(), None)).unwrap();
    let after = serde_json::to_string(&Project::new("After".to_string(), None)).unwrap();

    let mut stack = state.undo_stack.lock();
    stack.push_with_data(
        Command::Batch {
            commands: Vec::new(),
        },
        UndoData::ProjectSnapshotPair { before, after },
    );
    drop(stack);

    poison_mutex(&state.project);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error, Some(refused_after_crash("Undo")));
}

#[test]
fn test_redo_project_snapshot_pair_fails_when_project_lock_poisoned() {
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
    drop(stack);

    poison_mutex(&state.project);

    let result = state.redo();
    assert!(!result.success);
    assert_eq!(result.error, Some(refused_after_crash("Redo")));
}

#[test]
fn test_undo_merge_info_fails_when_project_lock_poisoned() {
    let state = AppState::new().unwrap();

    let mut stack = state.undo_stack.lock();
    stack.push_with_data(
        Command::MergeStations {
            station_a: StationRef::new(1, 0),
            station_b: StationRef::new(1, 1),
        },
        UndoData::MergeInfo {
            station_a: StationRef::new(1, 0),
            station_b: StationRef::new(1, 1),
            removed_closures: Vec::new(),
            previous_offsets: HashMap::new(),
        },
    );
    drop(stack);

    poison_mutex(&state.project);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error, Some(refused_after_crash("Undo")));
}

// === State Transition Tests ===

#[test]
fn test_project_modification_tracking() {
    let state = state_with_project("Test");

    let initial_modified = state.get_project_state().unwrap().modified_at;

    // Sleep a tiny bit to ensure timestamp changes
    std::thread::sleep(std::time::Duration::from_millis(1));

    state.execute(Command::CreateSection {
        name: Some("Section".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    let after_section = state.get_project_state().unwrap().modified_at;
    assert!(after_section > initial_modified);
}

#[test]
fn test_is_undoable_commands() {
    // Non-undoable commands
    assert!(!AppState::is_undoable(&Command::OpenProject {
        path: PathBuf::from("/test.stalx"),
    }));
    assert!(!AppState::is_undoable(&Command::SaveProject { path: None }));
    assert!(!AppState::is_undoable(&Command::CloseProject));
    assert!(!AppState::is_undoable(&Command::CreateProject {
        name: "Test".to_string(),
        location: None,
    }));

    // Undoable commands
    assert!(AppState::is_undoable(&Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#000000".to_string(),
        direction: None,
        shots: vec![],
    }));
    assert!(AppState::is_undoable(&Command::DeleteSection {
        section_id: 0
    }));
    assert!(AppState::is_undoable(&Command::CreateShot {
        section_id: 0,
        shot: ShotData::default(),
        after_shot_id: None,
        before_shot_id: None,
    }));
}

#[test]
fn path_bearing_commands_never_enter_shared_undo_history() {
    let private = PathBuf::from("/home/alice/private/survey.stalx");
    let commands = [
        Command::OpenProject {
            path: private.clone(),
        },
        Command::SaveProject {
            path: Some(private.clone()),
        },
        Command::MergeProjects {
            source_paths: vec![private.clone()],
            output_path: private,
            output_name: "Merged".to_string(),
            archive_originals: false,
        },
    ];

    for command in commands {
        assert!(
            !AppState::is_undoable(&command),
            "a filesystem-path command must never enter Full-mode shared undo: {command:?}"
        );
        assert!(
            !AppState::is_batch_allowed(&command),
            "a filesystem-path command must not bypass the privacy fence inside Batch: {command:?}"
        );
    }
}

#[test]
fn confirmed_push_persists_the_pruned_undo_and_redo_stacks() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("post-push-history.stalx");
    let cutoff = chrono::Utc::now();
    let row = |name: &str, stamp: chrono::DateTime<chrono::Utc>| {
        (
            serde_json::to_string(&Command::CreateProject {
                name: name.to_string(),
                location: None,
            })
            .unwrap(),
            serde_json::to_string(&UndoData::InverseCommand(Command::CloseProject)).unwrap(),
            Some(stamp.to_rfc3339()),
        )
    };
    let old = row("old", cutoff - chrono::Duration::hours(1));
    let late = row("late", cutoff + chrono::Duration::hours(1));
    let redo = row("redo", cutoff + chrono::Duration::hours(2));

    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("Post push".to_string(), None))
        .unwrap();
    db.save_speleo_link(Some(&fixture_speleo_link())).unwrap();
    db.save_undo_stack_with_timestamps(vec![old.clone(), late.clone()])
        .unwrap();
    db.save_redo_stack_with_timestamps(vec![redo.clone()])
        .unwrap();
    db.save_speleo_undo_chain(Some("previous-tip")).unwrap();
    db.save_speleo_undo_watermark(Some(&cutoff.to_rfc3339()))
        .unwrap();

    let state = AppState::new().unwrap();
    state
        .undo_stack
        .lock()
        .load_from_persisted_with_timestamps(vec![old, late], vec![redo]);
    *state.database.lock() = Some(db);

    state.speleodb_clear_undo_after_push(&cutoff.to_rfc3339(), None);

    let stack = state.undo_stack.lock();
    assert_eq!(stack.len(), 1, "the late edit remains undoable in memory");
    assert_eq!(stack.redo_len(), 0, "redo never survives a confirmed push");
    drop(stack);

    let database = state.database.lock();
    let db = database.as_ref().unwrap();
    let disk_undo = db.load_undo_stack_with_timestamps().unwrap();
    assert_eq!(disk_undo.len(), 1, "uploaded rows are removed from SQLite");
    assert!(disk_undo[0].1.contains("late"));
    assert!(db.load_redo_stack_with_timestamps().unwrap().is_empty());
    assert_eq!(db.load_speleo_undo_chain().unwrap(), None);
    assert_eq!(db.load_speleo_undo_watermark().unwrap(), None);
}
