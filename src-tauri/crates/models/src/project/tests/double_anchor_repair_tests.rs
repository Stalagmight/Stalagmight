//! Double-anchored merge groups: detection, repair demotion and the measured alignment claim
//! (#469).
//!
//! Lines 1543-2350 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

#[test]
fn convert_orphan_closures_leaves_double_anchored_groups_unconverted() {
    // Two independently GPS-anchored surveys (separate merge components) joined only
    // by a sole-bridge loop closure. Converting that closure into a rigid MERGE would
    // place two datums in one merge component and tear it on the next cascade - the
    // reported failure mode - so the conversion must leave it as a (soft) closure.
    let mut project = Project::new("DoubleAnchorBridge".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();

    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .set_zero_datum(
            StationRef::new(b, 0),
            GpsCoordinates::new(45.001, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // A closure between two anchored-but-disconnected groups is legitimate.
    project
        .create_loop_closure(
            StationRef::new(a, 1),
            StationRef::new(b, 1),
            ClosureStrategy::Batch,
        )
        .unwrap();

    let converted = project.convert_orphan_closures_to_merges();
    assert_eq!(
        converted, 0,
        "must not convert a closure that would double-anchor a merge component"
    );
    assert!(
        project.station_merges.is_empty(),
        "no merge should have been created"
    );
    assert_eq!(
        project.loop_closures.len(),
        1,
        "the bridge closure must be retained"
    );
    assert!(
        project.over_anchored_merge_components().is_empty(),
        "the two datums must stay in separate merge components"
    );
    // The retained bridge must NOT be flagged as an orphan-that-should-be-a-merge:
    // the checker and the converter agree via merge_would_double_anchor, so Repair
    // can't get stuck re-flagging a closure it will never convert.
    assert!(
        project
            .check_orphan_closures()
            .iter()
            .all(|v| v.category != "orphan_closure"),
        "a deliberately-retained double-anchored bridge closure must not be flagged as an orphan"
    );

    // Contrast: with only ONE side anchored, the same sole-bridge closure SHOULD
    // still convert to a merge (the original, intended behaviour).
    project.remove_zero_datum(StationRef::new(b, 0));
    let converted_after = project.convert_orphan_closures_to_merges();
    assert_eq!(
        converted_after, 1,
        "a sole-bridge closure with a single anchor still becomes a merge"
    );
    assert_eq!(project.station_merges.len(), 1);
    assert!(project.loop_closures.is_empty());
}

#[test]
fn detects_two_datums_in_one_merge_component() {
    // The "≤1 datum per merge component" invariant must be DETECTABLE after the
    // fact, so a file written by an older build (or any unguarded path) is flagged
    // rather than silently mis-solved. Force the violation by pushing datums
    // directly, then assert both the detector and the integrity pass catch it.
    let mut project = Project::new("OverAnchored".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });

    let over = project.over_anchored_merge_components();
    assert_eq!(over.len(), 1, "both datums share one merge component");
    assert_eq!(over[0].len(), 2);

    assert!(
        project
            .check_structural_integrity()
            .iter()
            .any(|v| v.category == "multiple_datums_per_merge_component"),
        "the integrity pass must flag the over-anchored component"
    );

    // Removing one datum restores the invariant.
    assert!(project.remove_zero_datum(StationRef::new(b, 0)));
    assert!(project.over_anchored_merge_components().is_empty());
}

/// The damage a past bug left in saved files: two independently GPS-anchored
/// merge components (A+B, and C) welded together by a RIGID merge whose two
/// endpoints sit 40 m apart. `resolve_offsets` can never satisfy it - each half
/// is pinned to its own datum - so it stays torn open forever.
///
/// Fully synthetic: three 2-shot sections running due north, placed by hand.
///   A:0 (0,0)  A:1 (0,10)  A:2 (0,20)   <- datum on A:0
///   B:0 (0,20) B:1 (0,30)  B:2 (0,40)   <- merged A:2 = B:0, correctly aligned
///   C:0 (0,0)  C:1 (0,10)  C:2 (0,20)   <- datum on C:0
/// plus the bogus merge B:2 <-> C:0, i.e. (0,40) welded to (0,0).
fn double_anchored_bridge_project() -> (Project, SectionId, SectionId, SectionId) {
    let mut project = Project::new("DoubleAnchoredBridge".to_string(), None);
    let add = |p: &mut Project| {
        p.add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id
    };
    let (a, b, c) = (add(&mut project), add(&mut project), add(&mut project));
    project.ensure_shot_uuids();

    // The legitimate, correctly aligned merge inside the first component.
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.set_section_offset(b, (0.0, 20.0));

    // One datum per component - legal on its own, which is why the interactive
    // guards never fired.
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(c, 0).into(),
        gps: GpsCoordinates::new(45.001, -90.0, 0.0).unwrap(),
    });

    // The bogus promotion: what should have stayed a loop closure.
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 2).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    (project, a, b, c)
}

