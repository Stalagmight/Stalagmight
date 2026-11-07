//! Bulk shot range deletion (#95), part 2: shifts, previews, split remaps.
//!
//! Lines 14126-15423 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

#[test]
fn test_delete_range_prefix_shift_not_blocked_by_internal_loop() {
    // Codex review #1: deleting a prefix range must not be blocked just because
    // an in-range shot looks loop-connected THROUGH another in-range shot.
    // Merge A:0<->A:2 makes the per-shot preview see shot 1 as part of a loop,
    // but deleting [0..=1] removes that path entirely, so the bulk shift is valid.
    let state = state_with_project("Internal Loop");
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0), shot(4.0), shot(6.0)],
    });
    let section_id = create.section_id.unwrap();
    // Same-section merge A:0 <-> A:2 (a loop through shots 0,1). MergeStations
    // would refuse it (already connected via shots), so push it raw.
    {
        let mut guard = state.project.lock();
        let proj = guard.as_mut().unwrap();
        proj.station_merges.push(StationMerge {
            station_a: StationRef::new(section_id, 0).into(),
            station_b: StationRef::new(section_id, 2).into(),
            merged_at: chrono::Utc::now(),
        });
    }
    let (s0, s1) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[0].id, sec.shots[1].id)
    };
    {
        let proj = state.get_project_state().unwrap();
        let impact = proj.preview_delete_shot_range(section_id, s0, s1).unwrap();
        assert!(
            !impact.shift_blocked,
            "prefix delete must not be blocked by an in-range loop: {:?}",
            impact.shift_blocked_reasons
        );
    }
    let result = state.execute_delete_shot_range_shift(section_id, s0, s1);
    assert!(
        result.success,
        "prefix shift should succeed: {:?}",
        result.error
    );
    assert_eq!(
        state
            .get_project_state()
            .unwrap()
            .get_section(section_id)
            .unwrap()
            .shots
            .len(),
        3
    );
}

#[test]
fn test_delete_range_suffix_shift_keeps_head_with_doomed_tail_anchor() {
    // Codex review #4: a suffix gap-close must keep the HEAD in place even when a
    // doomed tail station is anchored (merged to a datum-bearing section). The
    // per-shot keep-side decision must ignore anchors on stations the rest of
    // the range will also delete.
    let state = state_with_project("Doomed Tail");
    let create = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(10.0).with_azimuths(90.0, 90.0),
            shot(5.0).with_azimuths(90.0, 90.0),
            shot(7.0).with_azimuths(90.0, 90.0),
            shot(8.0).with_azimuths(90.0, 90.0),
            shot(6.0).with_azimuths(90.0, 90.0),
        ],
    });
    let sec_a = create.section_id.unwrap();
    let create_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0).with_azimuths(90.0, 90.0)],
    });
    let sec_b = create_b.section_id.unwrap();
    // B is datum-bearing and merged to A's END station (station 5) - the doomed tail.
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_b, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 5),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    let head_before = {
        let proj = state.get_project_state().unwrap();
        abs_coord_map(&proj)
    };
    let (s3, s4) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[3].id, sec.shots[4].id)
    };
    let result = state.execute_delete_shot_range_shift(sec_a, s3, s4);
    assert!(
        result.success,
        "suffix shift should succeed: {:?}",
        result.error
    );

    let proj = state.get_project_state().unwrap();
    let sec = proj.get_section(sec_a).unwrap();
    assert_eq!(
        sec.shots.iter().map(|s| s.length).collect::<Vec<_>>(),
        vec![10.0, 5.0, 7.0]
    );
    // Head stations 0..=3 must stay put - the doomed tail anchor must NOT shift them.
    let after = abs_coord_map(&proj);
    for i in 0..=3u32 {
        let r = StationRef::new(sec_a, i);
        assert_xy_close(*head_before.get(&r).unwrap(), *after.get(&r).unwrap());
    }
}

