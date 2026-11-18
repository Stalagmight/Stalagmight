//! Combine Sections and the alignment the combined project gives each `Move`
//! closure (#736, `combine_alignment.rs`). The app re-measures every snap's
//! alignment after each command, and since #736 a snap's flag decides what
//! the Closures Computed solve ties; so the combine's plan predicts what the
//! re-measure will find on the combined project, the combine re-measures
//! itself before its backstop, and the command's epilogue finds nothing left
//! to change. One survey per case the plan models, each built so the plan
//! before #736's follow-up got it wrong.

use super::combine_closure_tests::{
    add, check_combined, check_removed_across_one_shot, dist, options, settle_like_the_epilogue,
    shot, xy,
};
use crate::{
    ClosureStrategy, CombineJoinEnds, CombinePlan, CombineRemovedKind, Project, SectionEnd,
    SectionId, StationRef, TornTie, TornTieKind, TornTieReason,
};

fn st(section: SectionId, index: u32) -> StationRef {
    StationRef::new(section, index)
}

/// Move `id` so its station `index` sits exactly on `target`, as shown.
fn place(project: &mut Project, id: SectionId, index: u32, target: (f64, f64)) {
    let now = xy(project, st(id, index));
    project.adjust_section_offset(id, target.0 - now.0, target.1 - now.1);
}

/// One shot from `from` to `to`, as a bearing and a length.
fn toward(from: (f64, f64), to: (f64, f64)) -> crate::ShotData {
    let azimuth = (to.0 - from.0)
        .atan2(to.1 - from.1)
        .to_degrees()
        .rem_euclid(360.0);
    shot(azimuth, dist(from, to))
}

/// The closure between `a` and `b`, in either order.
fn closure_between(project: &Project, a: StationRef, b: StationRef) -> Option<&crate::LoopClosure> {
    project.loop_closures.iter().find(|c| {
        let ends = (
            project.resolve_key(&c.station_a),
            project.resolve_key(&c.station_b),
        );
        ends == (Some(a), Some(b)) || ends == (Some(b), Some(a))
    })
}

/// The torn tie between `a` and `b` the plan lists, in either order.
fn torn_between(plan: &CombinePlan, a: StationRef, b: StationRef) -> Option<TornTie> {
    plan.torn_ties
        .iter()
        .copied()
        .find(|t| (t.station_a, t.station_b) == (a, b) || (t.station_a, t.station_b) == (b, a))
}

/// Combine `other` into `kept` at the given ends as the dialog would, and
/// check it as the sweeps do: the plan against the result, the closures it
/// removed, the alignment flags it changed, the epilogue changing none, and
/// every structural invariant. Returns the plan and the combined project.
fn combine_checked(
    project: &Project,
    (kept, kept_end): (SectionId, SectionEnd),
    (other, other_end): (SectionId, SectionEnd),
    what: &str,
) -> (CombinePlan, Project) {
    let join = CombineJoinEnds {
        kept: kept_end,
        other: other_end,
    };
    let candidate = project
        .combine_candidate_joining(kept, other, Some(join))
        .unwrap();
    assert!(candidate.blocked_reason.is_none(), "{what}: {candidate:?}");
    let plan = if candidate.section_a == kept {
        candidate.plan_keeping_a.clone()
    } else {
        candidate.plan_keeping_b.clone()
    };
    let mut after = project.clone();
    let outcome = after
        .combine_sections_reporting(kept, other, &options(Some(join)))
        .unwrap();
    check_removed_across_one_shot(
        project,
        &after,
        &plan,
        &outcome.removed_across_one_shot,
        what,
    );
    check_combined(project, &after, &candidate, kept, what, &[]);
    let mut settled = after.clone();
    assert_eq!(settle_like_the_epilogue(&mut settled), 0, "{what}");
    (plan, after)
}

/// A passage P from `from` to `to` by way of `via`, merged at both ends: the
/// loop the combine closes.
fn passage(project: &mut Project, from: StationRef, via: (f64, f64), to: StationRef) -> SectionId {
    let (a, b) = (xy(project, from), xy(project, to));
    let p = add(project, "P", vec![toward(a, via), toward(via, b)]);
    place(project, p, 0, a);
    project.merge_stations(from, st(p, 0)).unwrap();
    project.merge_stations(st(p, 2), to).unwrap();
    p
}