#[test]
fn repair_demotes_a_double_anchoring_bridge_merge() {
    let (mut project, _a, b, c) = double_anchored_bridge_project();

    // Precondition: this is exactly the pair of symptoms a damaged file shows,
    // and re-solving offsets cannot clear either of them.
    assert_eq!(project.resolve_offsets(), 0, "the tear is unsolvable");
    assert_eq!(
        project.check_merge_alignment().len(),
        1,
        "the bogus merge reports as misaligned"
    );
    assert_eq!(
        project.over_anchored_merge_components().len(),
        1,
        "both datums sit in one merge component"
    );

    assert_eq!(project.demote_double_anchoring_merges(), 1);

    assert_eq!(
        project.station_merges.len(),
        1,
        "only the legitimate A-B merge survives"
    );
    assert!(
        !project.are_stations_merged(StationRef::new(b, 2), StationRef::new(c, 0)),
        "the over-constraining merge is gone"
    );
    assert!(
        project.are_stations_in_closure(StationRef::new(b, 2), StationRef::new(c, 0)),
        "an equivalent loop closure records the same coincidence"
    );
    let closure = project
        .loop_closures
        .last()
        .expect("the demotion pushed a closure");
    assert!(
        !closure.is_aligned,
        "an aligned closure would re-impose the constraint just removed"
    );
    assert!(closure.id > 0, "the closure gets a real monotonic id");

    // Both symptoms clear, and the solve is now achievable.
    project.resolve_offsets();
    assert!(
        project.check_merge_alignment().is_empty(),
        "no merge is left torn open"
    );
    assert!(
        project.over_anchored_merge_components().is_empty(),
        "each datum is back in its own merge component"
    );
    assert!(
        project
            .check_structural_integrity()
            .iter()
            .all(|v| v.category != "multiple_datums_per_merge_component"),
        "the over-anchoring warning is cleared"
    );
}

#[test]
fn demoted_closure_measures_the_merge_free_path() {
    // The merge being demoted is itself a zero-cost pathfinding edge between
    // the two stations being measured. Metrics taken while it still exists ride
    // that edge and persist path_length = 0 / error_percentage = 0 on the
    // replacement closure. Give the survey a real detour - soft closures
    // through a fourth section - and the demoted closure must carry the
    // detour's surveyed length instead.
    let (mut project, _a, b, c) = double_anchored_bridge_project();
    let d = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    let detour = |id: u64, from: StationRef, to: StationRef| LoopClosure {
        id,
        station_a: from.into(),
        station_b: to.into(),
        path_length: 1.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    };
    project
        .loop_closures
        .push(detour(1, StationRef::new(b, 1), StationRef::new(d, 0)));
    project
        .loop_closures
        .push(detour(2, StationRef::new(d, 2), StationRef::new(c, 1)));

    assert_eq!(project.demote_double_anchoring_merges(), 1);

    let closure = project
        .loop_closures
        .last()
        .expect("the demotion pushed a closure");
    assert!(
        closure.path_length > 0.0,
        "the metric must not ride the merge being demoted (got {})",
        closure.path_length
    );
    assert!(
        closure.error_percentage > 0.0,
        "a metres-wide misclosure over a real path is a nonzero error (got {})",
        closure.error_percentage
    );
}

