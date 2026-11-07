//! Undoing merges, unmerges and splits; section create/delete/update.
//!
//! Lines 3510-4338 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{heading_shot, shot, state_with_project};

#[test]
fn undo_unmerge_restores_converted_closure_and_position() {
    // The orphan-closure conversion unmerge triggers is a structural side-effect;
    // undo must reverse it completely - the re-created merge AND the
    // closure-that-became-a-merge AND the original section position must all come
    // back, with no leftover merge cycle. (User report: undoing the unmerge left
    // the converted merge in place and the geometry shifted.)
    let state = state_with_project("X");
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0), shot(5.0)],
        })
        .section_id
        .unwrap();
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#222222".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });

    // Snapshot the pre-unmerge state we expect undo to restore exactly.
    let before = state.get_project_state().unwrap();
    let offset_before = before.section_offset(b);
    let merges_before = before.station_merges.len();
    assert_eq!(before.loop_closures.len(), 1);

    // Unmerge → closure becomes a merge, section B repositions onto A0.
    let r = state.execute(Command::UnmergeStationPair {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(r.success, "{:?}", r.error);
    let converted = state.get_project_state().unwrap();
    assert!(
        converted.loop_closures.is_empty(),
        "closure should have converted to a merge"
    );
    let offset_converted = converted.section_offset(b);
    assert!(
        (offset_converted.0 - offset_before.0).abs() > 1e-9
            || (offset_converted.1 - offset_before.1).abs() > 1e-9,
        "precondition: conversion must actually move B, else the position check is vacuous"
    );

    // Undo → merge, closure, and position all come back.
    let u = state.undo();
    assert!(u.success, "{:?}", u.error);
    let restored = state.get_project_state().unwrap();
    assert_eq!(
        restored.loop_closures.len(),
        1,
        "undo must turn the converted merge back into a closure"
    );
    assert_eq!(
        restored.station_merges.len(),
        merges_before,
        "undo must restore exactly the original merge - no leftover cycle"
    );
    assert!(
        restored.check_orphan_closures().is_empty(),
        "with the merge restored the closure is no longer a sole bridge"
    );
    assert!(
        !restored.has_critical_integrity_violations(),
        "restored state must be structurally clean"
    );
    let offset_restored = restored.section_offset(b);
    assert!(
        (offset_restored.0 - offset_before.0).abs() < 1e-9
            && (offset_restored.1 - offset_before.1).abs() < 1e-9,
        "undo must reset section position: before={offset_before:?} after={offset_restored:?}"
    );

    // Redo → back to the converted state.
    let re = state.redo();
    assert!(re.success, "{:?}", re.error);
    let redone = state.get_project_state().unwrap();
    assert!(
        redone.loop_closures.is_empty(),
        "redo must re-apply the closure→merge conversion"
    );
    assert!(redone.check_orphan_closures().is_empty());
    let offset_redone = redone.section_offset(b);
    assert!(
        (offset_redone.0 - offset_converted.0).abs() < 1e-9
            && (offset_redone.1 - offset_converted.1).abs() < 1e-9,
        "redo must restore the converted position"
    );
}

#[test]
fn undo_unmerge_singular_restores_converted_closure_and_position() {
    // Same round-trip as the pair variant, but via the singular Command::UnmergeStations
    // (which removes ALL merges on a station). It shares the conversion + snapshot path,
    // so this guards against the two unmerge variants diverging.
    let state = state_with_project("X");
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0), shot(5.0)],
        })
        .section_id
        .unwrap();
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#222222".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });

    let offset_before = state.get_project_state().unwrap().section_offset(b);

    // Singular unmerge of the merged station strands B onto the closure.
    let r = state.execute(Command::UnmergeStations {
        station_ref: StationRef::new(a, 2),
    });
    assert!(r.success, "{:?}", r.error);
    let converted = state.get_project_state().unwrap();
    assert!(
        converted.loop_closures.is_empty(),
        "closure should have converted to a merge"
    );

    let u = state.undo();
    assert!(u.success, "{:?}", u.error);
    let restored = state.get_project_state().unwrap();
    assert_eq!(
        restored.loop_closures.len(),
        1,
        "undo reverts the converted merge to a closure"
    );
    assert!(restored.check_orphan_closures().is_empty());
    assert!(!restored.has_critical_integrity_violations());
    let offset_restored = restored.section_offset(b);
    assert!(
        (offset_restored.0 - offset_before.0).abs() < 1e-9
            && (offset_restored.1 - offset_before.1).abs() < 1e-9,
        "undo must reset position: before={offset_before:?} after={offset_restored:?}"
    );
}

