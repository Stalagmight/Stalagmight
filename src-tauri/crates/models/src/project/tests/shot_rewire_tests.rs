//! Deleting and inserting shots: previews, rewires and station-ref shifts.
//!
//! Lines 4859-5392 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

// (Removed `shift_station_refs_updates_related_collections`: the positional
// ref-shifting mechanism it exercised no longer exists. Under stable
// shot-UUID identity, cross-references never shift on insert/delete - they
// resolve to the moved station automatically - so there is nothing to shift.
// Anchor-stability is covered by the insert/delete identity tests instead.)

#[test]
fn preview_delete_shot_basic() {
    let mut project = Project::new("PreviewDelete".to_string(), None);
    let section_a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let section_b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge station_a:1 with station_b:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 1).into(),
        station_b: StationRef::new(section_b, 0).into(),
        merged_at: Utc::now(),
    });

    // Preview deleting section_a shot at position 1 (second shot)
    // removed_station = (section_a, 2)
    let shot1_id = project.get_section(section_a).unwrap().shots[1].id;
    let impact = project.preview_delete_shot(section_a, shot1_id).unwrap();
    assert_eq!(impact.removed_station, StationRef::new(section_a, 2));
    // Merge is at station_a:1, not station_a:2 => removed_merges empty
    assert!(impact.removed_merges.is_empty());

    // Preview deleting section_a shot at position 0 (first shot)
    // removed_station = (section_a, 0)
    let shot0_id = project.get_section(section_a).unwrap().shots[0].id;
    let impact = project.preview_delete_shot(section_a, shot0_id).unwrap();
    assert_eq!(impact.removed_station, StationRef::new(section_a, 0));
    // Merge is at station_a:1, not station_a:0 => removed_merges empty
    assert!(impact.removed_merges.is_empty());

    // Add a merge at station_a:2 with station_b:1
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 2).into(),
        station_b: StationRef::new(section_b, 1).into(),
        merged_at: Utc::now(),
    });

    // Preview deleting section_a's second shot again
    // removed_station = (section_a, 2), which matches the new merge
    let impact = project.preview_delete_shot(section_a, shot1_id).unwrap();
    assert_eq!(impact.removed_station, StationRef::new(section_a, 2));
    assert_eq!(impact.removed_merges.len(), 1);
    assert_eq!(
        impact.removed_merges[0].station_a,
        StationRef::new(section_a, 2)
    );
    assert_eq!(
        impact.removed_merges[0].station_b,
        StationRef::new(section_b, 1)
    );
}

#[test]
fn preview_delete_shot_with_closure_rewire() {
    let mut project = Project::new("ClosureRewire".to_string(), None);
    let section_a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let section_b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge section_a:1 with section_b:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 1).into(),
        station_b: StationRef::new(section_b, 0).into(),
        merged_at: Utc::now(),
    });

    // Add a loop closure between section_a:1 and section_b:1
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(section_a, 1).into(),
        station_b: StationRef::new(section_b, 1).into(),
        path_length: 2.0,
        misclosure_distance: 0.1,
        error_percentage: 5.0,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    // Preview deleting section_a's second shot (position 1)
    // removed_station = (section_a, 2), but closure is on station_a:1 which is NOT removed
    let shot1_id = project.get_section(section_a).unwrap().shots[1].id;
    let impact = project.preview_delete_shot(section_a, shot1_id).unwrap();
    assert_eq!(impact.removed_station, StationRef::new(section_a, 2));
    assert!(impact.removed_closures.is_empty());

    // Now add a closure with section_a:2 and section_b:1
    project.loop_closures.push(LoopClosure {
        id: 2,
        station_a: StationRef::new(section_a, 2).into(),
        station_b: StationRef::new(section_b, 1).into(),
        path_length: 3.0,
        misclosure_distance: 0.2,
        error_percentage: 6.7,
        horizontal_misclosure: 0.2,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    // Preview delete of section_a's second shot => removed_station = (section_a, 2)
    // The closure IS on the removed station
    let impact = project.preview_delete_shot(section_a, shot1_id).unwrap();
    assert_eq!(impact.removed_station, StationRef::new(section_a, 2));
    assert_eq!(impact.removed_closures.len(), 1);
    assert_eq!(impact.removed_closures[0].closure_id, 2);

    // The merge group of station_a:2 is just {station_a:2} (not merged with anything)
    // So merge_group_without is empty => no replacement candidate => closure_rewire_skipped
    assert!(impact.closure_rewire_skipped);
    assert!(impact.replacement_closure_updates.is_empty());
}

#[test]
fn preview_delete_shot_mid_shot_loop_blocks_shift() {
    let mut project = Project::new("MidShotLoop".to_string(), None);

    // Section A with 3 shots: stations 0,1,2,3
    let section_a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 1.0, 0.0, 0.0),
                shot(0.0, 0.0, 1.0, 0.0, 0.0),
                shot(0.0, 0.0, 1.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B with 1 shot: stations 0,1
    let section_b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A:0 with B:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 0).into(),
        station_b: StationRef::new(section_b, 0).into(),
        merged_at: Utc::now(),
    });

    // Merge A:3 with B:1
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 3).into(),
        station_b: StationRef::new(section_b, 1).into(),
        merged_at: Utc::now(),
    });

    // Delete middle shot of A (position 1). This is a mid-shot (pos > 0 && pos < len-1).
    // station_before = A:1, station_after = A:2
    // Path: A:1 -> (shot 0 backward) -> A:0 -> (merge) -> B:0 -> (shot 0) -> B:1
    //   -> (merge) -> A:3 -> (shot 2 backward) -> A:2
    // This connects them excluding shot at pos 1, so shift_blocked should be true.
    let mid_shot_id = project.get_section(section_a).unwrap().shots[1].id;
    let impact = project.preview_delete_shot(section_a, mid_shot_id).unwrap();
    assert!(
        impact.shift_blocked,
        "Expected shift_blocked to be true for loop"
    );
    assert!(
        !impact.shift_blocked_reasons.is_empty(),
        "Expected at least one shift_blocked reason"
    );
    assert!(
        impact.shift_blocked_reasons[0].contains("loop"),
        "Expected loop-related reason, got: {}",
        impact.shift_blocked_reasons[0]
    );
}

