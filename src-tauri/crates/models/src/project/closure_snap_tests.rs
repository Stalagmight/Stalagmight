//! A snapped loop closure in the Closures Computed View (#736): a `Move`
//! closure the Raw Data View holds rigid (`is_aligned`, see
//! `Project::closure_alignment_stamp`) is one point in the least-squares
//! solve too, as a merge is, so the view (and Plan and Export) never pulls a
//! snapped pair apart.

use std::collections::{HashMap, HashSet};

use super::closure_hand_move_tests::{assert_view_is_the_solve, plant as plant_hand};
use super::combine_closure_tests::{add, dist, shot, xy};
use crate::config::ClosureAdjustmentConfig;
use crate::{
    local_to_gps, ClosureStrategy, DatumRole, GpsCoordinates, LoopClosure, Project, SectionId,
    ShotData, Station, StationRef, ZeroDatum,
};

/// The parts of `project` the Closures Computed View solves without failing:
/// every section of each.
fn solved_sections(project: &Project, failed: &HashSet<SectionId>) -> HashSet<SectionId> {
    let mut solved = HashSet::new();
    for anchor in project.adjustment_anchors_for_tests() {
        let component = project.get_connected_section_ids(anchor.section_id);
        if !component.iter().any(|id| failed.contains(id)) {
            solved.extend(component);
        }
    }
    solved
}

/// Every snap the solve ties (an aligned closure it does not take as an
/// eligible one, and does not skip as across one shot) in a part the view
/// solves without failing is shown on ONE point in the Closures Computed
/// View, to 1e-6 m: the tolerance the solve's own section realignment holds
/// the stations of one group to ("inconsistent section translation"), far
/// above the float dust the shot update leaves. Returns how many snapped
/// pairs it checked.
pub(super) fn assert_snaps_held(project: &Project, what: &str) -> usize {
    let adjusted = project.compute_adjusted_stations(&ClosureAdjustmentConfig::default());
    let view: HashMap<StationRef, (f64, f64)> = adjusted
        .stations
        .iter()
        .map(|s| (s.station_ref, (s.coordinates.x, s.coordinates.y)))
        .collect();
    let failed: HashSet<SectionId> = adjusted
        .failures
        .iter()
        .flat_map(|f| f.section_ids.iter().copied())
        .collect();
    let solved = solved_sections(project, &failed);
    let skipped: HashSet<u64> = project
        .closures_across_one_shot()
        .into_iter()
        .map(|c| c.closure_id)
        .collect();
    let mut held = 0;
    for closure in &project.loop_closures {
        if !Project::closure_is_snap(closure.strategy, closure.is_aligned)
            || skipped.contains(&closure.id)
        {
            continue;
        }
        let (Some(a), Some(b)) = (
            project.resolve_key(&closure.station_a),
            project.resolve_key(&closure.station_b),
        ) else {
            continue;
        };
        if !(solved.contains(&a.section_id) && solved.contains(&b.section_id)) {
            continue;
        }
        let apart = dist(view[&a], view[&b]);
        assert!(
            apart < 1e-6,
            "{what}: the snapped pair {a:?} {b:?} (closure {}) is {apart:.6} m apart in the \
             Closures Computed View",
            closure.id
        );
        held += 1;
    }
    held
}

/// The horizontal distance between `a` and `b` in `stations`.
fn gap_in(stations: &[Station], a: StationRef, b: StationRef) -> f64 {
    let at = |r: StationRef| {
        let s = stations.iter().find(|s| s.station_ref == r).unwrap();
        (s.coordinates.x, s.coordinates.y)
    };
    let (p, q) = (at(a), at(b));
    (p.0 - q.0).hypot(p.1 - q.1)
}

/// The issue's survey: a square A, B, C, D of one 10 m shot each, merged end
/// to start, D's end snapped onto A's start (a `Move` closure, aligned), and
/// a diagonal E from A's end that misses C's end by 0.5 m, closed there by a
/// `Batch` closure.
#[test]
fn adjusted_view_keeps_a_snapped_pair_together() {
    let mut p = Project::new("F1".to_string(), None);
    let a = add(&mut p, "A", vec![shot(90.0, 10.0)]);
    let b = add(&mut p, "B", vec![shot(0.0, 10.0)]);
    let c = add(&mut p, "C", vec![shot(270.0, 10.0)]);
    let d = add(&mut p, "D", vec![shot(180.0, 10.0)]);
    let e = add(&mut p, "E", vec![shot(315.0, 200f64.sqrt() + 0.5)]);
    for (x, y) in [
        ((a, 1), (b, 0)),
        ((b, 1), (c, 0)),
        ((c, 1), (d, 0)),
        ((a, 1), (e, 0)),
    ] {
        p.merge_stations(StationRef::new(x.0, x.1), StationRef::new(y.0, y.1))
            .unwrap();
    }
    let (d1, a0) = (StationRef::new(d, 1), StationRef::new(a, 0));
    let snapped = p
        .create_loop_closure(d1, a0, ClosureStrategy::Move)
        .unwrap();
    assert!(snapped.is_aligned);
    p.create_loop_closure(
        StationRef::new(e, 1),
        StationRef::new(c, 1),
        ClosureStrategy::Batch,
    )
    .unwrap();
    let measured = gap_in(&p.compute_station_coordinates(), d1, a0);
    let adjusted = gap_in(
        &p.compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default()),
        d1,
        a0,
    );
    assert!(measured < 1e-6);
    assert!(
        adjusted < 0.01,
        "the closure-adjusted view pulled the snapped pair {adjusted:.3} m apart"
    );
}

