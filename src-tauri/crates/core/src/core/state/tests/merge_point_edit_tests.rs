//! Import hash tracking, undo after restart, shot edits near merge points.
//!
//! Lines 21912-22677 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// === Import hash tracking tests ===

#[test]
fn test_import_tracking_happy_path() {
    let state = state_with_project("Hashes");
    let hash = "abc123deadbeef".to_string();
    assert!(!state.check_duplicate_import(&hash).unwrap());
    assert!(state.track_imported_file(hash.clone()).success);
    assert!(state.check_duplicate_import(&hash).unwrap());
}

#[test]
fn test_import_tracking_idempotent() {
    let state = state_with_project("IdempHash");
    let hash = "deadbeef".to_string();
    state.track_imported_file(hash.clone());
    state.track_imported_file(hash.clone());
    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.imported_file_hashes
            .iter()
            .filter(|h| h.as_str() == "deadbeef")
            .count(),
        1
    );
}

#[test]
fn test_section_hash_tracking_batch() {
    let state = state_with_project("SectionHashes");
    let hashes = vec!["h1".to_string(), "h2".to_string(), "h3".to_string()];
    assert_eq!(
        state.check_section_hashes(&hashes).unwrap(),
        vec![false, false, false]
    );
    state.track_section_hashes(vec!["h1".to_string(), "h3".to_string()]);
    assert_eq!(
        state.check_section_hashes(&hashes).unwrap(),
        vec![true, false, true]
    );
    state.track_section_hashes(hashes.clone());
    let proj = state.get_project_state().unwrap();
    assert_eq!(
        proj.imported_section_hashes
            .iter()
            .filter(|h| h.as_str() == "h1")
            .count(),
        1,
        "h1 must not be duplicated"
    );
}

#[test]
fn test_import_tracking_no_project_returns_false() {
    let state = AppState::new().unwrap();
    let result = state.check_duplicate_import("somehash");
    assert!(result.is_ok());
    assert!(!result.unwrap());
}

#[test]
fn test_track_imported_file_no_project_fails() {
    let state = AppState::new().unwrap();
    assert!(!state.track_imported_file("hash".to_string()).success);
}

/// Withdrawing an import forgets its dedup bookkeeping, in the SAME undo entry
/// as the deletes (#394).
///
/// `execute_import_sections` records the file hash and each section's content
/// hash so a re-import is recognised as a duplicate, and `delete_section` does
/// not touch them - `cleanup_section_references` walks merges, closures,
/// offsets, comments and datums, nothing else. So removing the sections used to
/// leave the project claiming the file was already imported while none of it
/// remained, and the DMP selection dialog DESELECTS what it believes is a
/// duplicate: the obvious retry silently offered nothing to import.
#[test]
fn a_withdrawal_forgets_the_import_tracking_and_undo_restores_it() {
    let state = state_with_project("Withdraw");

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
    state.track_imported_file("file-hash".to_string());
    state.track_section_hashes(vec!["sec-a".to_string(), "sec-b".to_string()]);
    assert!(state.check_duplicate_import("file-hash").unwrap());

    // The withdrawal: delete the section AND forget what it was tracked as.
    let withdrawn = state.execute(Command::Batch {
        commands: vec![
            Command::DeleteSection { section_id },
            Command::UntrackImport {
                file_hashes: vec!["file-hash".to_string()],
                section_hashes: vec!["sec-a".to_string()],
            },
        ],
    });
    assert!(withdrawn.success, "withdrawal: {:?}", withdrawn.error);

    assert!(
        !state.check_duplicate_import("file-hash").unwrap(),
        "the file must stop counting as imported once its sections are gone"
    );
    assert_eq!(
        state
            .check_section_hashes(&["sec-a".to_string(), "sec-b".to_string()])
            .unwrap(),
        vec![false, true],
        "only the named section hash is forgotten; an untouched one stays"
    );

    // ONE undo brings back the sections AND their tracking - both live on
    // `Project`, so the batch's snapshot pair covers them together.
    assert!(state.undo().success);
    assert_eq!(state.get_project_state().unwrap().sections.len(), 1);
    assert!(
        state.check_duplicate_import("file-hash").unwrap(),
        "undoing the withdrawal must restore the duplicate flag with the sections"
    );
    assert_eq!(
        state.check_section_hashes(&["sec-a".to_string()]).unwrap(),
        vec![true]
    );
}