#[test]
fn undo_merge_restores_aligned_closure_section_outside_merge_groups() {
    // A merge re-anchors not just the two merge groups but any section reachable
    // through an ALIGNED loop closure (cascade phase 2). The old lightweight MergeInfo
    // undo captured only the two groups' offsets, so such an external section stayed
    // drifted on undo - and realign_merges() can't self-heal closure drift (it inspects
    // merges only). With the full-snapshot undo, undo must restore EVERY section the
    // cascade could touch.
    let state = state_with_project("X");
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#222222".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();
    let z = state
        .execute(Command::CreateSection {
            name: Some("Z".to_string()),
            surveyor: None,
            date: None,
            color: "#333333".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();

    // Anchor A to GPS so the merge moves the B side (and, via the aligned closure, Z).
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(45.0, 90.0, 0.0).unwrap(),
    });

    // Inject an ALIGNED loop closure B.1 <-> Z.0 (no merge). Z is reachable from the
    // rest only through this aligned closure, so the cascade re-anchors it on merge.
    {
        let mut proj_guard = state.project.lock();
        let project = proj_guard.as_mut().unwrap();
        let closure_id = project.next_closure_id;
        project.loop_closures.push(LoopClosure {
            id: closure_id,
            station_a: StationRef::new(b, 1).into(),
            station_b: StationRef::new(z, 0).into(),
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

    let z_before = state.get_project_state().unwrap().section_offset(z);

    // Merge A.1 <-> B.0: A holds the datum, so B (and Z via the aligned closure) move.
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    assert!(r.success, "{:?}", r.error);

    let z_merged = state.get_project_state().unwrap().section_offset(z);
    assert!(
        (z_merged.0 - z_before.0).abs() > 1e-6 || (z_merged.1 - z_before.1).abs() > 1e-6,
        "precondition: the merge cascade must re-anchor Z (else vacuous): \
         before={z_before:?} merged={z_merged:?}"
    );

    // Undo must restore Z exactly - not just the two merge groups.
    let u = state.undo();
    assert!(u.success, "{:?}", u.error);
    let z_restored = state.get_project_state().unwrap().section_offset(z);
    assert!(
        (z_restored.0 - z_before.0).abs() < 1e-9 && (z_restored.1 - z_before.1).abs() < 1e-9,
        "undo must restore the aligned-closure section outside the merge groups: \
         before={z_before:?} after_undo={z_restored:?}"
    );
}

#[test]
fn undo_move_station_restores_pitch_and_adjacent_shots() {
    // MoveStation rewrites the two adjacent shots' raw geometry including pitch.
    // The old lightweight StationMoveInfo undo restored azimuth/length/depth but
    // NOT pitch_in/pitch_out, so undo left pitch corrupted. Full-snapshot undo must
    // restore every field.
    let state = state_with_project("X");
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0), shot(5.0)],
        })
        .section_id
        .unwrap();

    // `shot` leaves pitch unset (None). Capture the adjacent shots' state.
    let before = state.get_project_state().unwrap();
    let sec_before = before.get_section(a).unwrap();
    let shot0_before = sec_before.shots[0].clone();
    let shot1_before = sec_before.shots[1].clone();
    assert_eq!(shot0_before.pitch_in, None);
    assert_eq!(shot1_before.pitch_in, None);

    // Move station 1 (the boundary between shot 0 and shot 1).
    let r = state.execute(Command::MoveStation {
        station_ref: StationRef::new(a, 1),
        offset: Coordinates::new(2.0, 3.0, 1.5),
    });
    assert!(r.success, "{:?}", r.error);

    // Precondition: the move set pitch on the adjacent shots (so undo has work to do).
    let moved = state.get_project_state().unwrap();
    let sec_moved = moved.get_section(a).unwrap();
    assert!(
        sec_moved.shots[0].pitch_in.is_some() || sec_moved.shots[1].pitch_in.is_some(),
        "precondition: the move must set pitch on an adjacent shot"
    );

    // Undo must restore pitch (the bug) and every other field exactly.
    let u = state.undo();
    assert!(u.success, "{:?}", u.error);
    let restored = state.get_project_state().unwrap();
    let sec_r = restored.get_section(a).unwrap();
    assert_eq!(
        sec_r.shots[0].pitch_in, shot0_before.pitch_in,
        "pitch_in[0] restored"
    );
    assert_eq!(
        sec_r.shots[0].pitch_out, shot0_before.pitch_out,
        "pitch_out[0] restored"
    );
    assert_eq!(
        sec_r.shots[1].pitch_in, shot1_before.pitch_in,
        "pitch_in[1] restored"
    );
    assert_eq!(
        sec_r.shots[1].pitch_out, shot1_before.pitch_out,
        "pitch_out[1] restored"
    );
    assert!(
        (sec_r.shots[0].length - shot0_before.length).abs() < 1e-9,
        "length[0] restored"
    );
    assert!(
        (sec_r.shots[1].depth_in - shot1_before.depth_in).abs() < 1e-9,
        "depth_in[1] restored"
    );
    assert!(
        (sec_r.shots[0].azimuth_in - shot0_before.azimuth_in).abs() < 1e-9,
        "azimuth_in[0] restored"
    );
}