#[test]
fn repair_leaves_legitimate_merges_alone() {
    // (a) A torn merge whose two halves stay connected through a HEALTHY merge.
    // A:0 and C:0 both sit at world (0,0), so that parallel merge is aligned and
    // is not going anywhere: it keeps A+B+C one rigid component whether or not
    // the torn B:2 = C:0 bridge survives. Dropping the torn merge therefore heals
    // nothing, and demotion (which exists to SEPARATE two anchored halves) must
    // leave it alone. Note the contrast with two torn merges bridging the same
    // halves - see `parallel_torn_bridges_are_all_demoted` - where neither is
    // load-bearing and both have to go.
    let (mut project, a, _b, c) = double_anchored_bridge_project();
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });
    assert_eq!(
        project.check_merge_alignment().len(),
        1,
        "only the bogus bridge is torn open; the parallel A:0 = C:0 merge measures 0 m"
    );
    assert_eq!(
        project.demote_double_anchoring_merges(),
        0,
        "a torn merge with an intact parallel merge path is redundant, not over-constraining"
    );
    assert_eq!(project.station_merges.len(), 3, "every merge is intact");
    assert!(project.loop_closures.is_empty(), "no closure was invented");

    // (b) A sole bridge whose FAR half carries no datum. That is an ordinary
    // single-anchor group with drift, which resolve_offsets can and does heal by
    // moving the unanchored half - demoting it would destroy a real connection.
    let (mut project, _a, _b, c) = double_anchored_bridge_project();
    assert!(project.remove_zero_datum(StationRef::new(c, 0)));
    assert_eq!(
        project.demote_double_anchoring_merges(),
        0,
        "a bridge to an unanchored group is a legitimate merge"
    );
    assert_eq!(project.station_merges.len(), 2, "every merge is intact");
    assert!(project.loop_closures.is_empty(), "no closure was invented");
    project.resolve_offsets();
    assert!(
        project.check_merge_alignment().is_empty(),
        "and the ordinary solver heals it, which is why demotion must not fire"
    );
}

#[test]
fn parallel_torn_bridges_are_all_demoted() {
    // Two bogus merges welding the SAME two anchored halves together. Judging a
    // candidate with only itself excluded makes them shield each other: each one
    // looks redundant because the other still joins B to C, so neither is demoted
    // and the project stays torn forever. Judging the whole torn set at once asks
    // the question that matters - are these halves welded together only by merges
    // the solver cannot satisfy? - and both go.
    //
    // B:1 is world (0,30) and C:1 is (0,10), so the second bridge is a 20 m tear
    // needing no extra geometry.
    let (mut project, _a, b, c) = double_anchored_bridge_project();
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 1).into(),
        merged_at: Utc::now(),
    });
    assert_eq!(
        project.check_merge_alignment().len(),
        2,
        "both cross-component merges are torn open"
    );

    assert_eq!(project.demote_double_anchoring_merges(), 2);

    assert_eq!(
        project.station_merges.len(),
        1,
        "only the legitimate A-B merge survives"
    );
    assert!(!project.are_stations_merged(StationRef::new(b, 2), StationRef::new(c, 0)));
    assert!(!project.are_stations_merged(StationRef::new(b, 1), StationRef::new(c, 1)));
    assert!(
        project.are_stations_in_closure(StationRef::new(b, 2), StationRef::new(c, 0)),
        "each demoted merge leaves an equivalent loop closure"
    );
    assert!(project.are_stations_in_closure(StationRef::new(b, 1), StationRef::new(c, 1)));
    assert_eq!(project.loop_closures.len(), 2);

    project.resolve_offsets();
    assert!(
        project.check_merge_alignment().is_empty(),
        "no merge is left torn open"
    );
    assert!(
        project.over_anchored_merge_components().is_empty(),
        "each datum is back in its own merge component"
    );
}