/// Untracking a hash nobody tracked is a no-op, not a failure: the wizard names
/// what its own run SENT to `import_sections`, and whether each hash is still
/// tracked is not something it can know. A failure here would roll back the
/// whole withdrawal batch.
#[test]
fn untracking_an_unknown_hash_succeeds() {
    let state = state_with_project("Withdraw");

    let result = state.execute(Command::Batch {
        commands: vec![Command::UntrackImport {
            file_hashes: vec!["never-seen".to_string()],
            section_hashes: vec!["nor-this".to_string()],
        }],
    });

    assert!(result.success, "{:?}", result.error);
}

// === Undo-after-restart end-to-end test ===

#[test]
fn test_undo_actually_works_after_project_reopen() {
    let temp_dir = TempDir::new().unwrap();
    let save_path = temp_dir.path().join("undo-restart.stalx");
    // Session 1
    let state1 = state_with_project("UndoRestart");
    let create_result = state1.execute(Command::CreateSection {
        name: Some("Main".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    assert!(create_result.success);
    let section_id = create_result.section_id.unwrap();
    assert!(state1.save_project(Some(save_path.clone())).success);
    // Session 2: reopen and undo
    let state2 = AppState::new().unwrap();
    assert!(state2.open_project(save_path).success);
    {
        let stack = state2.undo_stack.lock();
        assert!(stack.len() > 0, "undo stack should be restored");
    }
    assert!(state2.undo().success, "undo should work after reopen");
    assert!(
        !state2
            .get_project_state()
            .unwrap()
            .sections
            .iter()
            .any(|s| s.id == section_id),
        "section should be undone after reopen"
    );
}

// ==========================================================================
// Group 2: Shot insertion/deletion near merge points
// ==========================================================================

/// Insert a shot before a merged station - verify merge ref shifts correctly
/// and merge alignment is maintained via cascade offset propagation.
#[test]
fn test_insert_shot_before_merged_station_shifts_merge_ref() {
    let state = state_with_project("InsertBeforeMerge");

    // Section A: 3 shots (stations 0,1,2,3)
    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0), shot(3.0), shot(4.0)],
        })
        .section_id
        .unwrap();

    // Section B: 1 shot (stations 0,1)
    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(6.0)],
        })
        .section_id
        .unwrap();

    // Merge A:2 with B:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 2),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    // Get shot ID of shot at index 1 in section A (before the merged station)
    let project = state.get_project_state().unwrap();
    let shot_id_1 = project
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots[1]
        .id;

    // Insert a new shot before shot 1 - this adds a station, shifting station 2 to 3
    let insert_r = state.execute(Command::CreateShot {
        section_id: sec_a,
        shot: shot(2.0),
        after_shot_id: None,
        before_shot_id: Some(shot_id_1),
    });
    assert!(insert_r.success, "insert should succeed");

    // After insertion, A now has 4 shots (stations 0..4)
    // Original A:2 should now be A:3
    let project_after = state.get_project_state().unwrap();
    let sec_a_after = project_after
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap();
    assert_eq!(
        sec_a_after.shots.len(),
        4,
        "section A should now have 4 shots"
    );

    // The merge should now reference A:3 (shifted from A:2) and B:0 (unchanged)
    assert_eq!(
        project_after.station_merges.len(),
        1,
        "merge should still exist"
    );
    let m = &project_after.station_merges[0];
    let merge_refs: HashSet<StationRef> = [&m.station_a, &m.station_b]
        .into_iter()
        .filter_map(|k| project_after.resolve_key(k))
        .collect();
    assert!(
        merge_refs.contains(&StationRef::new(sec_b, 0)),
        "B:0 should be unchanged in merge"
    );
    assert!(
        merge_refs.contains(&StationRef::new(sec_a, 3)),
        "A:2 should have shifted to A:3 in merge, got {:?}",
        m
    );

    // Merged stations should remain colocated (the fix)
    let a3 = project_after.compute_station_raw_coords(StationRef::new(sec_a, 3));
    let b0 = project_after.compute_station_raw_coords(StationRef::new(sec_b, 0));
    if let (Some(a3c), Some(b0c)) = (a3, b0) {
        let eps = 0.01;
        assert!(
            (a3c.x - b0c.x).abs() < eps,
            "A:3 and B:0 x should be colocated after insert: {} vs {}",
            a3c.x,
            b0c.x
        );
        assert!(
            (a3c.y - b0c.y).abs() < eps,
            "A:3 and B:0 y should be colocated after insert: {} vs {}",
            a3c.y,
            b0c.y
        );
    }
}