fn st(section: SectionId, index: u32) -> StationRef {
    StationRef::new(section, index)
}

/// Every station's closure-adjusted position (the Plan and Export geometry),
/// and the sections of the parts whose solve failed.
fn view_of(project: &Project) -> (HashMap<StationRef, (f64, f64)>, HashSet<SectionId>) {
    let adjusted = project.compute_adjusted_stations(&ClosureAdjustmentConfig::default());
    let view = adjusted
        .stations
        .iter()
        .map(|s| (s.station_ref, (s.coordinates.x, s.coordinates.y)))
        .collect();
    let failed = adjusted
        .failures
        .iter()
        .flat_map(|f| f.section_ids.iter().copied())
        .collect();
    (view, failed)
}

/// `project` as the solve saw it before #736: every snap still in the file,
/// but tying nothing. Clearing the claim on a copy is exactly that, for the
/// horizontal solve and its failures: `is_aligned` reaches the solve only
/// through [`Project::closure_is_snap`] (the raw view's depth co-location
/// reads it too, and the solve is horizontal).
pub(super) fn without_snaps(project: &Project) -> Project {
    let mut copy = project.clone();
    for closure in &mut copy.loop_closures {
        if Project::closure_is_snap(closure.strategy, closure.is_aligned) {
            closure.is_aligned = false;
        }
    }
    copy
}

/// The issue's square with D's end `short` metres short of A's start, snapped
/// there by a `Move` closure (aligned while `short` is under 1 cm), and the
/// diagonal E closed 0.5 m off C's end by a `Batch` closure. Returns the
/// sections A, B, C, D, E.
fn square(short: f64) -> (Project, [SectionId; 5]) {
    let mut p = Project::new("Square".to_string(), None);
    let a = add(&mut p, "A", vec![shot(90.0, 10.0)]);
    let b = add(&mut p, "B", vec![shot(0.0, 10.0)]);
    let c = add(&mut p, "C", vec![shot(270.0, 10.0)]);
    let d = add(&mut p, "D", vec![shot(180.0, 10.0 - short)]);
    let e = add(&mut p, "E", vec![shot(315.0, 200f64.sqrt() + 0.5)]);
    for (x, y) in [
        ((a, 1), (b, 0)),
        ((b, 1), (c, 0)),
        ((c, 1), (d, 0)),
        ((a, 1), (e, 0)),
    ] {
        p.merge_stations(st(x.0, x.1), st(y.0, y.1)).unwrap();
    }
    p.create_loop_closure(st(d, 1), st(a, 0), ClosureStrategy::Move)
        .unwrap();
    p.create_loop_closure(st(e, 1), st(c, 1), ClosureStrategy::Batch)
        .unwrap();
    (p, [a, b, c, d, e])
}

/// A snap holds its two stations on one point whatever sub-centimetre gap it
/// carries (#736, decision a): the solve unknown is the GROUP as shown, as
/// for a merge, so the gap is closed by the shots around the loop like any
/// misclosure. The stamp's 1 cm bound is what makes a group's members close
/// enough to be one point; past it the closure is not aligned and the solve
/// ignores it as before.
#[test]
fn a_snap_with_a_sub_centimetre_gap_is_shown_on_one_point() {
    for short in [0.0, 1e-4, 0.006, 0.0099] {
        let (p, [a, _, _, d, _]) = square(short);
        let snap = &p.loop_closures[0];
        assert!(snap.is_aligned, "{short}: setup");
        let measured = dist(xy(&p, st(d, 1)), xy(&p, st(a, 0)));
        assert!((measured - short).abs() < 1e-9, "{short}: setup {measured}");
        let (view, failed) = view_of(&p);
        assert!(failed.is_empty(), "{short}: {failed:?}");
        let apart = dist(view[&st(d, 1)], view[&st(a, 0)]);
        assert!(apart < 1e-6, "{short}: the snapped pair is {apart} m apart");
        assert_eq!(assert_snaps_held(&p, &format!("{short}")), 1);
        assert_view_is_the_solve(&p, &format!("{short}"));
        // Before #736 the solve pulled the pair apart.
        let (was, _) = view_of(&without_snaps(&p));
        assert!(dist(was[&st(d, 1)], was[&st(a, 0)]) > 0.2, "{short}");
    }
    // 1 cm out is not a snap: the stamp leaves it a soft record the solve
    // ignores, and the view is exactly what it was.
    let (p, _) = square(0.0101);
    assert!(!p.loop_closures[0].is_aligned);
    let (now, _) = view_of(&p);
    let (was, _) = view_of(&without_snaps(&p));
    assert_eq!(now, was);
}

/// A snap is nothing to solve on its own: a part whose only closures are
/// snaps is shown as measured, as before #736.
#[test]
fn a_part_with_only_snaps_is_shown_as_measured() {
    let (mut p, [_, _, _, _, e]) = square(0.004);
    let batch = p.loop_closures[1].id;
    p.loop_closures.retain(|c| c.id != batch);
    assert!(p.adjustment_anchors_for_tests().is_empty());
    let (view, _) = view_of(&p);
    for (at, shown) in &view {
        assert!(dist(*shown, xy(&p, *at)) < 1e-12, "{at:?} moved");
    }
    assert!(view.contains_key(&st(e, 1)));
}