/// K runs 10 m east; O starts 0.3 m past K's end and runs 5 m east, tied to
/// it by an ordinary loop closure, so combining moves O back by 0.3 m. A
/// passage P from K's start to O's end closes the loop by merges. B, merged
/// at K's start, ends 2 m north of where O's end lands; C, merged at O's end,
/// runs 2 m north. B's end and C's end are a snap the 0.3 m tore: the `Move`
/// closure between them is not aligned.
fn reclosed_across_the_sides() -> (Project, [SectionId; 4]) {
    let mut project = Project::new("Across".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let o = add(&mut project, "O", vec![shot(90.0, 5.0)]);
    let b = add(&mut project, "B", vec![toward((0.0, 0.0), (15.0, 2.0))]);
    let c = add(&mut project, "C", vec![shot(0.0, 2.0)]);
    place(&mut project, o, 0, (10.3, 0.0));
    passage(&mut project, st(k, 0), (7.0, -3.0), st(o, 1));
    project.merge_stations(st(k, 0), st(b, 0)).unwrap();
    place(&mut project, c, 0, (15.3, 0.0));
    project.merge_stations(st(o, 1), st(c, 0)).unwrap();
    project
        .create_loop_closure(st(b, 1), st(c, 1), ClosureStrategy::Move)
        .unwrap();
    project
        .create_loop_closure(st(k, 1), st(o, 0), ClosureStrategy::Batch)
        .unwrap();
    project.redrive_offsets_after_load();
    (project, [k, o, b, c])
}

/// A torn snap the combine's move re-closes across the two sides (#736): the
/// shift brings C's end back onto B's, where the `Move` closure would be
/// aligned again by the next re-measure and tie the combined section to
/// itself through B and C. It becomes an ordinary loop closure, as a snap
/// there does, and the dialog lists it as the loop closure left from a snap
/// it is, not as a snapped closure. Before, the plan left it alone and the
/// command's epilogue snapped it.
#[test]
fn a_torn_snap_the_shift_recloses_across_the_two_sides_becomes_an_ordinary_closure() {
    let (project, [k, o, b, c]) = reclosed_across_the_sides();
    let snap = closure_between(&project, st(b, 1), st(c, 1)).unwrap();
    assert!(
        snap.strategy == ClosureStrategy::Move && !snap.is_aligned,
        "{snap:?}"
    );
    let what = "re-closed across the sides";
    let (plan, after) =
        combine_checked(&project, (k, SectionEnd::End), (o, SectionEnd::Start), what);
    assert_eq!(plan.moving_section, Some(o), "{plan:?}");
    assert!((plan.shift_m - 0.3).abs() < 1e-9, "{plan:?}");
    let torn = torn_between(&plan, st(b, 1), st(c, 1)).unwrap_or_else(|| panic!("{plan:?}"));
    assert_eq!(
        (torn.kind, torn.reason),
        (TornTieKind::FormerSnappedClosure, TornTieReason::Loop)
    );
    let now = closure_between(&after, st(b, 1), st(c, 1)).unwrap();
    assert_eq!(
        (now.strategy, now.is_aligned),
        (ClosureStrategy::Batch, false)
    );
    assert!(dist(xy(&after, st(b, 1)), xy(&after, st(c, 1))) < 1e-9);
}

/// K runs 10 m east, then 2 m north; O starts 0.3 m east of K's end, tied to
/// it by an ordinary loop closure, and runs 2 m south, so its end is 0.3 m
/// east of K's station 1. A passage P from K's start to O's end closes the
/// loop by merges. A `Move` closure ties O's end to K's station 1, torn by
/// the 0.3 m.
fn reclosed_inside() -> (Project, [SectionId; 2]) {
    let mut project = Project::new("Inside".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0), shot(0.0, 2.0)]);
    let o = add(&mut project, "O", vec![shot(180.0, 2.0)]);
    place(&mut project, o, 0, (10.3, 2.0));
    passage(&mut project, st(k, 0), (5.0, -2.0), st(o, 1));
    project
        .create_loop_closure(st(o, 1), st(k, 1), ClosureStrategy::Move)
        .unwrap();
    project
        .create_loop_closure(st(k, 2), st(o, 0), ClosureStrategy::Batch)
        .unwrap();
    project.redrive_offsets_after_load();
    (project, [k, o])
}