#[test]
fn test_delete_range_preview_surfaces_interior_split_comment() {
    // Codex review #2: an interior split drops station comments on the removed
    // interior stations; the preview must surface them so the dialog can warn.
    let state = state_with_project("Interior Comment");
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0), shot(4.0), shot(6.0)],
    });
    let section_id = create.section_id.unwrap();
    // Comment on interior station 2 of the range [1..=2].
    assert!(
        state
            .execute(Command::SetStationComment {
                station_ref: StationRef::new(section_id, 2),
                comment: Some("note".to_string()),
            })
            .success
    );
    let (s1, s2) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[1].id, sec.shots[2].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(section_id, s1, s2).unwrap();
    assert!(
        impact
            .removed_comments
            .iter()
            .any(|sr| sr.station_index == 2),
        "the interior-station comment the split drops must be surfaced: {:?}",
        impact.removed_comments
    );
}

/// [#469] The preview's `removed_comments` comes out SORTED, like the `removed_stations`
/// list built beside it.
///
/// It is collected by iterating the `station_comments` HashMap, so before the sort the
/// delete dialog listed the same comments in a different order on every run of the same
/// preview - and nothing downstream re-sorts (`BulkDeleteDialog.tsx` renders the vector
/// as given). Six commented stations make an accidentally-ascending hash order a
/// 1-in-720 coincidence rather than a coin flip.
#[test]
fn test_delete_range_preview_sorts_the_removed_comments() {
    let state = state_with_project("Comment Order");
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: (0..8).map(|i| shot(2.0 + f64::from(i))).collect(),
    });
    let section_id = create.section_id.unwrap();
    for idx in 1..=6u32 {
        assert!(
            state
                .execute(Command::SetStationComment {
                    station_ref: StationRef::new(section_id, idx),
                    comment: Some(format!("note {idx}")),
                })
                .success
        );
    }

    let (first, last) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[1].id, sec.shots[5].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj
        .preview_delete_shot_range(section_id, first, last)
        .unwrap();

    let indices: Vec<u32> = impact
        .removed_comments
        .iter()
        .map(|sr| sr.station_index)
        .collect();
    assert!(
        indices.len() >= 5,
        "the fixture must actually carry the comments it is sorting, got {indices:?}"
    );
    assert!(
        indices.windows(2).all(|w| w[0] < w[1]),
        "removed_comments must be in ascending station order, got {indices:?}"
    );
}