/// A section with a near-vertical shot (4 mm across, 4 m down) on a loop
/// closed by a `Batch` closure, its two ends snapped (#736, decision c): a
/// snap across one shot. Creation refuses it; one a file carries is skipped
/// and reported like any closure across one shot (#737), instead of being
/// tied and failing the whole part with "inconsistent section translation"
/// (the solve would keep that shot at its measured 4 mm and put both its
/// ends on one point).
#[test]
fn a_snap_across_a_near_vertical_shot_is_skipped_and_reported() {
    let mut p = Project::new("Shaft".to_string(), None);
    let v = add(
        &mut p,
        "V",
        vec![
            shot(90.0, 10.0),
            ShotData {
                depth_in: 0.0,
                depth_out: 4.0,
                ..shot(0.0, (16.0f64 + 0.004 * 0.004).sqrt())
            },
            ShotData {
                depth_in: 4.0,
                depth_out: 4.0,
                ..shot(0.0, 10.0)
            },
        ],
    );
    let w = add(
        &mut p,
        "W",
        vec![
            ShotData {
                depth_in: 4.0,
                depth_out: 4.0,
                ..shot(270.0, 10.0)
            },
            ShotData {
                depth_in: 4.0,
                depth_out: 0.0,
                ..shot(180.0, 9.7)
            },
        ],
    );
    let at = xy(&p, st(v, 3));
    p.set_section_offset(w, at);
    p.merge_stations(st(v, 3), st(w, 0)).unwrap();
    p.create_loop_closure(st(w, 2), st(v, 0), ClosureStrategy::Batch)
        .unwrap();
    let refused = p.create_loop_closure(st(v, 1), st(v, 2), ClosureStrategy::Move);
    assert!(refused.is_err(), "{refused:?}");
    let snap = plant(&mut p, st(v, 1), st(v, 2), ClosureStrategy::Move, false);
    assert_eq!(p.remeasure_closure_alignment(), 1);
    assert!(p.loop_closures.iter().any(|c| c.id == snap && c.is_aligned));
    let skipped = p.closures_across_one_shot();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert_eq!(
        (skipped[0].closure_id, skipped[0].collapsed_shot),
        (snap, (v, 1))
    );
    let (view, failed) = view_of(&p);
    assert!(failed.is_empty(), "{failed:?}");
    assert!(dist(view[&st(w, 2)], view[&st(v, 0)]) < 1e-6);
    assert_view_is_the_solve(&p, "shaft");
}

/// A `Batch` closure that, with a snap, would squeeze a shot (#736, decision
/// c): D's end is snapped onto A's start, so a closure from D's end to A's
/// end ties the two ends of A's one shot. Creation now refuses it; one a file
/// carries (created before #736, when snaps tied nothing) is skipped and
/// reported, and the snap holds. Before #736 the solve took it and pulled
/// D's end 10 m off A's start in the view.
#[test]
fn a_closure_squeezing_a_shot_through_a_snap_is_refused_and_skipped() {
    let (mut p, [a, _, _, d, _]) = square(0.0);
    let refused = p.create_loop_closure(st(d, 1), st(a, 1), ClosureStrategy::Batch);
    assert_eq!(
        refused.unwrap_err(),
        super::closure_shots::closure_across_one_shot_refusal((a, 0))
    );
    let squeeze = plant(&mut p, st(d, 1), st(a, 1), ClosureStrategy::Batch, false);
    let skipped = p.closures_across_one_shot();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert_eq!(
        (skipped[0].closure_id, skipped[0].collapsed_shot),
        (squeeze, (a, 0))
    );
    let (view, failed) = view_of(&p);
    assert!(failed.is_empty(), "{failed:?}");
    assert!(dist(view[&st(d, 1)], view[&st(a, 0)]) < 1e-6);
    assert_view_is_the_solve(&p, "squeeze");
    let (was, _) = view_of(&without_snaps(&p));
    assert!(dist(was[&st(d, 1)], was[&st(a, 0)]) > 9.0);
}

/// A section whose end is snapped onto its own start (#736, decision c): the
/// two ends of one section, on a loop of their own. The snap closes the
/// loop, so every shot of it is adjusted and the two ends stay on one point;
/// before #736 the loop was a spur and its shots kept their vectors while
/// the shot it shares with a solved loop changed, tearing the pair.
#[test]
fn a_section_snapped_onto_its_own_start_stays_closed() {
    let mut p = Project::new("Ring".to_string(), None);
    let s = add(
        &mut p,
        "S",
        vec![
            shot(90.0, 10.0),
            shot(0.0, 10.0),
            shot(270.0, 10.0),
            shot(180.0, 9.996),
        ],
    );
    let snap = p
        .create_loop_closure(st(s, 4), st(s, 0), ClosureStrategy::Move)
        .unwrap();
    assert!(snap.is_aligned);
    let t = add(
        &mut p,
        "T",
        vec![shot(90.0, 10.0), shot(180.0, 10.0), shot(270.0, 9.5)],
    );
    let at = xy(&p, st(s, 2));
    p.set_section_offset(t, at);
    p.merge_stations(st(s, 2), st(t, 0)).unwrap();
    p.create_loop_closure(st(t, 3), st(s, 1), ClosureStrategy::Batch)
        .unwrap();
    let (view, failed) = view_of(&p);
    assert!(failed.is_empty(), "{failed:?}");
    assert!(dist(view[&st(s, 4)], view[&st(s, 0)]) < 1e-6);
    assert_view_is_the_solve(&p, "ring");
    let (was, _) = view_of(&without_snaps(&p));
    assert!(dist(was[&st(s, 4)], was[&st(s, 0)]) > 0.01);
}