/// A torn snap between the two sections the combine's move re-closes
/// (#736): left a `Move` closure, it would be aligned by the next re-measure,
/// a snapped closure inside the combined section, which ties the section to
/// itself. It becomes an ordinary loop closure inside the combined section,
/// as a snap between the two sections does, and the dialog lists it.
#[test]
fn a_torn_snap_the_shift_recloses_inside_the_combined_section_becomes_an_ordinary_closure() {
    let (project, [k, o]) = reclosed_inside();
    let snap = closure_between(&project, st(o, 1), st(k, 1)).unwrap();
    assert!(
        snap.strategy == ClosureStrategy::Move && !snap.is_aligned,
        "{snap:?}"
    );
    let what = "re-closed inside";
    let (plan, after) =
        combine_checked(&project, (k, SectionEnd::End), (o, SectionEnd::Start), what);
    let torn = torn_between(&plan, st(o, 1), st(k, 1)).unwrap_or_else(|| panic!("{plan:?}"));
    assert_eq!(
        (torn.kind, torn.reason),
        (TornTieKind::FormerSnappedClosure, TornTieReason::Loop)
    );
    // O's end is the combined section's station 3 now.
    let now = closure_between(&after, st(k, 3), st(k, 1)).unwrap();
    assert_eq!(
        (now.strategy, now.is_aligned),
        (ClosureStrategy::Batch, false)
    );
}