#[test]
fn test_delete_range_interior_preview_includes_shift_boundary_merge() {
    // Codex review #3: the interior preview's relationship set must cover the
    // gap-close (shift) removed station end_pos+1, so a merge anchored there is
    // shown before the user closes the gap (the split keeps it; the shift does not).
    let state = state_with_project("Shift Boundary");
    let create_a = state.execute(Command::CreateSection {
        name: Some("Main".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(5.0),
            shot(7.0),
            shot(3.0),
            shot(4.0),
            shot(6.0),
            shot(8.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let sec_b = create_b.section_id.unwrap();
    // Merge on station 4 = end_pos+1 for the interior range [2..=3].
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 4),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(sec_a, s2, s3).unwrap();
    assert!(
        impact.removed_merges.iter().any(|p| {
            (p.station_a.section_id == sec_a && p.station_a.station_index == 4)
                || (p.station_b.section_id == sec_a && p.station_b.station_index == 4)
        }),
        "merge on the shift-boundary station end_pos+1 must be surfaced: {:?}",
        impact.removed_merges
    );
}

#[test]
fn test_delete_range_interior_shift_not_blocked_by_closure_loop() {
    // User report: a range whose two sides are connected only through a LOOP
    // CLOSURE must NOT block the gap-close - a closure is a soft/redundant
    // constraint the delete removes/converts, so the result is still valid. (A
    // merge-based loop still blocks; see test_delete_range_shift_blocked_aborts.)
    let state = state_with_project("Closure Loop");
    let create_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(5.0),
            shot(7.0),
            shot(3.0),
            shot(4.0),
            shot(6.0),
            shot(8.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let sec_b = create_b.section_id.unwrap();
    // Head A:2 <-> B via a merge; tail A:5 <-> B via a LOOP CLOSURE. So for the
    // interior range [2..=3] the only head<->tail path is through the closure.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 2),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );
    assert!(
        state
            .execute(Command::CreateLoopClosure {
                station_a: StationRef::new(sec_a, 5),
                station_b: StationRef::new(sec_b, 1),
                closure_strategy: ClosureStrategy::default(),
            })
            .success
    );

    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    {
        let proj = state.get_project_state().unwrap();
        let impact = proj.preview_delete_shot_range(sec_a, s2, s3).unwrap();
        assert!(
            !impact.shift_blocked,
            "a closure-only loop must not block the gap-close: {:?}",
            impact.shift_blocked_reasons
        );
    }
    // And the gap-close executes cleanly (no over-constraint / alignment panic).
    let result = state.execute_delete_shot_range_shift(sec_a, s2, s3);
    assert!(
        result.success,
        "closure-loop gap-close should succeed: {:?}",
        result.error
    );
}

#[test]
fn test_delete_range_blocked_interior_impact_describes_split() {
    // Review finding (P2): when the gap-close is blocked, an interior range can
    // only SPLIT (which keeps both boundary stations), so the relationship panel
    // must describe the split - NOT list the kept tail boundary (end_pos+1) or
    // its merge as removed. (Complements the not-blocked case above, where the
    // gap-close IS offered and the boundary merge is surfaced.)
    let state = state_with_project("Blocked Impact");
    let create = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0), shot(4.0), shot(6.0)],
    });
    let sec_a = create.section_id.unwrap();
    let create_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let sec_b = create_b.section_id.unwrap();
    // Datum on interior station A:3 blocks the gap-close (it would drop the datum).
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(sec_a, 3),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    // Merge on the tail boundary A:4 (= end_pos+1 for range [2..=3]) - kept by split.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 4),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(sec_a, s2, s3).unwrap();
    assert!(
        impact.shift_blocked,
        "datum on interior station blocks the gap-close"
    );
    assert!(
        !impact
            .removed_stations
            .iter()
            .any(|sr| sr.section_id == sec_a && sr.station_index == 4),
        "split-only impact must not list the kept tail boundary: {:?}",
        impact.removed_stations
    );
    assert!(
        !impact.removed_merges.iter().any(|p| {
            (p.station_a.section_id == sec_a && p.station_a.station_index == 4)
                || (p.station_b.section_id == sec_a && p.station_b.station_index == 4)
        }),
        "the kept tail boundary's merge must not be reported as removed: {:?}",
        impact.removed_merges
    );
}

#[test]
fn test_delete_range_shift_does_not_displace_free_floating_merged_section() {
    // Codex re-review (P1): a suffix gap-close keeps the head fixed and shifts
    // the doomed tail. A per-shot cascade would push a free-floating (undatumed)
    // section merged to that tail, then the tail+merge are deleted - leaving it
    // displaced with no edge to pull it back. Deferring to one final cascade
    // keeps it in place.
    let state = state_with_project("Free Float");
    let create = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(10.0).with_azimuths(90.0, 90.0),
            shot(5.0).with_azimuths(90.0, 90.0),
            shot(7.0).with_azimuths(90.0, 90.0),
            shot(8.0).with_azimuths(90.0, 90.0),
            shot(6.0).with_azimuths(90.0, 90.0),
        ],
    });
    let sec_a = create.section_id.unwrap();
    let create_b = state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0).with_azimuths(45.0, 45.0)],
    });
    let sec_b = create_b.section_id.unwrap();
    // B is free-floating (NO datum), merged to A's END station (A:5) - the doomed tail.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 5),
                station_b: StationRef::new(sec_b, 0),
            })
            .success
    );

    let before = {
        let proj = state.get_project_state().unwrap();
        abs_coord_map(&proj)
    };
    let b0_before = *before.get(&StationRef::new(sec_b, 0)).unwrap();
    let b1_before = *before.get(&StationRef::new(sec_b, 1)).unwrap();

    let (s3, s4) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[3].id, sec.shots[4].id)
    };
    assert!(state.execute_delete_shot_range_shift(sec_a, s3, s4).success);

    let proj = state.get_project_state().unwrap();
    let after = abs_coord_map(&proj);
    // B is now disconnected from A; it must stay where it was, not be dragged by
    // the doomed tail's intermediate shift.
    assert_xy_close(b0_before, *after.get(&StationRef::new(sec_b, 0)).unwrap());
    assert_xy_close(b1_before, *after.get(&StationRef::new(sec_b, 1)).unwrap());
}