/// The issue's variant (#736, decision d): a `Batch` loop whose only cycle
/// runs through a snap. P and Q are tied only by the two closures (a state
/// built directly; the next orphan sweep would make one a merge). Before
/// #736 the snap was no edge of the spur graph, so the loop was a spur and
/// kept its 0.3 m; now it is adjusted and the snap holds.
#[test]
fn a_batch_loop_closed_only_through_a_snap_is_adjusted() {
    let mut p = Project::new("Through".to_string(), None);
    let pp = add(&mut p, "P", vec![shot(90.0, 10.0)]);
    let q = add(
        &mut p,
        "Q",
        vec![shot(0.0, 10.0), shot(270.0, 10.0), shot(180.0, 9.7)],
    );
    let at = xy(&p, st(pp, 1));
    p.set_section_offset(q, at);
    plant(&mut p, st(pp, 1), st(q, 0), ClosureStrategy::Move, true);
    plant(&mut p, st(q, 3), st(pp, 0), ClosureStrategy::Batch, false);
    let (view, failed) = view_of(&p);
    assert!(failed.is_empty(), "{failed:?}");
    assert!(dist(view[&st(q, 3)], view[&st(pp, 0)]) < 1e-6);
    assert!(dist(view[&st(pp, 1)], view[&st(q, 0)]) < 1e-6);
    assert_view_is_the_solve(&p, "through");
    let (was, _) = view_of(&without_snaps(&p));
    assert!((dist(was[&st(q, 3)], was[&st(pp, 0)]) - 0.3).abs() < 1e-6);
}

/// A section that hangs off the survey by a snap alone (a state built
/// directly, like the variant above): all of it is spur, so the solve moves
/// it as a block through its attachment point, and a snap is one as a merge
/// is (#736). Before, nothing attached it, it stayed where it was measured,
/// and the point it is snapped to moved away with its loop's correction.
#[test]
fn a_section_hanging_off_a_snap_alone_follows_it() {
    let (mut p, [_, b, _, _, _]) = square(0.0);
    let x = add(&mut p, "X", vec![shot(30.0, 6.0), shot(60.0, 4.0)]);
    let at = xy(&p, st(b, 1));
    p.set_section_offset(x, at);
    plant(&mut p, st(x, 0), st(b, 1), ClosureStrategy::Move, true);
    let (view, failed) = view_of(&p);
    assert!(failed.is_empty(), "{failed:?}");
    assert!(dist(view[&st(x, 0)], view[&st(b, 1)]) < 1e-6);
    // As a block: X's own shots are untouched.
    let moved = |i: u32| {
        let (v, m) = (view[&st(x, i)], xy(&p, st(x, i)));
        (v.0 - m.0, v.1 - m.1)
    };
    assert!(dist(moved(0), moved(2)) < 1e-9);
    assert_view_is_the_solve(&p, "hanging");
    let (was, _) = view_of(&without_snaps(&p));
    assert!(dist(was[&st(x, 0)], was[&st(b, 1)]) > 0.05);
}

/// A legacy aligned closure of a strategy the solve does not take (#736,
/// decision b). No version wrote one (`create_loop_closure` stamps only
/// `Move` closures, and the destructive adjust before 0.5.0 marked only the
/// `Batch` / `LeastSquaresWeighted` closures it solved, which are tied
/// already), but the Raw Data View holds any aligned closure rigid, so the
/// solve ties it as a snap too: one definition of a rigid closure.
#[test]
fn a_legacy_aligned_closure_of_another_strategy_is_tied_as_a_snap() {
    let (mut p, [a, _, _, d, _]) = square(0.0);
    p.loop_closures[0].strategy = ClosureStrategy::ProportionalByLength;
    assert!(p.loop_closures[0].is_aligned);
    let (view, failed) = view_of(&p);
    assert!(failed.is_empty(), "{failed:?}");
    assert!(dist(view[&st(d, 1)], view[&st(a, 0)]) < 1e-6);
    // The same pair under `Move` solves to the same view.
    let (snap, _) = square(0.0);
    assert_eq!(view, view_of(&snap).0);
}

/// A placement zero datum on a snapped station, either end (#736, decision
/// e): the group is held at the datum's raw position, so its partner sits
/// there too.
#[test]
fn a_placement_datum_on_a_snapped_station_holds_its_partner_there() {
    for on_the_snapped_end in [false, true] {
        let (mut p, [a, _, _, d, _]) = square(0.003);
        let held = if on_the_snapped_end {
            st(d, 1)
        } else {
            st(a, 0)
        };
        let other = if on_the_snapped_end {
            st(a, 0)
        } else {
            st(d, 1)
        };
        p.set_zero_datum(held, origin()).unwrap();
        assert!(p.loop_closures[0].is_aligned, "setup");
        let (view, failed) = view_of(&p);
        assert!(failed.is_empty(), "{failed:?}");
        assert!(dist(view[&held], xy(&p, held)) < 1e-9, "{held:?} moved");
        assert!(dist(view[&other], view[&held]) < 1e-6);
        assert_view_is_the_solve(&p, &format!("datum on {held:?}"));
    }
}