/// K runs 10 m east; O's start is merged to K's end, but O's chain starts
/// 0.4 m further east and a hand move (a manual station offset) brings it
/// back, so combining moves O by the 0.4 m residual (#740). B, merged at the
/// join, runs 3 m north in two shots. O runs 3 m north, its end 0.4 m east of
/// B's: a torn snap. A loop closure ties B's middle station to O's end.
fn reclosed_beside_a_merged_join() -> (Project, [SectionId; 3]) {
    let mut project = Project::new("Beside".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let o = add(&mut project, "O", vec![shot(0.0, 3.0)]);
    let b = add(&mut project, "B", vec![shot(0.0, 1.5), shot(0.0, 1.5)]);
    place(&mut project, o, 0, (10.4, 0.0));
    let key = project.key_for_ref(st(o, 0)).unwrap();
    project.station_offsets.insert(key, (-0.4, 0.0, 0.0));
    project.merge_stations(st(k, 1), st(o, 0)).unwrap();
    place(&mut project, b, 0, (10.0, 0.0));
    project.merge_stations(st(k, 1), st(b, 0)).unwrap();
    project
        .create_loop_closure(st(o, 1), st(b, 2), ClosureStrategy::Move)
        .unwrap();
    project
        .create_loop_closure(st(b, 1), st(o, 1), ClosureStrategy::Batch)
        .unwrap();
    project.redrive_offsets_after_load();
    (project, [k, o, b])
}

/// A torn snap the combine re-closes where it ties nothing to itself
/// (#736): B hangs off the join point, so it is on neither side, and the
/// residual brings O's end onto B's. That is a snap again (the loop through
/// O was there before), and the plan's model of the joined survey ties it:
/// the loop closure from B's middle station to O's end then joins the two
/// ends of B's second shot, so the dialog lists it and the combine removes
/// it. Before, the plan swept the snap as torn, listed nothing, and the
/// epilogue's re-measure left that closure across one shot.
#[test]
fn a_torn_snap_a_merged_join_recloses_beside_it_snaps_again() {
    let (project, [k, o, b]) = reclosed_beside_a_merged_join();
    let snap = closure_between(&project, st(o, 1), st(b, 2)).unwrap();
    assert!(
        snap.strategy == ClosureStrategy::Move && !snap.is_aligned,
        "{snap:?}"
    );
    assert!(project.closures_across_one_shot().is_empty());
    let what = "re-closed beside a merged join";
    let (plan, after) =
        combine_checked(&project, (k, SectionEnd::End), (o, SectionEnd::Start), what);
    assert_eq!(plan.moving_section, Some(o), "{plan:?}");
    assert!(plan.torn_ties.is_empty(), "{plan:?}");
    let removed = plan.closures_removed_across_one_shot.unwrap_or_default();
    assert_eq!(removed.len(), 1, "{removed:?}");
    assert_eq!(removed[0].kind, CombineRemovedKind::LoopClosure);
    assert!(
        [
            (removed[0].station_a, removed[0].station_b),
            (removed[0].station_b, removed[0].station_a)
        ]
        .contains(&(st(b, 1), st(o, 1))),
        "{removed:?}"
    );
    // O's end is the combined section's station 2; the snap holds there.
    let now = closure_between(&after, st(k, 2), st(b, 2)).unwrap();
    assert_eq!(
        (now.strategy, now.is_aligned),
        (ClosureStrategy::Move, true)
    );
    assert!(closure_between(&after, st(b, 1), st(k, 2)).is_none());
}

/// K runs 10 m east. O's start is side by side with K's end on the chain,
/// but a hand move shows it 0.5 m north; O runs 5 m east. S, merged at O's
/// end, runs back to O's start as shown, where a `Move` closure snaps the
/// two: a loop through O.
fn pulled_apart() -> (Project, [SectionId; 3]) {
    let mut project = Project::new("Apart".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let o = add(&mut project, "O", vec![shot(90.0, 5.0)]);
    let s = add(&mut project, "S", vec![toward((15.0, 0.0), (10.0, 0.5))]);
    place(&mut project, o, 0, (10.0, 0.0));
    let key = project.key_for_ref(st(o, 0)).unwrap();
    project.station_offsets.insert(key, (0.0, 0.5, 0.0));
    place(&mut project, s, 0, (15.0, 0.0));
    project.merge_stations(st(o, 1), st(s, 0)).unwrap();
    project
        .create_loop_closure(st(o, 0), st(s, 1), ClosureStrategy::Move)
        .unwrap();
    project.redrive_offsets_after_load();
    (project, [k, o, s])
}

/// A snap the combine pulls apart without moving a side (#736): the joined
/// station keeps K's hand move (none), so O's start lands on K's end, 0.5 m
/// from S's end, which stays. The next re-measure would find the snap torn;
/// the plan tears it into an ordinary loop closure instead, as it tears a
/// snap a moving side pulls apart, and the dialog lists it with that reason
/// (the joined station moves), not as a tie that would tie the combined
/// section to itself. Before, the plan swept it as a snap and the epilogue
/// tore it unannounced.
#[test]
fn a_snap_a_dropped_hand_move_pulls_apart_becomes_an_ordinary_closure() {
    let (project, [k, o, s]) = pulled_apart();
    let snap = closure_between(&project, st(o, 0), st(s, 1)).unwrap();
    assert!(
        snap.strategy == ClosureStrategy::Move && snap.is_aligned,
        "{snap:?}"
    );
    let what = "pulled apart";
    let (plan, after) =
        combine_checked(&project, (k, SectionEnd::End), (o, SectionEnd::Start), what);
    assert_eq!(plan.moving_section, None, "{plan:?}");
    let torn = torn_between(&plan, st(o, 0), st(s, 1)).unwrap_or_else(|| panic!("{plan:?}"));
    assert_eq!(
        (torn.kind, torn.reason),
        (
            TornTieKind::SnappedClosure,
            TornTieReason::JoinedStationMoves
        )
    );
    let now = closure_between(&after, st(k, 1), st(s, 1)).unwrap();
    assert_eq!(
        (now.strategy, now.is_aligned),
        (ClosureStrategy::Batch, false)
    );
    assert!((dist(xy(&after, st(k, 1)), xy(&after, st(s, 1))) - 0.5).abs() < 1e-9);
}