/// #464 defect B: a range gap-close whose first removal orphans a loop closure
/// promotes it to a rigid merge, and the promotion used to carry the loop's REAL
/// misclosure into a constraint that says the two endpoints coincide. The single
/// cascade the range driver runs at the end is seeded with the edited section
/// plus the datum sections, and by then the promoted pair sits in a datum-less
/// component of its own - so the BFS never enters it and
/// `debug_assert_merge_alignment("delete_shot_range_shift")` fires on a gap
/// nothing in that command could close.
///
/// Y - X - Z with a closure Y1 <-> Z1 closing the loop; the range takes out both
/// of X's merge stations, so Y is stranded on the first removal (promotion) and
/// Z on the second (the component floats free).
#[test]
fn delete_range_shift_leaves_a_promoted_closure_aligned() {
    let state = state_with_project("Range Promotion");
    let sec_x = state
        .execute(Command::CreateSection {
            name: Some("X".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![
                shot(10.0).with_azimuths(0.0, 0.0),
                shot(10.0).with_azimuths(0.0, 0.0),
                shot(10.0).with_azimuths(0.0, 0.0),
                shot(10.0).with_azimuths(0.0, 0.0),
                shot(10.0).with_azimuths(0.0, 0.0),
            ],
        })
        .section_id
        .unwrap();
    let sec_y = state
        .execute(Command::CreateSection {
            name: Some("Y".to_string()),
            surveyor: None,
            date: None,
            color: "#00ff00".to_string(),
            direction: None,
            shots: vec![shot(10.0).with_azimuths(90.0, 90.0)],
        })
        .section_id
        .unwrap();
    let sec_z = state
        .execute(Command::CreateSection {
            name: Some("Z".to_string()),
            surveyor: None,
            date: None,
            color: "#0000ff".to_string(),
            direction: None,
            shots: vec![shot(10.0).with_azimuths(270.0, 270.0)],
        })
        .section_id
        .unwrap();
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(sec_x, 0),
                coordinates: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
            })
            .success
    );
    // Y and Z hang off the two stations the range is about to remove.
    for (station, other) in [((sec_x, 2), (sec_y, 0)), ((sec_x, 3), (sec_z, 0))] {
        assert!(
            state
                .execute(Command::MergeStations {
                    station_a: StationRef::new(station.0, station.1),
                    station_b: StationRef::new(other.0, other.1),
                })
                .success
        );
    }
    // The loop's other leg, carrying a genuine 22.4 m misclosure.
    assert!(
        state
            .execute(Command::CreateLoopClosure {
                station_a: StationRef::new(sec_y, 1),
                station_b: StationRef::new(sec_z, 1),
                closure_strategy: ClosureStrategy::Batch,
            })
            .success
    );

    let (s1, s2) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_x).unwrap();
        (sec.shots[1].id, sec.shots[2].id)
    };
    let deleted = state.execute_delete_shot_range_shift(sec_x, s1, s2);
    assert!(
        deleted.success,
        "delete_shot_range_shift failed: {:?}",
        deleted.error
    );

    let proj = state.get_project_state().unwrap();
    assert!(
        proj.loop_closures.is_empty(),
        "the stranded closure must be promoted: {:?}",
        proj.loop_closures
    );
    let beyond = proj.merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M);
    assert!(
        beyond.is_empty(),
        "the range gap-close left a merge out of alignment: {beyond:?}"
    );
}