/// A GPS fix (#741) on a snapped station (#736, decision e): placed at B's
/// end, the fix on D's end, 1.5 m off. A hard fix holds the pair on its GPS;
/// a soft one pulls the pair, as one point, toward it.
#[test]
fn a_gps_fix_on_a_snapped_station_moves_the_pair_as_one() {
    for accuracy_m in [None, Some(2.0)] {
        let (mut p, [a, b, _, d, _]) = square(0.0);
        p.set_zero_datum(st(b, 1), origin()).unwrap();
        let shown = xy(&p, st(d, 1));
        let gps = (shown.0 + 1.2, shown.1 - 0.9);
        let (lat, lon) = local_to_gps(gps.0, gps.1, &origin()).unwrap();
        p.set_zero_datum_with_accuracy(
            st(d, 1),
            GpsCoordinates::new(lat, lon, 0.0).unwrap(),
            accuracy_m,
        )
        .unwrap();
        assert_eq!(
            p.datum_roles(),
            vec![Some(DatumRole::Placement), Some(DatumRole::GpsFix)]
        );
        let (view, failed) = view_of(&p);
        assert!(failed.is_empty(), "{failed:?}");
        assert!(dist(view[&st(d, 1)], view[&st(a, 0)]) < 1e-6);
        if accuracy_m.is_none() {
            assert!(dist(view[&st(d, 1)], gps) < 1e-6);
        } else {
            assert!(dist(view[&st(d, 1)], gps) < dist(shown, gps));
        }
        assert_view_is_the_solve(&p, &format!("fix {accuracy_m:?}"));
    }
}

/// A hand move (#759) on a snapped station, settled as a file opens (#736,
/// decision e): a few millimetres keeps the snap aligned, and the view keeps
/// the pair on one point AS SHOWN, the hand move kept on its station, as it
/// keeps a merge.
#[test]
fn a_hand_move_on_a_snapped_station_keeps_the_pair_on_one_point_as_shown() {
    for at_snapped_end in [false, true] {
        let (mut p, [a, _, _, d, _]) = square(0.0);
        let at = if at_snapped_end { st(d, 1) } else { st(a, 0) };
        plant_hand(&mut p, at, (0.003, -0.002, 0.0));
        assert!(p.loop_closures[0].is_aligned, "setup: 3.6 mm is a snap");
        let (view, failed) = view_of(&p);
        assert!(failed.is_empty(), "{failed:?}");
        assert!(dist(view[&st(d, 1)], view[&st(a, 0)]) < 1e-6);
        assert_view_is_the_solve(&p, &format!("hand move on {at:?}"));
    }
}

/// Two placement zero datums tied into one group through a snap (#736): D's
/// end snapped onto A's start, a datum at A's start, and a second survey
/// placed on its own whose datum station a `Batch` closure ties to D's end.
/// That closure ties the two datums together exactly as a closure between
/// them directly would, which the solve has always refused ("Multiple
/// placement zero datums in a single constraint group"): the part is shown
/// as measured and the failure is reported with its plain reason. Before
/// #736 the solve put D's end on the second datum and tore the snap by the
/// distance between the two datums.
#[test]
fn two_datums_tied_through_a_snap_are_a_reported_failure() {
    let (mut p, [a, _, _, d, _]) = square(0.0);
    p.set_zero_datum(st(a, 0), origin()).unwrap();
    let y = add(&mut p, "Y", vec![shot(45.0, 8.0)]);
    let (lat, lon) = local_to_gps(-30.0, 25.0, &origin()).unwrap();
    p.set_zero_datum(st(y, 0), GpsCoordinates::new(lat, lon, 0.0).unwrap())
        .unwrap();
    assert_eq!(p.placement_datum_refs().len(), 2, "setup");
    p.create_loop_closure(st(d, 1), st(y, 0), ClosureStrategy::Batch)
        .unwrap();
    assert!(p.loop_closures[0].is_aligned, "setup");
    let adjusted = p.compute_adjusted_stations(&ClosureAdjustmentConfig::default());
    assert_eq!(adjusted.failures.len(), 1, "{:?}", adjusted.failures);
    assert_eq!(
        adjusted.failures[0].error,
        "Multiple placement zero datums in a single constraint group"
    );
    assert!(adjusted.failures[0]
        .user_reason()
        .contains("tie two zero datums"));
    // Shown as measured.
    for s in &adjusted.stations {
        let shown = (s.coordinates.x, s.coordinates.y);
        assert!(dist(shown, xy(&p, s.station_ref)) < 1e-9);
    }
    let old = without_snaps(&p).compute_adjusted_stations(&ClosureAdjustmentConfig::default());
    assert!(old.failures.is_empty());
    assert!(gap_in(&old.stations, st(d, 1), st(a, 0)) > 30.0);
}