#[test]
fn split_converts_orphaned_bridge_closure_immediately() {
    // The split path bypasses execute()'s reconcile; confirm it still converts a
    // closure that the split strands as a section's sole bridge (the reported
    // "not transformed immediately on split" bug).
    let state = state_with_project("X");
    // Section A: 3 shots → stations 0..3 (so we can split at the middle shot).
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0), shot(5.0), shot(5.0)],
        })
        .section_id
        .unwrap();
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#222222".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    // B hangs off A's last station; a closure A0=B1 closes the loop.
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 3),
        station_b: StationRef::new(b, 0),
    });
    let closure = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });
    assert!(closure.success, "{:?}", closure.error);
    assert_eq!(state.get_project_state().unwrap().loop_closures.len(), 1);

    // Split A at its MIDDLE shot - the loop's path through A breaks, leaving the
    // closure as the only connection between A's first half and B.
    let mid = state.get_project_state().unwrap().sections[0].shots[1].id;
    let r = state.execute_delete_shot_and_split(a, mid, "A-1".to_string(), "A-2".to_string());
    assert!(r.success, "{:?}", r.error);

    let p = state.get_project_state().unwrap();
    assert!(
        p.loop_closures.is_empty(),
        "split should immediately convert the orphaned closure to a merge: {:?}",
        p.loop_closures
    );
    assert!(p.check_orphan_closures().is_empty());
}

#[test]
fn split_section_does_not_promote_a_bridge_closure_across_two_anchored_components() {
    // The split rewrites every cross-reference through `map_station_refs`, which
    // can move a MERGE endpoint into the newly created section. The merge-only
    // adjacency the double-anchor guard consults must reflect that immediately:
    // answering from the pre-split shape reports the moved section as unanchored,
    // and `convert_orphan_closures_to_merges` then promotes a legitimate bridge
    // closure into a rigid merge - fusing two independently GPS-anchored
    // components ("Merged stations X and Y are N m apart" + "Merge-connected group
    // has 2 zero datums").
    let state = state_with_project("Bridge");
    let mk = |name: &str, color: &str, azimuth: f64| {
        state
            .execute(Command::CreateSection {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: color.to_string(),
                direction: Some(SurveyDirection::In),
                shots: vec![
                    heading_shot(azimuth, 5.0),
                    heading_shot(azimuth, 5.0),
                    heading_shot(azimuth, 5.0),
                ],
            })
            .section_id
            .unwrap()
    };
    // Three straight 3-shot runs (stations 0..3 each).
    let a = mk("A", "#111111", 0.0);
    let b = mk("B", "#222222", 90.0);
    let c = mk("C", "#333333", 0.0);

    // A is GPS-anchored.
    let r = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    assert!(r.success, "{:?}", r.error);

    // B hangs rigidly off A by its LAST station, so the split below moves that
    // merge endpoint into the new section.
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 3),
        station_b: StationRef::new(b, 3),
    });
    assert!(r.success, "{:?}", r.error);

    // C is a second, independently GPS-anchored survey.
    let r = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(c, 0),
        coordinates: GpsCoordinates::new(10.001, 20.0, 0.0).unwrap(),
    });
    assert!(r.success, "{:?}", r.error);

    // The ONLY link between the two anchored components: a soft loop closure.
    // Legitimate as a closure, illegal as a merge.
    let r = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(b, 2),
        station_b: StationRef::new(c, 0),
        closure_strategy: ClosureStrategy::Batch,
    });
    assert!(r.success, "{:?}", r.error);

    // Split B at its middle shot: stations 2 (closure endpoint) and 3 (merge
    // endpoint) both move into the new section.
    let mid = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .find(|s| s.id == b)
        .expect("section B")
        .shots[1]
        .id;
    let r = state.execute_delete_shot_and_split(b, mid, "B-1".to_string(), "B-2".to_string());
    assert!(r.success, "{:?}", r.error);

    let p = state.get_project_state().unwrap();
    assert_eq!(
        p.loop_closures.len(),
        1,
        "the bridge closure must survive the split as a closure: {:?}",
        p.loop_closures
    );
    assert_eq!(
        p.station_merges.len(),
        1,
        "only the original A-B merge may exist; the split must not promote the bridge closure: {:?}",
        p.station_merges
    );
    assert!(
        p.station_merges
            .iter()
            .all(|m| m.station_a.section_id != c && m.station_b.section_id != c),
        "no merge may bridge into the second anchored component: {:?}",
        p.station_merges
    );
    assert!(
        p.check_merge_alignment().is_empty(),
        "a merge fusing two anchored components tears apart: {:?}",
        p.check_merge_alignment()
    );
    assert!(
        p.over_anchored_merge_components().is_empty(),
        "the two datums must stay in separate merge components: {:?}",
        p.over_anchored_merge_components()
    );
}