#[test]
fn test_delete_range_interior_shift_not_blocked_via_removed_tail_boundary() {
    // Codex re-review: a path from the head to the REMOVED tail boundary
    // (end_pos+1) through an external section is not a real loop - that station
    // and its merge are deleted by the gap-close - so it must not block.
    let state = state_with_project("Tail Boundary Loop");
    let create_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(5.0),
            shot(7.0),
            shot(3.0),
            shot(4.0),
            shot(6.0),
            shot(8.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_e = state.execute(Command::CreateSection {
        name: Some("E".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let sec_e = create_e.section_id.unwrap();
    // Head side A:1 <-> E:0. A:4 (= end_pos+1 for range [2..=3]) <-> E:1 is
    // already connected (A:4->A:1->E:0->E:1), so push it raw to form the loop
    // that runs through the to-be-removed tail boundary A:4.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_a, 1),
                station_b: StationRef::new(sec_e, 0),
            })
            .success
    );
    {
        let mut guard = state.project.lock();
        let proj = guard.as_mut().unwrap();
        proj.station_merges.push(StationMerge {
            station_a: StationRef::new(sec_a, 4).into(),
            station_b: StationRef::new(sec_e, 1).into(),
            merged_at: chrono::Utc::now(),
        });
    }
    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(sec_a, s2, s3).unwrap();
    assert!(
        !impact.shift_blocked,
        "close-gap connected only through the removed tail boundary must not block: {:?}",
        impact.shift_blocked_reasons
    );
}

#[test]
fn test_delete_range_interior_shift_not_blocked_when_stitch_is_redundant() {
    // Two external sections P and Q are each merged to a removed interior station
    // AND to a surviving station of the same section A. The stitch the deletion
    // would once have invented, A:2<->Q:0, is rigid-REDUNDANT: Q is already
    // merged to A:5, so section A and section Q are rigidly connected without it
    // (the merge would only close the cycle A:2-Q:0-A:5-shots-A:2, which
    // merge_stations refuses interactively). `replacement_merge_allowed` skips it,
    // so no head<->tail loop re-forms and the gap-close must be OFFERED.
    //
    // This test previously asserted the opposite. Blocking here was circular: the
    // auto-stitch fabricated the very loop that then blocked the shift. With the
    // merge gone the premise dissolves - after the gap-close A:2 and old A:5 are
    // adjacent through the closed gap with P merged to one end and Q to the
    // other, so the rigid graph is the tree P-A-Q and nothing is over-constrained.
    // P and Q stay rigidly connected through A; only their merge-graph adjacency
    // (which described a bridge station that no longer exists) is gone.
    let state = state_with_project("Redundant Stitch");
    let create_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(5.0),
            shot(7.0),
            shot(3.0),
            shot(4.0),
            shot(6.0),
            shot(8.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_p = state.execute(Command::CreateSection {
        name: Some("P".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let sec_p = create_p.section_id.unwrap();
    let create_q = state.execute(Command::CreateSection {
        name: Some("Q".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![shot(4.0)],
    });
    let sec_q = create_q.section_id.unwrap();
    // P:0 <-> head A:2, Q:0 <-> tail A:5 (valid merges).
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_p, 0),
                station_b: StationRef::new(sec_a, 2),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_q, 0),
                station_b: StationRef::new(sec_a, 5),
            })
            .success
    );
    // P:0 and Q:0 each also merged to the REMOVED interior A:3 (already connected,
    // so push raw). A:3 is the only P<->Q bridge.
    {
        let mut guard = state.project.lock();
        let proj = guard.as_mut().unwrap();
        proj.station_merges.push(StationMerge {
            station_a: StationRef::new(sec_p, 0).into(),
            station_b: StationRef::new(sec_a, 3).into(),
            merged_at: chrono::Utc::now(),
        });
        proj.station_merges.push(StationMerge {
            station_a: StationRef::new(sec_q, 0).into(),
            station_b: StationRef::new(sec_a, 3).into(),
            merged_at: chrono::Utc::now(),
        });
    }
    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(sec_a, s2, s3).unwrap();
    assert!(
        !impact.shift_blocked,
        "no loop re-forms once the redundant stitch is skipped, so the gap-close must be \
         offered: {:?}",
        impact.shift_blocked_reasons
    );
    assert!(
        !impact.replacement_merges.iter().any(|pair| {
            (pair.station_a.section_id == sec_a && pair.station_b.section_id == sec_q)
                || (pair.station_a.section_id == sec_q && pair.station_b.section_id == sec_a)
        }),
        "A and Q are already rigidly connected through Q:0<->A:5; the dialog must not promise \
         a redundant stitch: {:?}",
        impact.replacement_merges
    );
}

#[test]
fn test_delete_range_shift_undo_redo_roundtrip() {
    // Self-review (#6): the only range undo/redo test exercised the SPLIT path.
    // Cover the SHIFT path too - including a replacement merge added during the
    // deferred-cascade gap-close - so snapshot restore/redo is verified.
    let state = state_with_project("Shift Undo");
    let create_a = state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![
            shot(10.0).with_azimuths(90.0, 90.0),
            shot(5.0).with_azimuths(45.0, 45.0),
            shot(7.0).with_azimuths(90.0, 90.0),
            shot(8.0).with_azimuths(135.0, 135.0),
            shot(6.0).with_azimuths(90.0, 90.0),
            shot(4.0).with_azimuths(90.0, 90.0),
        ],
    });
    let sec_a = create_a.section_id.unwrap();
    let create_p = state.execute(Command::CreateSection {
        name: Some("P".to_string()),
        surveyor: None,
        date: None,
        color: "#00ff00".to_string(),
        direction: None,
        shots: vec![shot(4.0).with_azimuths(90.0, 90.0)],
    });
    let sec_p = create_p.section_id.unwrap();
    let create_q = state.execute(Command::CreateSection {
        name: Some("Q".to_string()),
        surveyor: None,
        date: None,
        color: "#0000ff".to_string(),
        direction: None,
        shots: vec![shot(4.0).with_azimuths(90.0, 90.0)],
    });
    let sec_q = create_q.section_id.unwrap();
    // P and Q bridge ONLY through interior A:3 (removed by the range) - the
    // gap-close adds a replacement merge P:0<->Q:0 to keep them connected.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_p, 0),
                station_b: StationRef::new(sec_a, 3),
            })
            .success
    );
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(sec_q, 0),
                station_b: StationRef::new(sec_a, 3),
            })
            .success
    );

    let before_abs = {
        let proj = state.get_project_state().unwrap();
        abs_coord_map(&proj)
    };
    let undo_len_before = state.get_undo_stack().len();
    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(sec_a).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let result = state.execute_delete_shot_range_shift(sec_a, s2, s3);
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        state.get_undo_stack().len(),
        undo_len_before + 1,
        "one undo entry"
    );
    {
        let proj = state.get_project_state().unwrap();
        assert_eq!(
            proj.get_section(sec_a).unwrap().shots.len(),
            4,
            "two shots removed"
        );
        assert!(
            proj.are_stations_merged(StationRef::new(sec_p, 0), StationRef::new(sec_q, 0)),
            "P and Q stay connected via a replacement merge after the gap-close"
        );
    }

    assert!(state.undo().success);
    {
        let proj = state.get_project_state().unwrap();
        let after_undo = abs_coord_map(&proj);
        assert_eq!(before_abs.len(), after_undo.len());
        for (r, c) in &before_abs {
            let c2 = after_undo
                .get(r)
                .unwrap_or_else(|| panic!("missing {r:?} after undo"));
            assert_coords_close(*c, *c2);
        }
        assert_eq!(
            proj.get_section(sec_a).unwrap().shots.len(),
            6,
            "undo restores all shots"
        );
    }

    assert!(state.redo().success);
    {
        let proj = state.get_project_state().unwrap();
        assert_eq!(
            proj.get_section(sec_a).unwrap().shots.len(),
            4,
            "redo re-applies the shift"
        );
        assert!(proj.are_stations_merged(StationRef::new(sec_p, 0), StationRef::new(sec_q, 0)));
    }
}