/// Combine Sections' model of the joined survey (`joined_sweep`) with a snap
/// its dedupe drops (#736): K's end and O's start side by side at the
/// origin, Z merged at O's far end and back to the origin, where Z's end is
/// snapped onto O's start (X). A `Batch` closure C ties O's second station to
/// Z's end, and a later `Batch` closure E ties K's end to Z's end (both as an
/// older file carries them). Joined, X and E tie the same two stations, and
/// the dedupe keeps E: the pair is tied where E stands, after C, so C joins
/// O:1 to Z's end and E then squeezes O's first shot. A model that tied the
/// pair where X stands would skip C instead and miss E; the plan removes E,
/// and X with it (it would otherwise outlive E and tie the pair where it
/// stands), and the combine leaves no closure across one shot it did not
/// list.
#[test]
fn a_combine_ties_a_pair_its_dedupe_keeps_where_the_kept_closure_stands() {
    let mut p = Project::new("Dedupe".to_string(), None);
    let k = add(&mut p, "K", vec![shot(90.0, 10.0)]);
    p.set_section_offset(k, (-10.0, 0.0));
    let o = add(&mut p, "O", vec![shot(0.0, 10.0), shot(90.0, 10.0)]);
    let z = add(&mut p, "Z", vec![shot(225.0, 200f64.sqrt())]);
    let at = xy(&p, st(o, 2));
    p.set_section_offset(z, at);
    p.merge_stations(st(o, 2), st(z, 0)).unwrap();
    let x = p
        .create_loop_closure(st(o, 0), st(z, 1), ClosureStrategy::Move)
        .unwrap();
    assert!(x.is_aligned, "setup");
    let c = plant(&mut p, st(o, 1), st(z, 1), ClosureStrategy::Batch, false);
    let e = plant(&mut p, st(k, 1), st(z, 1), ClosureStrategy::Batch, false);
    // Before the combine C squeezes O's first shot through the snap; E is a
    // real (tiny) loop.
    let skipped: Vec<u64> = p
        .closures_across_one_shot()
        .iter()
        .map(|s| s.closure_id)
        .collect();
    assert_eq!(skipped, vec![c]);
    let candidate = p
        .combine_candidates_at_point(st(k, 1))
        .into_iter()
        .find(|cand| {
            [cand.section_a, cand.section_b] == [k, o] || [cand.section_a, cand.section_b] == [o, k]
        })
        .expect("K and O are offered");
    assert!(candidate.blocked_reason.is_none(), "{candidate:?}");
    let plan = if candidate.section_a == k {
        &candidate.plan_keeping_a
    } else {
        &candidate.plan_keeping_b
    };
    let removed: Vec<_> = plan
        .closures_removed_across_one_shot
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.station_a, r.station_b, r.kind))
        .collect();
    assert_eq!(
        removed,
        vec![
            (st(k, 1), st(z, 1), crate::CombineRemovedKind::LoopClosure),
            (
                st(o, 0),
                st(z, 1),
                crate::CombineRemovedKind::SnappedClosure
            ),
        ],
        "{plan:?}"
    );
    let mut after = p.clone();
    let end = |cand_end| crate::CombineJoinEnds {
        kept: cand_end,
        other: if candidate.section_a == k {
            candidate.end_b
        } else {
            candidate.end_a
        },
    };
    let kept_end = if candidate.section_a == k {
        candidate.end_a
    } else {
        candidate.end_b
    };
    // In a debug build the combine's backstop asserts that it left no
    // closure across one shot its plan did not list.
    let outcome = after
        .combine_sections_reporting(
            k,
            o,
            &super::combine_closure_tests::options(Some(end(kept_end))),
        )
        .unwrap();
    assert_eq!(outcome.removed_across_one_shot.len(), 2);
    assert!(after.closures_across_one_shot().is_empty());
    assert!(after.loop_closures.iter().any(|lc| lc.id == c));
    assert!(!after
        .loop_closures
        .iter()
        .any(|lc| lc.id == e || lc.id == x.id));
    assert!(view_of(&after).1.is_empty());
}

fn deep(azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    ShotData {
        depth_in,
        depth_out,
        ..shot(azimuth, length)
    }
}

fn origin() -> GpsCoordinates {
    GpsCoordinates::new(20.5, -87.3, 0.0).unwrap()
}

/// Plant a closure as a file can carry it, past `create_loop_closure`'s
/// measurement and refusals.
fn plant(
    project: &mut Project,
    a: StationRef,
    b: StationRef,
    strategy: ClosureStrategy,
    is_aligned: bool,
) -> u64 {
    let (station_a, station_b) = (
        project.key_for_ref(a).unwrap(),
        project.key_for_ref(b).unwrap(),
    );
    project.normalize_next_closure_id();
    let id = project.next_closure_id;
    project.next_closure_id += 1;
    project.loop_closures.push(LoopClosure {
        id,
        station_a,
        station_b,
        path_length: 10.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy,
        is_aligned,
    });
    id
}