#[test]
fn remove_shot_with_rewire_first_shot_adjusts_offset() {
    let mut project = Project::new("OffsetAdjust".to_string(), None);

    // First shot going east (azimuth 90), second shot going north
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let first_shot_id = project.get_section(section_id).unwrap().shots[0].id;

    // Offset before should be (0, 0) by default
    let offset_before = project
        .section_offsets
        .get(&section_id)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!((offset_before.0).abs() < 1e-6);
    assert!((offset_before.1).abs() < 1e-6);

    // Remove the first shot
    let result = project.remove_shot_with_rewire(section_id, first_shot_id);
    assert!(result.is_ok());

    // Displacement of shot going east at 90°:
    // horizontal_length * sin(90°) = 10.0 * 1.0 = 10.0  (x)
    // horizontal_length * cos(90°) = 10.0 * 0.0 = ~0.0   (y)
    // After removing, offset = (0.0 + 10.0, 0.0 + ~0.0) = (~10.0, ~0.0)
    let offset_after = project
        .section_offsets
        .get(&section_id)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!(
        (offset_after.0 - 10.0).abs() < 1e-6,
        "Expected x offset ~10.0, got {}",
        offset_after.0
    );
    assert!(
        (offset_after.1).abs() < 1e-6,
        "Expected y offset ~0.0, got {}",
        offset_after.1
    );
}

#[test]
fn remove_shot_with_rewire_replaces_merges() {
    let mut project = Project::new("ReplaceMerges".to_string(), None);

    let section_a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let section_b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let section_c = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Align merge points: a:1 is at (0,1), so b:0 and c:0 need offset (0,1)
    project.set_section_offset(section_b, (0.0, 1.0));
    project.set_section_offset(section_c, (0.0, 1.0));

    // Merge section_a:1 with section_b:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 1).into(),
        station_b: StationRef::new(section_b, 0).into(),
        merged_at: Utc::now(),
    });

    // Merge section_b:0 with section_c:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_b, 0).into(),
        station_b: StationRef::new(section_c, 0).into(),
        merged_at: Utc::now(),
    });

    // Delete section_b's first shot (pos=0) => removed_station = section_b:0
    let shot_id = project.get_section(section_b).unwrap().shots[0].id;
    let (_, impact) = project.remove_shot_with_rewire(section_b, shot_id).unwrap();

    // removed_merges should include both merges involving section_b:0
    assert_eq!(impact.removed_merges.len(), 2);

    // After removal, there should be a replacement merge between section_a:1 and section_c:0
    let has_replacement = project.station_merges.iter().any(|m| {
        (project.key_is_ref(&m.station_a, StationRef::new(section_a, 1))
            && project.key_is_ref(&m.station_b, StationRef::new(section_c, 0)))
            || (project.key_is_ref(&m.station_a, StationRef::new(section_c, 0))
                && project.key_is_ref(&m.station_b, StationRef::new(section_a, 1)))
    });
    assert!(
        has_replacement,
        "Expected replacement merge between section_a:1 and section_c:0"
    );
}

#[test]
fn insert_shot_before_at_position_zero_adjusts_offset() {
    let mut project = Project::new("InsertBefore".to_string(), None);

    // Section with 1 shot going north
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    let first_shot_id = project.get_section(section_id).unwrap().shots[0].id;

    // Insert a shot going east before the first shot
    let inserted = project.insert_shot_before(
        section_id,
        first_shot_id,
        shot(90.0, 90.0, 5.0, 0.0, 0.0),
        None,
        None,
    );
    assert!(inserted.is_some());

    // Displacement of inserted shot going east at 90°:
    // (horizontal * sin(90°), horizontal * cos(90°)) = (5.0, ~0.0)
    // Offset adjusted by (-5.0, ~0.0) to keep existing stations in place
    let offset = project
        .section_offsets
        .get(&section_id)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!(
        (offset.0 - (-5.0)).abs() < 1e-6,
        "Expected x offset ~-5.0, got {}",
        offset.0
    );
    assert!(
        (offset.1).abs() < 1e-6,
        "Expected y offset ~0.0, got {}",
        offset.1
    );
}