#[test]
fn test_delete_range_prefix_shift_surfaces_comment() {
    // Codex re-review: prefix/suffix shifts drop station comments on removed
    // stations; the preview must surface them so the dialog can warn (the
    // warning was previously gated to interior splits only).
    let state = state_with_project("Prefix Comment");
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0), shot(4.0)],
    });
    let section_id = create.section_id.unwrap();
    // Comment on station 1 (removed by the prefix range [0..=1]).
    assert!(
        state
            .execute(Command::SetStationComment {
                station_ref: StationRef::new(section_id, 1),
                comment: Some("note".to_string()),
            })
            .success
    );
    let (s0, s1) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[0].id, sec.shots[1].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(section_id, s0, s1).unwrap();
    assert!(
        impact
            .removed_comments
            .iter()
            .any(|sr| sr.station_index == 1),
        "prefix shift comment loss must be surfaced: {:?}",
        impact.removed_comments
    );
}

#[test]
fn test_delete_range_interior_shift_surfaces_boundary_comment() {
    // Codex re-review: the interior close-gap deletes end_pos+1 and drops a
    // comment there. With the gap-close available, the comment set covers the
    // shift removed set, so the boundary comment is surfaced before closing.
    let state = state_with_project("Boundary Comment");
    let create = state.execute(Command::CreateSection {
        name: Some("S".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0), shot(4.0), shot(6.0)],
    });
    let section_id = create.section_id.unwrap();
    // Comment on station 4 = end_pos+1 for the interior range [2..=3]. The split
    // keeps it; the gap-close drops it.
    assert!(
        state
            .execute(Command::SetStationComment {
                station_ref: StationRef::new(section_id, 4),
                comment: Some("boundary".to_string()),
            })
            .success
    );
    let (s2, s3) = {
        let proj = state.get_project_state().unwrap();
        let sec = proj.get_section(section_id).unwrap();
        (sec.shots[2].id, sec.shots[3].id)
    };
    let proj = state.get_project_state().unwrap();
    let impact = proj.preview_delete_shot_range(section_id, s2, s3).unwrap();
    assert!(
        !impact.shift_blocked,
        "no datum, so the gap-close is available"
    );
    assert!(
        impact
            .removed_comments
            .iter()
            .any(|sr| sr.station_index == 4),
        "close-gap tail-boundary comment must be surfaced: {:?}",
        impact.removed_comments
    );
}