/// A survey with no snap (no aligned closure the solve does not already
/// take), but every neighbour of one: a placement datum at A's start (the
/// frame) and a soft GPS fix on a spur's end, a `Batch` loop and a
/// `LeastSquaresWeighted` one, a spur merged off the second, a `Move`
/// closure a later edit tore 0.3 m apart (so not aligned), a legacy aligned
/// `Batch` closure (the destructive adjust before 0.5.0 wrote those), a
/// closure across one shot (skipped), a hand move on a loop station, and a
/// second, datum-less part with a loop of its own. Depths vary.
fn snap_free_survey() -> (Project, [SectionId; 6]) {
    let mut p = Project::new("Snap-free".to_string(), None);
    let a = add(
        &mut p,
        "A",
        vec![
            deep(10.0, 8.0, 2.0, 3.1),
            deep(80.0, 7.5, 3.1, 4.4),
            deep(170.0, 9.0, 4.4, 5.0),
            deep(250.0, 6.2, 5.0, 3.6),
            deep(330.0, 4.1, 3.6, 2.2),
        ],
    );
    p.set_section_offset(a, (12.0, -7.5));
    p.zero_datums.push(ZeroDatum::new(
        p.key_for_ref(StationRef::new(a, 0)).unwrap(),
        origin(),
    ));
    p.create_loop_closure(
        StationRef::new(a, 5),
        StationRef::new(a, 0),
        ClosureStrategy::Batch,
    )
    .unwrap();
    let b = add(
        &mut p,
        "B",
        vec![
            deep(120.0, 6.5, 5.0, 6.2),
            deep(200.0, 5.5, 6.2, 6.0),
            deep(280.0, 7.7, 6.0, 4.0),
        ],
    );
    let at = xy(&p, StationRef::new(a, 2));
    p.set_section_offset(b, at);
    p.merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    p.create_loop_closure(
        StationRef::new(b, 3),
        StationRef::new(a, 4),
        ClosureStrategy::LeastSquaresWeighted,
    )
    .unwrap();
    let c = add(
        &mut p,
        "C",
        vec![deep(60.0, 3.3, 6.2, 7.0), deep(20.0, 4.8, 7.0, 7.7)],
    );
    let at = xy(&p, StationRef::new(b, 1));
    p.set_section_offset(c, at);
    p.merge_stations(StationRef::new(b, 1), StationRef::new(c, 0))
        .unwrap();
    // D from A:3 towards A:0, its end 0.3 m off: the Move closure measures
    // the gap and is not aligned.
    let (from, to) = (xy(&p, StationRef::new(a, 3)), xy(&p, StationRef::new(a, 0)));
    let bearing = (to.0 - from.0)
        .atan2(to.1 - from.1)
        .to_degrees()
        .rem_euclid(360.0);
    let length = (to.0 - from.0).hypot(to.1 - from.1);
    let d = add(&mut p, "D", vec![deep(bearing + 2.0, length, 3.6, 2.0)]);
    p.set_section_offset(d, from);
    p.merge_stations(StationRef::new(a, 3), StationRef::new(d, 0))
        .unwrap();
    let torn = p
        .create_loop_closure(
            StationRef::new(d, 1),
            StationRef::new(a, 0),
            ClosureStrategy::Move,
        )
        .unwrap();
    assert!(!torn.is_aligned, "setup: D's end is 0.3 m off A's start");
    // A hand move on a loop station, settled as opening the file does.
    let key = p.key_for_ref(StationRef::new(b, 2)).unwrap();
    p.station_offsets.insert(key, (0.04, -0.03, 0.0));
    p.redrive_offsets_after_load();
    // A second part, without a datum, with a loop and a torn Move closure.
    let g = add(
        &mut p,
        "G",
        vec![
            deep(0.0, 11.0, 1.0, 1.5),
            deep(95.0, 9.4, 1.5, 2.5),
            deep(185.0, 10.3, 2.5, 1.9),
            deep(272.0, 8.8, 1.9, 1.2),
        ],
    );
    p.set_section_offset(g, (140.0, 90.0));
    p.create_loop_closure(
        StationRef::new(g, 4),
        StationRef::new(g, 0),
        ClosureStrategy::Batch,
    )
    .unwrap();
    let h = add(&mut p, "H", vec![deep(135.0, 9.0, 1.5, 2.0)]);
    let at = xy(&p, StationRef::new(g, 1));
    p.set_section_offset(h, at);
    p.merge_stations(StationRef::new(g, 1), StationRef::new(h, 0))
        .unwrap();
    // Legacy and odd closures, as a file carries them.
    plant(
        &mut p,
        StationRef::new(c, 2),
        StationRef::new(a, 1),
        ClosureStrategy::Batch,
        true,
    );
    plant(
        &mut p,
        StationRef::new(c, 1),
        StationRef::new(c, 2),
        ClosureStrategy::Batch,
        false,
    );
    plant(
        &mut p,
        StationRef::new(h, 1),
        StationRef::new(g, 3),
        ClosureStrategy::Move,
        false,
    );
    // A soft GPS fix on C's end, 1.4 m off where it is shown.
    let shown = xy(&p, StationRef::new(c, 2));
    let (lat, lon) = local_to_gps(shown.0 + 1.2, shown.1 - 0.7, &origin()).unwrap();
    p.zero_datums.push(ZeroDatum {
        station_ref: p.key_for_ref(StationRef::new(c, 2)).unwrap(),
        gps: GpsCoordinates::new(lat, lon, 0.0).unwrap(),
        accuracy_m: Some(1.5),
        entrance: None,
    });
    assert!(
        !p.loop_closures
            .iter()
            .any(|c| c.is_aligned && !Project::closure_is_eligible(c.strategy)),
        "setup: no snap"
    );
    assert_eq!(p.gps_fix_datums().len(), 1, "setup: one GPS fix");
    assert_eq!(p.closures_across_one_shot().len(), 1, "setup: one skipped");
    (p, [a, b, c, d, g, h])
}

/// Every station's closure-adjusted `x, y, z` as raw bits, in station order.
fn view_bits(project: &Project) -> Vec<(u32, u32, [u64; 3])> {
    let mut out: Vec<(u32, u32, [u64; 3])> = project
        .compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default())
        .into_iter()
        .map(|s| {
            (
                s.station_ref.section_id,
                s.station_ref.station_index,
                [
                    s.coordinates.x.to_bits(),
                    s.coordinates.y.to_bits(),
                    s.coordinates.z.to_bits(),
                ],
            )
        })
        .collect();
    out.sort_unstable_by_key(|(s, i, _)| (*s, *i));
    out
}