#[test]
fn demotion_softens_an_existing_aligned_closure_over_the_same_pair() {
    // The pair can already carry a loop closure, in which case the demotion
    // invents no second one. But an `is_aligned: true` closure is a RIGID edge to
    // the offset solver, so leaving it as-is would re-impose exactly the
    // over-constraint the demotion just removed, and its stored metrics were
    // measured while the merge welded the pair together. Both must be corrected in
    // place, under the closure's existing id.
    let (mut project, _a, b, c) = double_anchored_bridge_project();
    project.loop_closures.push(LoopClosure {
        id: 7,
        station_a: StationRef::new(b, 2).into(),
        station_b: StationRef::new(c, 0).into(),
        // Junk: what a measurement taken across the merge recorded.
        path_length: 123.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: true,
    });

    assert_eq!(project.demote_double_anchoring_merges(), 1);

    assert_eq!(
        project.station_merges.len(),
        1,
        "only the legitimate A-B merge survives"
    );
    assert!(!project.are_stations_merged(StationRef::new(b, 2), StationRef::new(c, 0)));
    assert_eq!(
        project.loop_closures.len(),
        1,
        "the existing closure is corrected, not duplicated"
    );
    let closure = &project.loop_closures[0];
    assert_eq!(closure.id, 7, "it keeps its id - same closure, corrected");
    assert!(
        !closure.is_aligned,
        "an aligned closure would re-impose the constraint just removed"
    );
    assert!(
        closure.misclosure_distance > 0.0,
        "the metrics are restamped from the merge-free measurement (got {})",
        closure.misclosure_distance
    );
    assert!(closure.horizontal_misclosure > 0.0);
    // path_length is restamped too. The measurement excludes the surviving
    // closure itself - left in, it is a zero-cost shortcut over exactly this
    // pair and the answer would be 0. With it excluded the halves have no
    // other link, so Dijkstra finds no path and the documented fallback (sum
    // of the reachable sections' lengths: A + B = 40 m) kicks in. 123 -> 40
    // therefore proves both the restamp and the exclusion; the detour variant
    // below proves a real route is used when one exists.
    assert_eq!(closure.path_length, 40.0);

    assert!(
        project.check_closure_alignment().is_empty(),
        "and the softened closure is no longer a stale aligned one"
    );
    project.resolve_offsets();
    assert!(
        project.over_anchored_merge_components().is_empty(),
        "each datum is back in its own merge component"
    );
}

#[test]
fn softened_closure_measures_the_closure_free_path() {
    // Companion to `demoted_closure_measures_the_merge_free_path`, for the
    // already-closed branch: the surviving closure is itself a zero-cost
    // pathfinding edge over exactly the pair being measured, so measuring with
    // it still in the list would stamp path_length = 0 / error 0% no matter how
    // long the real detour is. The demotion must pull the direct closure out,
    // measure, and restamp it - here the detour through section D is the only
    // real path, so a nonzero path_length proves the exclusion happened.
    let (mut project, _a, b, c) = double_anchored_bridge_project();
    let d = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    let detour = |id: u64, from: StationRef, to: StationRef| LoopClosure {
        id,
        station_a: from.into(),
        station_b: to.into(),
        path_length: 1.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    };
    project
        .loop_closures
        .push(detour(1, StationRef::new(b, 1), StationRef::new(d, 0)));
    project
        .loop_closures
        .push(detour(2, StationRef::new(d, 2), StationRef::new(c, 1)));
    // The pre-existing closure on the demoted pair itself, with junk metrics a
    // merge-era measurement would have recorded.
    let mut direct = detour(7, StationRef::new(b, 2), StationRef::new(c, 0));
    direct.is_aligned = true;
    project.loop_closures.push(direct);

    assert_eq!(project.demote_double_anchoring_merges(), 1);

    assert_eq!(
        project.loop_closures.len(),
        3,
        "the existing closure is corrected, not duplicated"
    );
    let closure = project
        .loop_closures
        .iter()
        .find(|lc| lc.id == 7)
        .expect("the direct closure survives under its own id");
    assert!(!closure.is_aligned);
    assert!(
        closure.path_length > 0.0,
        "the metric must not ride the closure being softened (got {})",
        closure.path_length
    );
    assert!(
        closure.error_percentage > 0.0,
        "a metres-wide misclosure over a real path is a nonzero error (got {})",
        closure.error_percentage
    );
}