#[test]
fn test_split_remaps_station_offsets() {
    let state = state_with_project("Station Offset Remap");

    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0)],
    });
    let section_id = create_result.section_id.unwrap();

    // Directly insert a station offset for station 3 (simulates legacy offset data)
    {
        let mut proj = state.get_project_state().unwrap();
        proj.station_offsets
            .insert(StationRef::new(section_id, 3).into(), (1.0, 2.0, 3.0));
        state.set_project_for_tests(proj);
    }

    // Split at shot index 1
    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[1].id
    };

    let result =
        state.execute_delete_shot_and_split(section_id, shot_id, "A".to_string(), "B".to_string());
    assert!(result.success);
    let new_section_id = result.section_id.unwrap();

    // Station offset should be remapped from (section_id, 3) to (new_section_id, 1)
    // split_station = 2, so 3 - 2 = 1
    let proj = state.get_project_state().unwrap();
    let new_ref = StationRef::new(new_section_id, 1);
    let offset = proj.station_offset_for_ref(new_ref);
    assert!(
        offset.is_some(),
        "Station offset should be remapped to new section"
    );
    let (ox, oy, oz) = offset.unwrap();
    assert!((ox - 1.0).abs() < 0.001);
    assert!((oy - 2.0).abs() < 0.001);
    assert!((oz - 3.0).abs() < 0.001);

    // Old ref should not exist
    let old_ref = StationRef::new(section_id, 3);
    assert!(
        !proj.station_offset_for_ref(old_ref).is_some(),
        "Old station offset should be removed"
    );
}

