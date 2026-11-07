//! Orphaned and broken loop closures: detection, conversion to merges and the double-anchor guards.
//!
//! Lines 11913-12920 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`). `integrity_fixture` (former lines
//! 11872-11912) moved to `structural_integrity_tests.rs`, its only user.

use super::*;

#[test]
fn check_orphan_closures_detects_sole_bridge_closure() {
    let mut p = Project::new("Orphan".to_string(), None);
    let a = p
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = p
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // A closure between A and B with NO merge connecting them: a sole bridge.
    p.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: true,
    });
    let v = p.check_orphan_closures();
    assert_eq!(v.len(), 1, "{v:?}");
    assert_eq!(v[0].category, "orphan_closure");

    // Once a merge connects the two sections, the closure is a genuine
    // (redundant) loop and is no longer flagged.
    p.record_station_merge(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 1).into(),
        merged_at: Utc::now(),
    });
    assert!(p.check_orphan_closures().is_empty());
}

#[test]
fn deleting_shot_converts_orphaned_bridge_closure_to_merge() {
    // A: A0-A1-A2 (datum A0); B: B0-B1. B hangs off A2 (merge A2=B0), and a
    // closure A0=B1 closes the loop. Now delete A's last shot (A1->A2): that
    // removes A2 and the A2=B0 merge, so A and B are connected ONLY through the
    // closure A0=B1. A closure that is the sole bridge is not a loop - it must
    // become a merge (issue #8, the section-31 case).
    let mut project = Project::new("Bridge".to_string(), None);
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
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    project.record_station_merge(StationMerge {
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 1).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: true,
    });

    let last = project.get_section(a).unwrap().shots.last().unwrap().id;
    project.remove_shot_with_rewire(a, last).unwrap();

    // The bridge closure is gone and replaced by a merge between its endpoints.
    assert!(
        project.loop_closures.is_empty(),
        "sole-bridge closure should have become a merge: {:?}",
        project.loop_closures
    );
    assert!(
        project.station_merges.iter().any(|m| {
            let pair = (m.station_a.clone(), m.station_b.clone());
            pair == (StationRef::new(a, 0).into(), StationRef::new(b, 1).into())
                || pair == (StationRef::new(b, 1).into(), StationRef::new(a, 0).into())
        }),
        "expected a merge A0<->B1; got {:?}",
        project.station_merges
    );
}