#[test]
fn unmeasurable_geometry_is_never_demoted_or_flagged() {
    // Non-finite (NaN/inf) shot geometry makes every distance derived from it
    // NaN, and `NaN < TOL` and `NaN >= TOL` are BOTH false. Left unguarded, the
    // three consumers of `endpoint_gap` would each answer by accident: the two
    // alignment checks would call the pair aligned, and the demotion would call
    // it not-torn. `endpoint_gap` returning `None` makes that one deliberate
    // decision - unmeasurable means hands off - so the poisoned project is
    // described by the `non_finite_geometry` violation and by nothing else.
    let (mut project, a, b, c) = double_anchored_bridge_project();
    // Poison ONLY the far end of the bogus bridge: section B's second shot puts
    // B:2 at NaN while B:0 and B:1 stay finite, so the legitimate A:2 = B:0 merge
    // is still measurable and this is a test about the unmeasurable pair alone.
    project.get_section_mut(b).unwrap().shots[1].length = f64::NAN;
    // The third consumer needs an ALIGNED closure over a poisoned pair.
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(b, 2).into(),
        station_b: StationRef::new(c, 1).into(),
        path_length: 10.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: true,
    });

    assert!(
        project
            .check_structural_integrity()
            .iter()
            .any(|v| v.category == "non_finite_geometry"),
        "the poison itself is what gets reported"
    );
    assert!(
        project.check_merge_alignment().is_empty(),
        "no merge is reported: A:2 = B:0 measures 0 m, and B:2 = C:0 cannot be measured at all"
    );
    assert!(
        project.check_closure_alignment().is_empty(),
        "broken geometry is not evidence that an aligned closure has drifted open"
    );
    assert_eq!(
        project.demote_double_anchoring_merges(),
        0,
        "and no merge is deleted on the strength of a distance that does not exist"
    );
    assert_eq!(project.station_merges.len(), 2, "both merges are intact");
    assert!(project.are_stations_merged(StationRef::new(a, 2), StationRef::new(b, 0)));
    assert!(project.are_stations_merged(StationRef::new(b, 2), StationRef::new(c, 0)));
}

#[test]
fn demoted_closure_is_not_re_promoted_by_orphan_conversion() {
    // The demotion and `convert_orphan_closures_to_merges` run in the same Repair
    // pass, and the converter's job is turning sole-bridge closures INTO merges -
    // exactly what was just undone. Its double-anchor guard must see the same two
    // anchored halves and leave the demoted closure alone, or Repair would
    // ping-pong the same edge forever.
    let (mut project, _a, _b, _c) = double_anchored_bridge_project();
    assert_eq!(project.demote_double_anchoring_merges(), 1);

    assert_eq!(
        project.convert_orphan_closures_to_merges(),
        0,
        "the converter must not re-promote the closure it was just handed"
    );
    assert_eq!(project.loop_closures.len(), 1, "the closure survives");
    assert_eq!(project.station_merges.len(), 1, "no merge was recreated");
    assert!(
        project
            .check_orphan_closures()
            .iter()
            .all(|v| v.category != "orphan_closure"),
        "and it is not flagged as an orphan Repair can never convert"
    );

    // Idempotent: a second Repair finds nothing left to demote.
    assert_eq!(project.demote_double_anchoring_merges(), 0);
    assert_eq!(project.loop_closures.len(), 1);
    assert_eq!(project.station_merges.len(), 1);
}

// ===========================================================================
// #469 item 2: `is_aligned` is a MEASUREMENT
// ===========================================================================