#[test]
fn create_section_rejects_non_finite_geometry() {
    let state = state_with_project("X");
    let mut bad = shot(5.0);
    bad.length = f64::NAN;
    let r = state.execute(Command::CreateSection {
        name: None,
        surveyor: None,
        date: None,
        color: "#ffffff".to_string(),
        direction: None,
        shots: vec![bad],
    });
    assert!(!r.success, "create_section must reject non-finite geometry");
    assert_eq!(
        state.get_project_state().unwrap().sections.len(),
        0,
        "nothing committed"
    );
}

#[test]
fn move_station_rejects_non_finite_offset() {
    let state = state_with_project("X");
    let a = state
        .execute(Command::CreateSection {
            name: None,
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();
    let r = state.execute(Command::MoveStation {
        station_ref: StationRef::new(a, 1),
        offset: Coordinates::new(f64::INFINITY, 0.0, 0.0),
    });
    assert!(!r.success, "move_station must reject a non-finite offset");
    let p = state.get_project_state().unwrap();
    assert!(
        p.sections[0].shots[0].all_finite(),
        "shot geometry left untouched"
    );
}

#[test]
fn redo_insert_shot_keeps_merged_stations_aligned() {
    let state = state_with_project("Align");

    // Section A (id 0): two shots → stations A0, A1, A2.
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0), shot(7.0)],
        })
        .section_id
        .unwrap();
    // Section B (id 1): one shot → stations B0, B1.
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#222222".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();

    // Anchor A at a zero datum, then merge B0 onto A2 (B hangs off A's tail).
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    let merge = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(merge.success, "{:?}", merge.error);

    let aligned = |state: &AppState| {
        state
            .get_project_state()
            .unwrap()
            .check_merge_alignment()
            .is_empty()
    };
    assert!(aligned(&state), "merge should start aligned");

    // Insert a shot after A's first shot (between A1 and A2): shifts the tail and
    // the merge point. The forward path cascades B back into alignment.
    let after_id = state.get_project_state().unwrap().sections[0].shots[0].id;
    let ins = state.execute(Command::CreateShot {
        section_id: a,
        shot: shot(6.0),
        after_shot_id: Some(after_id),
        before_shot_id: None,
    });
    assert!(ins.success, "{:?}", ins.error);
    assert!(aligned(&state), "forward insert should stay aligned");

    // Undo (already correct) then redo - redo used to omit the offset cascade and
    // leave B drifted tens of metres. The post-history reconcile must prevent it.
    assert!(state.undo().success);
    assert!(aligned(&state), "undo should stay aligned");
    assert!(state.redo().success);
    assert!(aligned(&state), "redo must not desync the merge geometry");
}

#[test]
fn test_create_section_returns_id() {
    let state = state_with_project("Test");

    let result = state.execute(Command::CreateSection {
        name: Some("First".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    assert!(result.success);
    assert_eq!(result.section_id, Some(0));

    // Create another section
    let result2 = state.execute(Command::CreateSection {
        name: Some("Second".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![],
    });

    assert!(result2.success);
    assert_eq!(result2.section_id, Some(1));
}

#[test]
fn test_create_section_no_project() {
    let state = AppState::new().unwrap();

    let result = state.execute(Command::CreateSection {
        name: Some("Orphan".to_string()),
        surveyor: None,
        date: None,
        color: "#000000".to_string(),
        direction: None,
        shots: vec![],
    });

    assert!(!result.success);
    assert!(result.error.is_some());
}

#[test]
fn test_delete_section() {
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("To Delete".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(3.0)],
    });

    let section_id = create_result.section_id.unwrap();

    let result = state.execute(Command::DeleteSection { section_id });
    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert!(project.sections.is_empty());
}

#[test]
fn test_delete_nonexistent_section() {
    let state = state_with_project("Test");

    let result = state.execute(Command::DeleteSection { section_id: 999 });
    assert!(!result.success);
}

#[test]
fn test_update_section_metadata() {
    let state = state_with_project("Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![],
    });

    let section_id = create_result.section_id.unwrap();

    let result = state.execute(Command::UpdateSectionMetadata {
        section_id,
        updates: SectionUpdates {
            name: Some(Some("Updated".to_string())),
            color: Some("#00ff00".to_string()),
            ..Default::default()
        },
    });

    assert!(result.success);

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections[0].name, Some("Updated".to_string()));
    assert_eq!(project.sections[0].color, "#00ff00");
}