#[test]
fn undoing_an_appended_first_shot_converts_a_closure_it_orphans() {
    // A is a one-shot section, exactly as an `add_shot` append into an EMPTY
    // section leaves it, so undoing that `CreateShot` takes
    // `remove_appended_first_shot`. That inverse drops every relationship anchored
    // on the station the append created (A1) - here the merge A1=C0 - and dropping
    // it leaves the B=C closure as the SOLE connection between B and C. A closure
    // that is not a loop is a merge (#8). Every other delete entry point converts
    // it; #413 finding 10 is this one being the exception, which let an undo leave
    // behind a state the app declares impossible and the save then persisted.
    let mut project = Project::new("OrphanOnUndo".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![
                shot(180.0, 180.0, 5.0, 0.0, 0.0),
                shot(270.0, 270.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // B hangs off the station that predates the append, C off the one it created.
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(c, 0))
        .unwrap();
    // With both merges in place B and C are a genuine loop (shots + merges through
    // A one way, this closure the other), so it is legitimately a closure today.
    let closure_a = project.key_for_ref(StationRef::new(b, 2)).unwrap();
    let closure_b = project.key_for_ref(StationRef::new(c, 2)).unwrap();
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: closure_a,
        station_b: closure_b,
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    let created = project.get_section(a).unwrap().shots[0].id;
    project.remove_appended_first_shot(a, created).unwrap();

    assert!(
        project.loop_closures.is_empty(),
        "the closure stopped being a loop when the merge on the created station \
         went, so it must have become a merge: {:?}",
        project.loop_closures
    );
    let converted = project.station_merges.iter().any(|m| {
        let pair = (
            project.resolve_key(&m.station_a),
            project.resolve_key(&m.station_b),
        );
        pair == (Some(StationRef::new(b, 2)), Some(StationRef::new(c, 2)))
            || pair == (Some(StationRef::new(c, 2)), Some(StationRef::new(b, 2)))
    });
    assert!(
        converted,
        "expected the orphaned closure to be replaced by a merge B2<->C2; got {:?}",
        project.station_merges
    );
    // The merge on station 0 predates the append and survives it; the one on the
    // created station is gone. Two merges, not three.
    assert_eq!(
        project.station_merges.len(),
        2,
        "{:?}",
        project.station_merges
    );
    assert!(
        project
            .station_merges
            .iter()
            .any(
                |m| project.resolve_key(&m.station_a) == Some(StationRef::new(a, 0))
                    || project.resolve_key(&m.station_b) == Some(StationRef::new(a, 0))
            ),
        "the station that predates the append keeps its merge"
    );
    // And the conversion left no merge the app would assert on: a closure is a soft
    // edge, the merge replacing it is rigid, so the new edge is solved.
    assert!(
        project
            .merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M)
            .is_empty(),
        "the new rigid edge must be aligned"
    );
}

/// Two closures, one project, and only ONE of them is this removal's business.
///
/// B2=C2 is a true loop today - shots and merges through A one way, the closure
/// the other - and stops being one when the merge on the deleted station goes.
/// Converting it is what the call exists for, and it is converted whether or not
/// its endpoints are anywhere near the removed station.
///
/// D1=E1 bridges two sections that nothing else connects, so it is ALREADY an
/// orphan when the removal starts. Nothing about this delete made it one (the
/// fuzzer's route there is a `removeZeroDatum` two moves back, which orphans a
/// closure and converts nothing), so it is part of the state the history
/// recorded - and both removal paths also run as the inverse of `CreateShot`, an
/// inverse must not do more than invert, and converting it here would land the
/// undo on a state no snapshot ever held (#413, the open observation).
#[test]
fn removing_a_shot_converts_the_closure_it_orphans_and_not_an_older_one() {
    let mut project = Project::new("ScopedConversion".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0), shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![
                shot(180.0, 180.0, 5.0, 0.0, 0.0),
                shot(270.0, 270.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    // D and E touch nothing else in the project.
    let d = project
        .add_section(SectionData {
            shots: vec![shot(45.0, 45.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let e = project
        .add_section(SectionData {
            shots: vec![shot(135.0, 135.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    // The merge that carries the loop's other path, on the station the delete
    // below removes.
    project
        .merge_stations(StationRef::new(a, 2), StationRef::new(c, 0))
        .unwrap();
    push_closure(
        &mut project,
        1,
        StationRef::new(b, 2),
        StationRef::new(c, 2),
    );
    push_closure(
        &mut project,
        2,
        StationRef::new(d, 1),
        StationRef::new(e, 1),
    );
    assert_eq!(
        project.orphan_closure_ids(),
        HashSet::from([2]),
        "setup: D1=E1 must already be an orphan and B2=C2 must not"
    );

    let last = project.get_section(a).unwrap().shots.last().unwrap().id;
    project.remove_shot_with_rewire(a, last).unwrap();

    assert_eq!(
        project
            .loop_closures
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        vec![2],
        "the closure this delete orphaned had to become a merge and the one that arrived \
         orphaned had to be left alone; got {:?}",
        project.loop_closures
    );
    let converted = project.station_merges.iter().any(|m| {
        let pair = (
            project.resolve_key(&m.station_a),
            project.resolve_key(&m.station_b),
        );
        pair == (Some(StationRef::new(b, 2)), Some(StationRef::new(c, 2)))
            || pair == (Some(StationRef::new(c, 2)), Some(StationRef::new(b, 2)))
    });
    assert!(
        converted,
        "expected the orphaned closure to be replaced by a merge B2<->C2; got {:?}",
        project.station_merges
    );
    assert!(
        !project.station_merges.iter().any(|m| {
            m.station_a.section_id == d
                || m.station_b.section_id == d
                || m.station_a.section_id == e
                || m.station_b.section_id == e
        }),
        "the pre-existing orphan must not have become a merge: {:?}",
        project.station_merges
    );
}

/// The same scoping on the other removal path, which is the one that is ONLY
/// ever an inverse. The loop and the merge that carries it are
/// `undoing_an_appended_first_shot_converts_a_closure_it_orphans`'s (that test
/// still pins the conversion itself); what is added is the older orphan the
/// undo must not touch.
#[test]
fn removing_an_appended_first_shot_converts_the_closure_it_orphans_and_not_an_older_one() {
    let mut project = Project::new("ScopedConversionOnUndo".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![
                shot(180.0, 180.0, 5.0, 0.0, 0.0),
                shot(270.0, 270.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let d = project
        .add_section(SectionData {
            shots: vec![shot(45.0, 45.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let e = project
        .add_section(SectionData {
            shots: vec![shot(135.0, 135.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // B hangs off the station that predates the append, C off the one it created.
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(c, 0))
        .unwrap();
    push_closure(
        &mut project,
        1,
        StationRef::new(b, 2),
        StationRef::new(c, 2),
    );
    push_closure(
        &mut project,
        2,
        StationRef::new(d, 1),
        StationRef::new(e, 1),
    );
    assert_eq!(
        project.orphan_closure_ids(),
        HashSet::from([2]),
        "setup: D1=E1 must already be an orphan and B2=C2 must not"
    );

    let created = project.get_section(a).unwrap().shots[0].id;
    project.remove_appended_first_shot(a, created).unwrap();

    assert_eq!(
        project
            .loop_closures
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        vec![2],
        "the undo had to convert the closure it orphaned and leave the one that arrived \
         orphaned alone; got {:?}",
        project.loop_closures
    );
    assert!(
        !project.station_merges.iter().any(|m| {
            m.station_a.section_id == d
                || m.station_b.section_id == d
                || m.station_a.section_id == e
                || m.station_b.section_id == e
        }),
        "the pre-existing orphan must not have become a merge: {:?}",
        project.station_merges
    );
}

/// A closure between two stations, with the metrics zeroed: these tests care
/// about the closure/merge classification, never about misclosure.
fn push_closure(project: &mut Project, id: u64, station_a: StationRef, station_b: StationRef) {
    let (Some(key_a), Some(key_b)) = (
        project.key_for_ref(station_a),
        project.key_for_ref(station_b),
    ) else {
        panic!("closure endpoint {station_a} or {station_b} has no stable anchor");
    };
    project.loop_closures.push(LoopClosure {
        id,
        station_a: key_a,
        station_b: key_b,
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
}

#[test]
fn deleting_shot_converts_broken_loop_closure_to_merge() {
    // Section A: A0-A1-A2 (2 shots); Section B: B0-B1 (1 shot).
    let mut project = Project::new("Loop".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 1.0, 0.0, 0.0),
                shot(90.0, 90.0, 1.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // A2 is merged with B0 (B hangs off the end of A).
    project.record_station_merge(StationMerge {
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    // A loop closure ties A0 to A2 (the A0-A1-A2 survey is the loop).
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(a, 2).into(),
        path_length: 2.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    // Canonicalize the transitional Index anchors to stable shot-UUID anchors,
    // as every load path does before any command runs (so in-memory keys are
    // canonical, which is what remove_shot_with_rewire matches against).
    project.reanchor_station_refs();

    // Delete A's last shot (A1->A2): removes station A2, a closure endpoint. The
    // surviving B segment is detached, so the closure must become a merge.
    let last_shot_id = project.get_section(a).unwrap().shots.last().unwrap().id;
    let (_, impact) = project.remove_shot_with_rewire(a, last_shot_id).unwrap();

    assert_eq!(impact.closure_to_merge.len(), 1, "impact: {impact:?}");
    assert!(
        project.loop_closures.is_empty(),
        "broken loop should not remain a closure: {:?}",
        project.loop_closures
    );
    assert!(
        project.station_merges.iter().any(|m| {
            let pair = (m.station_a.clone(), m.station_b.clone());
            pair == (StationRef::new(b, 0).into(), StationRef::new(a, 0).into())
                || pair == (StationRef::new(a, 0).into(), StationRef::new(b, 0).into())
        }),
        "expected a merge between A0 and B0; got {:?}",
        project.station_merges
    );
    // And the result is structurally sound.
    assert!(!project.has_critical_integrity_violations());
}

/// Two independently GPS-anchored components, bridged only by a loop closure,
/// and the shot carrying the closure's endpoint deleted (#464 defect A).
///
/// * component 1: section A (4 shots north) merged tail-to-head into section C,
///   whose station 0 carries the GPS datum;
/// * component 2: section B, with its OWN GPS datum ~1.4 km away;
/// * a loop closure A4 <-> B0, which `create_loop_closure` legitimately allows
///   between two unconnected components when both are datum-anchored.
///
/// Returns `(project, a, b, c)`.
fn double_anchored_closure_fixture() -> (Project, SectionId, SectionId, SectionId) {
    let mut project = Project::new("DoubleAnchor".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // A's tail hangs C off it, and C carries component 1's anchor.
    project
        .merge_stations(StationRef::new(a, 4), StationRef::new(c, 0))
        .unwrap();
    project
        .set_zero_datum(
            StationRef::new(c, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();
    // Component 2's own anchor, far enough away that welding the two is a gap
    // no cascade could hide.
    project
        .set_zero_datum(
            StationRef::new(b, 0),
            GpsCoordinates::new(45.01, -90.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(a, 4),
            StationRef::new(b, 0),
            ClosureStrategy::Batch,
        )
        .unwrap();
    (project, a, b, c)
}

/// #464 defect A: the broken-loop arm of `preview_delete_shot` used to convert
/// an orphaned closure into a merge with NO guard at all, welding two
/// independently datum-anchored components into one rigid group. Both ends are
/// authoritative to `cascade_bfs`, so the gap is permanent and Repair cannot
/// reduce it. The closure has to SURVIVE, re-anchored onto the replacement
/// station.
#[test]
fn deleting_a_shot_never_promotes_a_closure_that_would_double_anchor() {
    let (mut project, a, b, c) = double_anchored_closure_fixture();
    let closure_id = project.loop_closures[0].id;

    // Delete A's last shot: that removes station A4, which is both the merge
    // partner of C0 and one end of the closure.
    let last_shot = project.get_section(a).unwrap().shots.last().unwrap().id;
    let (_, impact) = project.remove_shot_with_rewire(a, last_shot).unwrap();

    assert!(
        impact.closure_to_merge.is_empty(),
        "a closure between two independently anchored components must not become a merge: {impact:?}"
    );
    assert_eq!(
        impact.replacement_closure_updates.len(),
        1,
        "the refused promotion must keep the closure, re-anchored: {impact:?}"
    );
    let update = &impact.replacement_closure_updates[0];
    assert_eq!(update.closure_id, closure_id);
    assert_eq!(update.new_station, StationRef::new(c, 0));

    assert_eq!(
        project.loop_closures.len(),
        1,
        "the closure must survive: {:?}",
        project.loop_closures
    );
    let survivor = &project.loop_closures[0];
    let ends = (
        project.resolve_key(&survivor.station_a).unwrap(),
        project.resolve_key(&survivor.station_b).unwrap(),
    );
    assert_eq!(ends, (StationRef::new(c, 0), StationRef::new(b, 0)));
    assert!(
        !project.are_stations_merged(StationRef::new(c, 0), StationRef::new(b, 0)),
        "no rigid merge may exist across the two anchors: {:?}",
        project.station_merges
    );

    // ...and the refusal is STABLE: the whole-project sweep the command epilogue
    // and the loader both run declines it for the same reason, so memory and
    // disk cannot disagree about what this closure is.
    assert_eq!(project.convert_orphan_closures_to_merges(), 0);
    project.redrive_offsets_after_load();
    assert_eq!(project.loop_closures.len(), 1);
    assert!(project.station_merges.is_empty());
}

/// One doomed station carrying TWO sole-bridge closures, each into its own
/// independently GPS-anchored component (#464 defect A, chained).
///
/// * section S: two shots north, GPS-anchored at S0, its tail S2 merged into
///   T0 - so T is the station both closures rewire onto when S2 goes;
/// * section T: one shot east, no datum of its own;
/// * sections U and V: one shot each, each carrying its OWN GPS datum, about a
///   kilometre away in different directions;
/// * closures S2 <-> U0 and S2 <-> V0 (legal to create: the pairs are
///   unconnected and both ends are datum-anchored).
///
/// Deleting S's last shot removes S2. EITHER closure alone promotes legally -
/// once S2's merge is gone the replacement T holds no datum, so nothing is
/// double-anchored - but promoting BOTH welds U and V into one rigid group
/// carrying two GPS anchors, which is exactly the state defect A is about.
///
/// Returns `(project, s, t, u, v)`.
fn chained_double_anchor_closure_fixture() -> (Project, SectionId, SectionId, SectionId, SectionId)
{
    let mut project = Project::new("ChainedDoubleAnchor".to_string(), None);
    let s = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let t = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let u = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let v = project
        .add_section(SectionData {
            shots: vec![shot(270.0, 270.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // T hangs off S's tail: it is the only survivor of S2's merge group, so it
    // is the replacement station the closure rewire picks.
    project
        .merge_stations(StationRef::new(s, 2), StationRef::new(t, 0))
        .unwrap();
    project
        .set_zero_datum(
            StationRef::new(s, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();
    // Two more anchors, each far enough that welding them into one group is a
    // gap no cascade could hide.
    project
        .set_zero_datum(
            StationRef::new(u, 0),
            GpsCoordinates::new(45.01, -90.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .set_zero_datum(
            StationRef::new(v, 0),
            GpsCoordinates::new(45.0, -90.01, 0.0).unwrap(),
        )
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(s, 2),
            StationRef::new(u, 0),
            ClosureStrategy::Batch,
        )
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(s, 2),
            StationRef::new(v, 0),
            ClosureStrategy::Batch,
        )
        .unwrap();
    (project, s, t, u, v)
}

/// Assert every recorded merge is within the invariant, the way
/// `debug_assert_merge_alignment` does - but unconditionally, so the check is
/// part of what this test proves rather than a debug-build side effect.
fn assert_every_merge_is_aligned(project: &Project, context: &str) {
    for merge in &project.station_merges {
        let (Some(a), Some(b)) = (
            project.resolve_key(&merge.station_a),
            project.resolve_key(&merge.station_b),
        ) else {
            panic!("{context}: merge with a dangling endpoint: {merge:?}");
        };
        let (Some((ax, ay)), Some((bx, by))) = (
            project.compute_station_raw_xy(a),
            project.compute_station_raw_xy(b),
        ) else {
            panic!("{context}: merge endpoint with no position: {merge:?}");
        };
        let gap = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
        assert!(
            gap < Project::MERGE_ALIGNMENT_INVARIANT_M,
            "{context}: merge {a:?} <-> {b:?} is {gap:.6} m out of alignment"
        );
    }
}

/// #464 defect A, chained: the guard is only sound if it accumulates.
///
/// `preview_delete_shot` built its `planned` hypothesis ONCE, from
/// `replacement_merges` alone, so a promotion the closure loop itself decided on
/// was invisible to the next closure's guard. With two sole-bridge closures on
/// one doomed station, each promotion passes alone - and together they weld two
/// independently GPS-anchored components into one rigid group, the permanent,
/// unrepairable gap this branch exists to prevent.
#[test]
fn deleting_a_shot_never_promotes_two_closures_that_together_double_anchor() {
    // The premise first: each closure ALONE is a legal promotion, so what the
    // test below catches is the COMBINATION and not either closure by itself.
    for keep in 0..2usize {
        let (mut project, s, t, ..) = chained_double_anchor_closure_fixture();
        let kept = project.loop_closures[keep].id;
        project.loop_closures.retain(|c| c.id == kept);
        let last_shot = project.get_section(s).unwrap().shots.last().unwrap().id;
        let (_, impact) = project.remove_shot_with_rewire(s, last_shot).unwrap();
        assert_eq!(
            impact.closure_to_merge.len(),
            1,
            "closure {keep} alone must promote: {impact:?}"
        );
        assert_eq!(impact.closure_to_merge[0].station_a, StationRef::new(t, 0));
        assert_every_merge_is_aligned(&project, "single promotion");
        assert!(project.over_anchored_merge_components().is_empty());
    }

    let (mut project, s, t, u, v) = chained_double_anchor_closure_fixture();
    let second_closure_id = project.loop_closures[1].id;

    let last_shot = project.get_section(s).unwrap().shots.last().unwrap().id;
    let (_, impact) = project.remove_shot_with_rewire(s, last_shot).unwrap();

    // Exactly one promotion: the first closure is a legal merge, the second is
    // refused BECAUSE of it and stays a closure, re-anchored onto T0.
    assert_eq!(
        impact.closure_to_merge.len(),
        1,
        "the second promotion would double-anchor the group the first created: {impact:?}"
    );
    assert_eq!(
        (
            impact.closure_to_merge[0].station_a,
            impact.closure_to_merge[0].station_b
        ),
        (StationRef::new(t, 0), StationRef::new(u, 0))
    );
    assert_eq!(
        impact.replacement_closure_updates.len(),
        1,
        "the refused promotion must keep the closure, re-anchored: {impact:?}"
    );
    assert_eq!(
        impact.replacement_closure_updates[0].closure_id,
        second_closure_id
    );
    assert_eq!(
        impact.replacement_closure_updates[0].new_station,
        StationRef::new(t, 0)
    );

    assert_eq!(
        project.loop_closures.len(),
        1,
        "the refused closure must survive: {:?}",
        project.loop_closures
    );
    let survivor = &project.loop_closures[0];
    assert_eq!(
        (
            project.resolve_key(&survivor.station_a).unwrap(),
            project.resolve_key(&survivor.station_b).unwrap()
        ),
        (StationRef::new(t, 0), StationRef::new(v, 0))
    );

    // The merge that WAS minted paid for the coincidence it declares, and no
    // rigid group ended up holding two GPS anchors.
    assert_every_merge_is_aligned(&project, "after the chained delete");
    assert!(
        project.over_anchored_merge_components().is_empty(),
        "the delete welded two anchored components: {:?}",
        project.over_anchored_merge_components()
    );

    // ...and the refusal is STABLE: the whole-project sweep and the loader both
    // decline it for the same reason, so memory and disk agree.
    assert_eq!(project.convert_orphan_closures_to_merges(), 0);
    project.redrive_offsets_after_load();
    assert_eq!(project.loop_closures.len(), 1);
    assert_eq!(project.station_merges.len(), 1);
    assert_every_merge_is_aligned(&project, "after a reload");
    assert!(project.over_anchored_merge_components().is_empty());
}

/// A genuine loop over two sections, with a real misclosure, whose other leg is
/// about to be deleted (#464 defect C).
///
/// * section P: two 10 m shots north, GPS-anchored at P0;
/// * section Q: two 10 m shots east, merged P2 <-> Q0;
/// * a loop closure P0 <-> Q2 - a true loop (P0-P1-P2=Q0-Q1-Q2) carrying the
///   28.28 m misclosure the survey actually measured.
///
/// Returns `(project, p, q)`.
fn misclosing_loop_fixture() -> (Project, SectionId, SectionId) {
    let mut project = Project::new("Misclosure".to_string(), None);
    let p = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let q = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project
        .set_zero_datum(
            StationRef::new(p, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .merge_stations(StationRef::new(p, 2), StationRef::new(q, 0))
        .unwrap();
    let closure = project
        .create_loop_closure(
            StationRef::new(p, 0),
            StationRef::new(q, 2),
            ClosureStrategy::Batch,
        )
        .unwrap();
    assert!(
        closure.misclosure_distance > 28.0,
        "the fixture must carry a REAL misclosure, got {}",
        closure.misclosure_distance
    );
    (project, p, q)
}

/// #464 defect C: promoting an orphaned closure to a merge declares a
/// coincidence the geometry does not have. The follow-up
/// `cascade_connection_offsets(&[edited_section])` cannot pay for it - the
/// edited section is a seed and the other end holds the datum, so both are
/// authoritative and `cascade_bfs` skips the freshly minted edge. The promotion
/// itself has to translate the movable side.
#[test]
fn promoting_an_orphaned_closure_co_locates_the_side_that_may_move() {
    let (mut project, p, q) = misclosing_loop_fixture();
    let q_before = project.section_offset(q);

    // Delete Q's first shot: station Q0 goes, and with it the P2 <-> Q0 merge,
    // so the closure is the sole bridge left between P and Q.
    let first_shot = project.get_section(q).unwrap().shots[0].id;
    project.remove_shot_with_rewire(q, first_shot).unwrap();

    assert!(
        project.loop_closures.is_empty(),
        "the sole-bridge closure must be promoted: {:?}",
        project.loop_closures
    );
    assert_eq!(project.station_merges.len(), 1);
    let merge = &project.station_merges[0];
    let a = project.resolve_key(&merge.station_a).unwrap();
    let b = project.resolve_key(&merge.station_b).unwrap();
    let (ax, ay) = project.compute_station_raw_xy(a).unwrap();
    let (bx, by) = project.compute_station_raw_xy(b).unwrap();
    let gap = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
    assert!(
        gap < Project::MERGE_ALIGNMENT_INVARIANT_M,
        "the promotion left the merge {gap:.6} m out of alignment"
    );

    // The DATUM side did not move; the free side did.
    assert!(
        project.section_offset(p).0.abs() < 1e-9 && project.section_offset(p).1.abs() < 1e-9,
        "the anchored side must stay put, got {:?}",
        project.section_offset(p)
    );
    assert!(
        (project.section_offset(q).0 - q_before.0).abs() > 1.0
            || (project.section_offset(q).1 - q_before.1).abs() > 1.0,
        "the free side must have been translated onto the anchor"
    );
}

/// The same promotion with NOTHING anchored: the coincidence is still owed, and
/// the side that moves is picked deterministically (`station_a`'s group, exactly
/// as `merge_stations` picks when neither side holds a datum).
#[test]
fn promoting_an_orphaned_closure_co_locates_even_with_no_datum() {
    let (mut project, p, q) = misclosing_loop_fixture();
    project.zero_datums.clear();
    let p_before = project.section_offset(p);
    let q_before = project.section_offset(q);

    let first_shot = project.get_section(q).unwrap().shots[0].id;
    project.remove_shot_with_rewire(q, first_shot).unwrap();

    assert!(project.loop_closures.is_empty());
    assert_eq!(project.station_merges.len(), 1);
    let merge = &project.station_merges[0];
    let a = project.resolve_key(&merge.station_a).unwrap();
    let b = project.resolve_key(&merge.station_b).unwrap();
    let (ax, ay) = project.compute_station_raw_xy(a).unwrap();
    let (bx, by) = project.compute_station_raw_xy(b).unwrap();
    let gap = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
    assert!(
        gap < Project::MERGE_ALIGNMENT_INVARIANT_M,
        "the promotion left the merge {gap:.6} m out of alignment"
    );

    // WHICH side moved, not just that the gap closed: the closure was created
    // P0 <-> Q2, so `station_a` is P, and with nothing anchored P's group is the
    // one translated onto Q. Inverting the anchor/moving pick in
    // `plan_merge_promotion`'s no-datum branch fails here.
    //
    // Measured against the SAME delete with no closure to promote, because the
    // deletion itself shifts Q (it closes the gap the removed first shot left).
    // Anything beyond that baseline is the promotion's own translation.
    let (p_baseline, q_baseline) = {
        let (mut baseline, p, q) = misclosing_loop_fixture();
        baseline.zero_datums.clear();
        baseline.loop_closures.clear();
        let first_shot = baseline.get_section(q).unwrap().shots[0].id;
        baseline.remove_shot_with_rewire(q, first_shot).unwrap();
        (baseline.section_offset(p), baseline.section_offset(q))
    };
    let p_after = project.section_offset(p);
    let q_after = project.section_offset(q);
    assert!(
        (p_after.0 - p_baseline.0).abs() > 1.0 || (p_after.1 - p_baseline.1).abs() > 1.0,
        "station_a's group is the movable side: p {p_before:?} -> {p_after:?} \
         (delete alone: {p_baseline:?})"
    );
    assert_eq!(
        q_after, q_baseline,
        "the side picked as anchor must not move beyond what the delete itself does: \
         q {q_before:?} -> {q_after:?}"
    );
}