#[test]
fn test_split_undo_restores_original() {
    let state = state_with_project("Undo Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0)],
    });
    let section_id = create_result.section_id.unwrap();

    // Record initial state
    let initial_shot_count = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots.len()
    };
    let initial_section_count = state.get_project_state().unwrap().sections.len();

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[1].id
    };

    let result =
        state.execute_delete_shot_and_split(section_id, shot_id, "A".to_string(), "B".to_string());
    assert!(result.success);

    // Verify split happened
    let proj = state.get_project_state().unwrap();
    assert_eq!(proj.sections.len(), initial_section_count + 1);

    // Undo
    let undo_result = state.undo();
    assert!(
        undo_result.success,
        "Undo should succeed: {:?}",
        undo_result.error
    );

    // Verify restored
    let proj = state.get_project_state().unwrap();
    assert_eq!(proj.sections.len(), initial_section_count);
    let section = proj.get_section(section_id).unwrap();
    assert_eq!(section.shots.len(), initial_shot_count);
    assert_eq!(section.name, Some("Original".to_string()));
}

#[test]
fn test_split_redo_resplits() {
    let state = state_with_project("Redo Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0)],
    });
    let section_id = create_result.section_id.unwrap();

    let shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[1].id
    };

    let result =
        state.execute_delete_shot_and_split(section_id, shot_id, "A".to_string(), "B".to_string());
    assert!(result.success);
    let new_section_id = result.section_id.unwrap();

    // Undo
    let undo_result = state.undo();
    assert!(undo_result.success);

    // Redo
    let redo_result = state.redo();
    assert!(
        redo_result.success,
        "Redo should succeed: {:?}",
        redo_result.error
    );

    // Verify re-split: should have section A and section B again
    let proj = state.get_project_state().unwrap();
    let section_a = proj.get_section(section_id).unwrap();
    let section_b = proj.get_section(new_section_id).unwrap();
    assert_eq!(section_a.shots.len(), 1);
    assert_eq!(section_b.shots.len(), 1);
    assert_eq!(section_a.name, Some("A".to_string()));
    assert_eq!(section_b.name, Some("B".to_string()));
}

#[test]
fn test_split_rejects_edge_shot() {
    let state = state_with_project("Edge Test");

    let create_result = state.execute(Command::CreateSection {
        name: Some("Original".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot(5.0), shot(7.0), shot(3.0)],
    });
    let section_id = create_result.section_id.unwrap();

    // Try to split at first shot
    let first_shot_id = {
        let proj = state.get_project_state().unwrap();
        proj.get_section(section_id).unwrap().shots[0].id
    };
    let result = state.execute_delete_shot_and_split(
        section_id,
        first_shot_id,
        "A".to_string(),
        "B".to_string(),
    );
    assert!(!result.success, "Should reject first shot");
    assert!(result.error.unwrap().contains("edge shot"));

    // Try to split at last shot
    let last_shot_id = {
        let proj = state.get_project_state().unwrap();
        let section = proj.get_section(section_id).unwrap();
        section.shots[section.shots.len() - 1].id
    };
    let result = state.execute_delete_shot_and_split(
        section_id,
        last_shot_id,
        "A".to_string(),
        "B".to_string(),
    );
    assert!(!result.success, "Should reject last shot");
    assert!(result.error.unwrap().contains("edge shot"));
}