/// Insert a shot after a merged station - verify merge ref stays unchanged.
#[test]
fn test_insert_shot_after_merged_station_keeps_merge_ref() {
    let state = state_with_project("InsertAfterMerge");

    let sec_a = state
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

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(6.0)],
        })
        .section_id
        .unwrap();

    // Merge A:1 with B:0 (middle station of A)
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 1),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    // Insert a shot AFTER shot[0] in A (between stations 1 and 2)
    let project = state.get_project_state().unwrap();
    let shot_id_0 = project
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots[0]
        .id;

    assert!(
        state
            .execute(Command::CreateShot {
                section_id: sec_a,
                shot: shot(2.0),
                after_shot_id: Some(shot_id_0),
                before_shot_id: None,
            })
            .success
    );

    // A:1 (the merged station) should remain at index 1 since we inserted after shot[0]
    // which creates a new station at index 2, shifting old station 2 to 3
    let project_after = state.get_project_state().unwrap();
    let m = &project_after.station_merges[0];
    let merge_refs: HashSet<StationRef> = [&m.station_a, &m.station_b]
        .into_iter()
        .filter_map(|k| project_after.resolve_key(k))
        .collect();
    assert!(
        merge_refs.contains(&StationRef::new(sec_a, 1)),
        "A:1 should remain unchanged since insertion is after it"
    );
    assert!(
        merge_refs.contains(&StationRef::new(sec_b, 0)),
        "B:0 should remain unchanged"
    );
}

/// Delete a mid-shot in a merged section - verify merges are rewired and
/// no dangling station references remain after the section splits.
#[test]
fn test_delete_mid_shot_splits_section_with_merge_intact() {
    let state = state_with_project("DeleteMidMerge");

    // Section A: 3 shots (stations 0,1,2,3)
    let sec_a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![shot(5.0), shot(3.0), shot(4.0)],
        })
        .section_id
        .unwrap();

    // Section B: 1 shot (stations 0,1)
    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(6.0)],
        })
        .section_id
        .unwrap();

    // Merge A:0 with B:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 0),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    // Delete the middle shot (shot[1]) from section A - this splits A
    let project = state.get_project_state().unwrap();
    let shot_id_1 = project
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots[1]
        .id;

    let delete_r = state.execute(Command::DeleteShot {
        section_id: sec_a,
        shot_id: shot_id_1,
    });
    assert!(delete_r.success, "delete should succeed");

    // After splitting, the project should be in a consistent state:
    // no merge should reference a nonexistent station
    let project_after = state.get_project_state().unwrap();
    for merge in &project_after.station_merges {
        let valid_a = project_after.resolve_key(&merge.station_a).is_some();
        let valid_b = project_after.resolve_key(&merge.station_b).is_some();
        assert!(
            valid_a,
            "merge station_a {:?} references valid station",
            merge.station_a
        );
        assert!(
            valid_b,
            "merge station_b {:?} references valid station",
            merge.station_b
        );
    }

    // The merge to B:0 should still exist (connected to one of the split fragments)
    let b_in_merge = project_after.station_merges.iter().any(|m| {
        project_after.key_is_ref(&m.station_a, StationRef::new(sec_b, 0))
            || project_after.key_is_ref(&m.station_b, StationRef::new(sec_b, 0))
    });
    assert!(
        b_in_merge,
        "B:0 merge should survive the split (rewired to fragment)"
    );
}