#[test]
fn insert_shot_after_shifts_station_refs() {
    let mut project = Project::new("InsertAfterShift".to_string(), None);

    let section_a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let section_b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Add a merge between section_a:2 (after second shot) and section_b:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 2).into(),
        station_b: StationRef::new(section_b, 0).into(),
        merged_at: Utc::now(),
    });

    // Add a loop closure between section_a:2 and section_b:1
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(section_a, 2).into(),
        station_b: StationRef::new(section_b, 1).into(),
        path_length: 2.0,
        misclosure_distance: 0.1,
        error_percentage: 5.0,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.next_closure_id = 2;

    // Canonicalize the transitional Index anchors to stable shot-UUID anchors
    // (as a load would) so they track the physical station across the insert -
    // that position-independence is exactly what C1 verifies here.
    project.reanchor_station_refs();

    // Insert a shot after section_a's first shot (position 0)
    let first_shot_id = project.get_section(section_a).unwrap().shots[0].id;
    let inserted = project.insert_shot_after(
        section_a,
        first_shot_id,
        shot(0.0, 0.0, 1.0, 0.0, 0.0),
        None,
        None,
    );
    assert!(inserted.is_some());

    // The new shot goes at position 1. Old stations at index >= 2 shift to index+1.
    // So section_a:2 becomes section_a:3.
    // The merge should now reference section_a:3.
    let merge = &project.station_merges[0];
    assert!(
        project.key_is_ref(&merge.station_a, StationRef::new(section_a, 3)),
        "Merge station_a should have shifted from 2 to 3"
    );
    assert!(project.key_is_ref(&merge.station_b, StationRef::new(section_b, 0)));

    // The closure should reference section_a:3 (was section_a:2).
    let closure = &project.loop_closures[0];
    assert!(
        project.key_is_ref(&closure.station_a, StationRef::new(section_a, 3)),
        "Closure station_a should have shifted from 2 to 3"
    );
    assert!(project.key_is_ref(&closure.station_b, StationRef::new(section_b, 1)));
}

/// C1 regression: inserting a shot UPSTREAM of a merge must keep the merge on
/// the SAME physical station - the stored anchor never moves, it just resolves
/// to a higher index - and must NOT silently re-point onto the new in-range
/// station the insert created. This is the exact failure mode positional refs
/// produced (a missed shift mispointed the merge); anchors make it impossible.
#[test]
fn insert_upstream_keeps_cross_refs_on_same_physical_station() {
    let mut project = Project::new("StableId".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // Merge A:1 ↔ B:0, then canonicalize anchors as a load would.
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    project.reanchor_station_refs();
    assert!(
        project.check_merge_alignment().is_empty(),
        "merge starts aligned"
    );

    let a_endpoint = |p: &Project| -> StationKey {
        let m = &p.station_merges[0];
        if m.station_a.section_id == a {
            m.station_a.clone()
        } else {
            m.station_b.clone()
        }
    };
    assert!(project.key_is_ref(&a_endpoint(&project), StationRef::new(a, 1)));

    // Insert a shot at the FRONT of A. The merge's physical station (old A:1) is
    // now A:2; B:0 is untouched.
    let first = project.get_section(a).unwrap().shots[0].id;
    project
        .insert_shot_before(a, first, shot(45.0, 45.0, 3.0, 0.0, 0.0), None, None)
        .expect("insert at front of A");

    assert!(
        project.key_is_ref(&a_endpoint(&project), StationRef::new(a, 2)),
        "merge must follow its physical station to A:2, not stay at the new A:1"
    );
    assert!(
        !project.key_is_ref(&a_endpoint(&project), StationRef::new(a, 1)),
        "merge must NOT silently re-point onto the freshly-inserted in-range station A:1"
    );
    // And the partner stays colocated (the offset cascade kept them coincident).
    assert!(
        project.check_merge_alignment().is_empty(),
        "merge endpoints stay colocated after the upstream insert"
    );
}

/// C1 regression: a stored anchor whose shot was deleted is flagged Critical by
/// the integrity check (a dangling reference), where a positional ref could only
/// ever be out-of-range - never a silent in-range mispoint.
#[test]
fn dangling_anchor_flagged_by_integrity() {
    let mut project = Project::new("Dangling".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    project.reanchor_station_refs();
    assert!(
        project.check_structural_integrity().is_empty(),
        "sound to start"
    );

    // Forge a merge endpoint onto a shot UUID that doesn't exist in the section.
    forge_first_merge(&mut project, |m| {
        m.station_a = StationKey::new(a, StationAnchor::AfterShot("does-not-exist".to_string()));
    });
    let v = project.check_structural_integrity();
    assert!(
        v.iter().any(|x| x.category == "dangling_merge_ref"),
        "dangling anchor must be flagged: {v:?}"
    );
}