#[test]
fn a_closure_is_born_with_a_measured_claim() {
    // The MODEL API, which the importers and the project merge call directly and
    // no command epilogue follows. Two laps of a 10 m square, so stations 0, 4
    // and 8 are all one physical point and station 2 is the far corner.
    let mut project = Project::new("Square".to_string(), None);
    let lap = [0.0, 90.0, 180.0, 270.0];
    let s = project
        .add_section(SectionData {
            shots: lap
                .iter()
                .chain(lap.iter())
                .map(|&azimuth| shot(azimuth, azimuth, 10.0, 0.0, 0.0))
                .collect(),
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();

    let closure = |project: &mut Project, from: u32, to: u32, strategy| {
        project
            .create_loop_closure(StationRef::new(s, from), StationRef::new(s, to), strategy)
            .expect("create_loop_closure failed")
            .is_aligned
    };

    assert!(
        closure(&mut project, 0, 4, ClosureStrategy::Move),
        "the lap closes on itself, so the coincidence claim is TRUE"
    );
    assert!(
        !closure(&mut project, 0, 2, ClosureStrategy::Move),
        "the diagonal is 14 m of open loop - a claim the app measured the \
         refutation of one line earlier must not be stamped"
    );
    assert!(
        !closure(&mut project, 4, 8, ClosureStrategy::Batch),
        "and a Batch closure RECORDS a misclosure: coincident or not, it is the \
         soft edge the non-destructive solve relaxes"
    );
    assert_eq!(
        project
            .loop_closures
            .iter()
            .map(|lc| lc.is_aligned)
            .collect::<Vec<_>>(),
        vec![true, false, false],
        "the recorded closures carry what was measured"
    );
}

#[test]
fn the_re_measure_never_re_promotes_what_repair_demoted() {
    // Repair's demotion and the per-command re-measure both run over the same
    // project, and the re-measure is the one that can set `is_aligned` back to
    // TRUE. If it went on geometry alone, the two would fight: Repair softens the
    // edge precisely so the two anchored halves stop being one rigid component,
    // and the next geometry command would rigidify it again the moment anything
    // pulled the endpoints together. The double-anchor term is what makes that
    // impossible, and it is asked of the MERGE graph, so no closure flag can
    // change the answer.
    let (mut project, _a, b, c) = double_anchored_bridge_project();
    // The pair already carries the one kind of closure the re-measure governs.
    project.loop_closures.push(LoopClosure {
        id: 7,
        station_a: StationRef::new(b, 2).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 123.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Move,
        is_aligned: true,
    });

    assert_eq!(project.demote_double_anchoring_merges(), 1);
    assert!(
        !project.loop_closures[0].is_aligned,
        "setup: Repair softens the closure over the demoted pair"
    );

    // What Repair itself does next, in order.
    project.resolve_offsets();
    assert_eq!(
        project.remeasure_closure_alignment(),
        0,
        "the re-measure must not re-impose the constraint Repair removed"
    );

    // And now the part geometry alone would get wrong: slide C's rigid group so
    // the two endpoints are exactly one point. The claim is now TRUE and still
    // must not be made, because the edge would weld two independently
    // GPS-anchored merge components into one - the #464 refusal.
    project.adjust_rigid_group_offsets(c, 0.0, 40.0);
    let (local, station_offsets) = (
        project.section_local_coords(),
        project.resolved_station_offsets(),
    );
    let gap = project
        .endpoint_gap(
            &StationRef::new(b, 2).into(),
            &StationRef::new(c, 0).into(),
            &local,
            &station_offsets,
        )
        .expect("both endpoints resolve")
        .2;
    assert!(
        gap < Project::MERGE_ALIGNMENT_INVARIANT_M,
        "setup: the endpoints are coincident now (gap {gap})"
    );
    assert_eq!(
        project.remeasure_closure_alignment(),
        0,
        "a coincident pair bridging two anchored groups is still not alignable"
    );
    assert!(!project.loop_closures[0].is_aligned);
}

#[test]
fn the_re_measure_leaves_a_legacy_baked_alignment_alone() {
    // The 0.5.0 migration promise (docs/closures.md): a project whose shots were
    // baked by the removed destructive "Adjust Closures" action opens looking
    // identical and is never rewritten again. Those closures carry `is_aligned`
    // with a NON-`Move` strategy, and their shots were rewritten to make the
    // claim true - so demoting them would silently change co-location, station
    // depth averaging, move-tool rigidity and which shots are frozen on a file
    // that is exactly what it says it is. The re-measure governs `Move` closures
    // and nothing else.
    //
    // The far end of the pair here is 40 m out, i.e. the worst case: even a
    // legacy claim the app can SEE is false is left as recorded, because the app
    // cannot know what the bake did to the shots and `check_closure_alignment`
    // is the honest surface for it.
    let (mut project, _a, b, c) = double_anchored_bridge_project();
    for (id, strategy, is_aligned) in [
        (1_u64, ClosureStrategy::Batch, true),
        (2, ClosureStrategy::LeastSquaresWeighted, true),
        // A modern snap-to-merge closure alongside them, so the sweep genuinely
        // runs (it short-circuits on a project holding no `Move` closure at all)
        // and the exemption below is the thing being measured.
        (3, ClosureStrategy::Move, false),
    ] {
        project.loop_closures.push(LoopClosure {
            id,
            station_a: StationRef::new(b, 2).into(),
            station_b: StationRef::new(c, 1).into(),
            path_length: 10.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy,
            is_aligned,
        });
    }
    let legacy_claims = |project: &Project| {
        project
            .loop_closures
            .iter()
            .filter(|lc| lc.strategy != ClosureStrategy::Move)
            .map(|lc| lc.is_aligned)
            .collect::<Vec<_>>()
    };

    assert_eq!(project.remeasure_closure_alignment(), 0);
    assert_eq!(legacy_claims(&project), vec![true, true]);
    // Including through the LOAD, which is where the force-restamp used to live.
    project.assign_closure_ids();
    assert_eq!(legacy_claims(&project), vec![true, true]);
    assert_eq!(
        project.check_closure_alignment().len(),
        2,
        "a torn legacy claim is still REPORTED - detect-only is the contract"
    );
}