/// Insert shot at position 0 of a merged section - verify offset adjustment keeps
/// existing stations visually in place and merged partner remains colocated.
#[test]
fn test_insert_shot_at_position_0_adjusts_offset_for_merge() {
    let state = state_with_project("InsertPos0Merge");

    let sec_a = state
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

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(6.0)],
        })
        .section_id
        .unwrap();

    // Merge A:0 with B:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 0),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    // Record coordinates of A:1 before insertion
    let project_before = state.get_project_state().unwrap();
    let coords_a1_before = project_before.compute_station_raw_coords(StationRef::new(sec_a, 1));

    // Insert a shot before shot[0] in section A (prepend)
    let shot_id_0 = project_before
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots[0]
        .id;
    assert!(
        state
            .execute(Command::CreateShot {
                section_id: sec_a,
                shot: shot(3.0),
                after_shot_id: None,
                before_shot_id: Some(shot_id_0),
            })
            .success
    );

    let project_after = state.get_project_state().unwrap();

    // A:1 (previously A:0) was the merged station - it should have shifted to A:1
    // A:2 (previously A:1) should be at the same coordinates as before
    if let Some(before) = coords_a1_before {
        if let Some(after) = project_after.compute_station_raw_coords(StationRef::new(sec_a, 2)) {
            let eps = 0.1;
            assert!(
                (before.x - after.x).abs() < eps,
                "A:2 x should match old A:1 x: {} vs {}",
                after.x,
                before.x
            );
            assert!(
                (before.y - after.y).abs() < eps,
                "A:2 y should match old A:1 y: {} vs {}",
                after.y,
                before.y
            );
        }
    }

    // Merge should now reference the shifted station
    let m = &project_after.station_merges[0];
    let merge_refs: HashSet<StationRef> = [&m.station_a, &m.station_b]
        .into_iter()
        .filter_map(|k| project_after.resolve_key(k))
        .collect();
    assert!(
        merge_refs.contains(&StationRef::new(sec_a, 1)),
        "A:0 should have shifted to A:1 in merge"
    );
    assert!(
        merge_refs.contains(&StationRef::new(sec_b, 0)),
        "B:0 should remain unchanged"
    );
}

/// Delete the only shot+section via execute_delete_last_shot_and_section
/// when section has merges - verify merges are cleaned up.
#[test]
fn test_delete_last_shot_and_section_cleans_merges() {
    let state = state_with_project("DeleteLastMerged");

    let sec_a = state
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

    let sec_b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(6.0)],
        })
        .section_id
        .unwrap();

    // Merge A:1 with B:0
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 1),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    // Delete section A's only shot via the dedicated method
    let project = state.get_project_state().unwrap();
    let shot_id = project
        .sections
        .iter()
        .find(|s| s.id == sec_a)
        .unwrap()
        .shots[0]
        .id;
    assert!(
        state
            .execute_delete_last_shot_and_section(sec_a, shot_id)
            .success
    );

    let project_after = state.get_project_state().unwrap();
    // Section A should be gone
    assert!(
        !project_after.sections.iter().any(|s| s.id == sec_a),
        "section A should be deleted"
    );
    // Merge referencing section A should be cleaned up
    for merge in &project_after.station_merges {
        assert_ne!(
            merge.station_a.section_id, sec_a,
            "no merge should reference deleted section A"
        );
        assert_ne!(
            merge.station_b.section_id, sec_a,
            "no merge should reference deleted section A"
        );
    }
    // Section B should still exist
    assert!(
        project_after.sections.iter().any(|s| s.id == sec_b),
        "section B should remain"
    );
}

/// Insert shot with a zero datum present - verify datum station ref shifts correctly.
#[test]
fn test_insert_shot_before_zero_datum_shifts_datum_ref() {
    let state = state_with_project("InsertBeforeDatum");

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

    // Set zero datum at station 1
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(sec, 1),
                coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
            })
            .success
    );

    // Insert a shot before shot[0], shifting all stations by 1
    let project = state.get_project_state().unwrap();
    let shot_id_0 = project.sections.iter().find(|s| s.id == sec).unwrap().shots[0].id;
    assert!(
        state
            .execute(Command::CreateShot {
                section_id: sec,
                shot: shot(2.0),
                after_shot_id: None,
                before_shot_id: Some(shot_id_0),
            })
            .success
    );

    let project_after = state.get_project_state().unwrap();
    assert_eq!(
        project_after.zero_datums.len(),
        1,
        "datum should still exist"
    );
    assert!(
        project_after.key_is_ref(
            &project_after.zero_datums[0].station_ref,
            StationRef::new(sec, 2)
        ),
        "datum should have shifted from station 1 to station 2"
    );
}