/// The view of [`snap_free_survey`] as 3a4f62b0c (before #736) computed it
/// on Linux, as raw bits (`x`, `y`, `z` per station).
const SNAP_FREE_VIEW: [(u32, u32, [u64; 3]); 22] = [
    (
        0,
        0,
        [
            0x4028_0000_0000_0000,
            0xc01e_0000_0000_0000,
            0x4000_cccc_cccc_cccd,
        ],
    ),
    (
        0,
        1,
        [
            0x402e_a839_0b96_34fb,
            0x3fcb_f2a6_6e52_6f20,
            0x4012_8888_8888_8889,
        ],
    ),
    (
        0,
        2,
        [
            0x4033_bf9c_e153_44c7,
            0xbfdc_f24b_e17a_449a,
            0x4012_6666_6666_6667,
        ],
    ),
    (
        0,
        3,
        [
            0x4033_73c7_743b_b692,
            0xc022_4ec0_8b89_ca56,
            0x4012_2222_2222_2222,
        ],
    ),
    (
        0,
        4,
        [
            0x402b_0010_0eb0_d02a,
            0xc025_7c6a_c8d4_eaba,
            0x400d_dddd_dddd_dddd,
        ],
    ),
    (
        0,
        5,
        [
            0x4028_0000_0000_0000,
            0xc01e_0000_0000_0000,
            0x4000_cccc_cccc_cccd,
        ],
    ),
    (
        1,
        0,
        [
            0x4033_bf9c_e153_44c7,
            0xbfdc_f24b_e17a_4498,
            0x4012_6666_6666_6667,
        ],
    ),
    (
        1,
        1,
        [
            0x4034_bb8a_5403_96f8,
            0xc015_3d17_b8dd_4104,
            0x4018_cccc_cccc_cccd,
        ],
    ),
    (
        1,
        2,
        [
            0x4034_5983_b00d_c9dd,
            0xc025_7fff_bd3b_4331,
            0x4018_0000_0000_0000,
        ],
    ),
    (
        1,
        3,
        [
            0x402b_0010_0eb0_d02a,
            0xc025_7c6a_c8d4_eaba,
            0x400d_dddd_dddd_dddd,
        ],
    ),
    (
        2,
        0,
        [
            0x4034_bb8a_5403_96f8,
            0xc015_3d17_b8dd_4103,
            0x4018_cccc_cccc_cccd,
        ],
    ),
    (
        2,
        1,
        [
            0x4034_9f59_6440_429b,
            0xc00f_d73d_562a_2716,
            0x401c_0000_0000_0000,
        ],
    ),
    (
        2,
        2,
        [
            0x402e_a839_0b96_34fa,
            0x3fcb_f2a6_6e52_6ee0,
            0x4012_8888_8888_8889,
        ],
    ),
    (
        3,
        0,
        [
            0x4033_73c7_743b_b692,
            0xc022_4ec0_8b89_ca55,
            0x4012_2222_2222_2222,
        ],
    ),
    (
        3,
        1,
        [
            0x4022_bba7_b27c_1009,
            0xc022_153d_a5b0_8d64,
            0x4000_0000_0000_0000,
        ],
    ),
    (
        4,
        0,
        [
            0x4061_8000_0000_0000,
            0x4056_8000_0000_0000,
            0x3ff1_9999_9999_999a,
        ],
    ),
    (
        4,
        1,
        [
            0x4061_8598_9d8f_58f1,
            0x4059_3e97_0ad2_34d3,
            0x3ff8_0000_0000_0000,
        ],
    ),
    (
        4,
        2,
        [
            0x4062_aff5_7201_01c9,
            0x4059_0328_ae53_0be5,
            0x4004_0000_0000_0000,
        ],
    ),
    (
        4,
        3,
        [
            0x4062_9827_38c0_1f3e,
            0x4056_72d6_9d92_1dd1,
            0x3ffe_6666_6666_6666,
        ],
    ),
    (
        4,
        4,
        [
            0x4061_8000_0000_0000,
            0x4056_8000_0000_0000,
            0x3ff1_9999_9999_999a,
        ],
    ),
    (
        5,
        0,
        [
            0x4061_8598_9d8f_58f1,
            0x4059_3e97_0ad2_34d3,
            0x3ff8_0000_0000_0000,
        ],
    ),
    (
        5,
        1,
        [
            0x4062_50ed_ab44_e055,
            0x4057_a7ec_ef67_260c,
            0x4000_0000_0000_0000,
        ],
    ),
];

/// No snap, no change: a survey whose closures include no aligned one the
/// solve does not already take is adjusted exactly as before #736, the
/// torn `Move` closure, the legacy aligned `Batch` one, the closure across
/// one shot, the hand move and the GPS fix included. On the Linux the values
/// were pinned on it is the same to the last bit; the comparison allows
/// 1e-12 m, a few ulps at these magnitudes, because `sin` and `cos` come from
/// the platform's libm and may round the last bit differently on macOS or
/// Windows. Any real change to the solve moves a station by far more.
#[test]
fn a_survey_without_a_snap_keeps_its_closures_computed_view() {
    let (p, _) = snap_free_survey();
    assert!(p
        .compute_adjusted_stations(&ClosureAdjustmentConfig::default())
        .failures
        .is_empty());
    let now = view_bits(&p);
    assert_eq!(now.len(), SNAP_FREE_VIEW.len());
    for (got, want) in now.iter().zip(SNAP_FREE_VIEW.iter()) {
        assert_eq!((got.0, got.1), (want.0, want.1));
        let (g, w) = (got.2.map(f64::from_bits), want.2.map(f64::from_bits));
        let off = (0..3).map(|k| (g[k] - w[k]).abs()).fold(0.0, f64::max);
        assert!(
            off <= 1e-12,
            "{}:{} is at {g:?} in the Closures Computed View, was {w:?} ({off:.3e} m off)",
            got.0,
            got.1,
        );
    }
}
