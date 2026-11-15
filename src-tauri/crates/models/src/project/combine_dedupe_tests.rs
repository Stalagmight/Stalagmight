//! Combine Sections making two loop closures identical (#749): which one
//! `dedupe_closures_in` keeps, case by case, and the surveys that reach it
//! through a combine, checked against the plan the station menu offered and
//! the closure-adjusted view (`combine_closure_tests::check_combined`).

use super::combine_closure_tests::{
    add, adjusted, check_combined, check_removed_across_one_shot, check_replaced, dist, options,
    shot, xy,
};
use crate::{
    ClosureStrategy, CombineCandidate, CombineJoinEnds, CombineJoinKind, CombinePlan,
    CombineRemovedKind, CombineReplacedClosure, CombineReplacedKind, KeptMovesReason, LoopClosure,
    Project, SectionEnd, SectionId, StationRef, TornTieKind, TornTieReason,
};

/// Plant a loop closure between `a` and `b` as a file can carry it: any
/// strategy (the app creates no legacy one any more), any alignment, and
/// `id` (a file can repeat one), measured off the current geometry.
fn plant(
    project: &mut Project,
    (a, b): (StationRef, StationRef),
    strategy: ClosureStrategy,
    is_aligned: bool,
    id: u64,
) {
    let misclosure = dist(xy(project, a), xy(project, b));
    project.next_closure_id = project.next_closure_id.max(id + 1);
    let closure = LoopClosure {
        id,
        station_a: project.key_for_ref(a).unwrap(),
        station_b: project.key_for_ref(b).unwrap(),
        path_length: 10.0,
        misclosure_distance: misclosure,
        error_percentage: misclosure * 10.0,
        horizontal_misclosure: misclosure,
        vertical_misclosure: 0.0,
        strategy,
        is_aligned,
    };
    project.loop_closures.push(closure);
}

/// The plan `candidate` offers for keeping `kept`.
fn plan_keeping(candidate: &CombineCandidate, kept: SectionId) -> &CombinePlan {
    if candidate.section_a == kept {
        &candidate.plan_keeping_a
    } else {
        &candidate.plan_keeping_b
    }
}

/// Every closure between `a` and `b`, either way round, as (id, strategy,
/// aligned).
fn closures_between(
    project: &Project,
    a: StationRef,
    b: StationRef,
) -> Vec<(u64, ClosureStrategy, bool)> {
    project
        .loop_closures
        .iter()
        .filter(|c| {
            let ends = (
                project.resolve_key(&c.station_a).unwrap(),
                project.resolve_key(&c.station_b).unwrap(),
            );
            ends == (a, b) || ends == (b, a)
        })
        .map(|c| (c.id, c.strategy, c.is_aligned))
        .collect()
}

// ---- The rule -------------------------------------------------------------

const LEGACY: ClosureStrategy = ClosureStrategy::ProportionalByLength;
const EQUAL: ClosureStrategy = ClosureStrategy::EqualDistribution;
const BATCH: ClosureStrategy = ClosureStrategy::Batch;
const LSQ: ClosureStrategy = ClosureStrategy::LeastSquaresWeighted;
const MOVE: ClosureStrategy = ClosureStrategy::Move;

/// S runs four shots; T runs one, apart. On S's stations 0 and 2 the
/// closures `pair` (strategy, aligned), in that order, with ids 1, 2, ...;
/// the first is stored S:2 to S:0, the rest S:0 to S:2 (a pair is a pair
/// whichever way round). Between them a `Batch` closure S:1 to S:3, and
/// after them a legacy closure and a `Batch` one between T's two stations,
/// both about T only. Dedupes onto S and returns the ids left, in order.
fn dedupe(pair: &[(ClosureStrategy, bool)]) -> Vec<u64> {
    let mut project = Project::new("Dedupe".to_string(), None);
    let s = add(
        &mut project,
        "S",
        vec![
            shot(90.0, 10.0),
            shot(0.0, 10.0),
            shot(270.0, 10.0),
            shot(180.0, 9.0),
        ],
    );
    let t = add(&mut project, "T", vec![shot(0.0, 3.0)]);
    let at = |id: SectionId, index: u32| StationRef::new(id, index);
    for (n, (strategy, aligned)) in pair.iter().enumerate() {
        let ends = if n == 0 {
            (at(s, 2), at(s, 0))
        } else {
            (at(s, 0), at(s, 2))
        };
        plant(&mut project, ends, *strategy, *aligned, n as u64 + 1);
        if n == 0 {
            plant(&mut project, (at(s, 1), at(s, 3)), BATCH, false, 100);
        }
    }
    plant(&mut project, (at(t, 0), at(t, 1)), LEGACY, false, 200);
    plant(&mut project, (at(t, 0), at(t, 1)), BATCH, false, 201);
    project.dedupe_closures_in(s);
    project.loop_closures.iter().map(|c| c.id).collect()
}

/// `dedupe_closures_in` keeps, of closures the combine made identical, the
/// first one the solve takes (`Batch`, `LeastSquaresWeighted`), else the
/// first one; each kept closure keeps its place, and closures that do not
/// touch the combined section are left alone.
///
/// Before #749 it kept the first one whatever it was: the rows marked
/// "#749" kept the closure the solve ignores and dropped the one it takes.
#[test]
fn the_closure_dedupe_keeps_the_first_closure_the_solve_takes() {
    let (others, t) = (100, [200, 201]);
    let cases: [(&str, &[(ClosureStrategy, bool)], &[u64]); 12] = [
        (
            "#749: legacy, then Batch",
            &[(LEGACY, false), (BATCH, false)],
            &[100, 2],
        ),
        (
            "Batch, then legacy",
            &[(BATCH, false), (LEGACY, false)],
            &[1, 100],
        ),
        (
            "#749: equal, then least squares",
            &[(EQUAL, false), (LSQ, false)],
            &[100, 2],
        ),
        (
            "#749: torn snap, then Batch",
            &[(MOVE, false), (BATCH, false)],
            &[100, 2],
        ),
        (
            "#749: snap, then Batch",
            &[(MOVE, true), (BATCH, false)],
            &[100, 2],
        ),
        (
            "Batch, then snap",
            &[(BATCH, false), (MOVE, true)],
            &[1, 100],
        ),
        ("both eligible", &[(BATCH, false), (LSQ, false)], &[1, 100]),
        (
            "both eligible, swapped",
            &[(LSQ, false), (BATCH, false)],
            &[1, 100],
        ),
        (
            "neither eligible",
            &[(LEGACY, false), (MOVE, true)],
            &[1, 100],
        ),
        (
            "neither eligible, swapped",
            &[(MOVE, true), (EQUAL, false)],
            &[1, 100],
        ),
        (
            "#749: the first eligible of many",
            &[
                (LEGACY, false),
                (MOVE, true),
                (BATCH, false),
                (EQUAL, false),
                (LSQ, false),
            ],
            &[100, 3],
        ),
        ("one closure alone", &[(LEGACY, false)], &[1, 100]),
    ];
    for (what, pair, kept) in cases {
        let mut want: Vec<u64> = kept.to_vec();
        want.extend(t);
        assert_eq!(dedupe(pair), want, "{what}");
        assert!(want.contains(&others), "{what}: setup");
    }
}

/// The dedupe drops by position: a file can repeat a closure id, and before
/// #749 dropping a legacy duplicate by its id also took a `Batch` closure
/// sharing that id, so neither was left on the pair.
#[test]
fn the_closure_dedupe_drops_by_position_so_a_repeated_id_keeps_its_twin() {
    let mut project = Project::new("Repeated id".to_string(), None);
    let s = add(
        &mut project,
        "S",
        vec![shot(90.0, 10.0), shot(0.0, 10.0), shot(270.0, 9.0)],
    );
    let ends = (StationRef::new(s, 0), StationRef::new(s, 2));
    plant(&mut project, ends, LEGACY, false, 5);
    plant(&mut project, ends, BATCH, false, 5);
    plant(
        &mut project,
        (ends.0, StationRef::new(s, 3)),
        BATCH,
        false,
        5,
    );
    project.dedupe_closures_in(s);
    let left: Vec<(u64, ClosureStrategy)> = project
        .loop_closures
        .iter()
        .map(|c| (c.id, c.strategy))
        .collect();
    assert_eq!(left, vec![(5, BATCH), (5, BATCH)]);
    assert_eq!(
        closures_between(&project, ends.0, ends.1),
        vec![(5, BATCH, false)]
    );
}

// ---- Through a combine ----------------------------------------------------

/// The smallest survey of #749. B (created first, so the lowest id: its
/// start is where the Closures Computed View holds the survey) runs 10 m
/// east, 9.6 m back west and 5 m north; a closure with `strategy` ties its
/// station 2, 0.4 m east of its start, back to its start. A runs one shot,
/// 4 m south, and its end is merged to B:2 (the middle of B). The station
/// menu on A's end offers A with B across that closure.
fn legacy_loop(strategy: ClosureStrategy) -> (Project, SectionId, SectionId) {
    let mut project = Project::new("Legacy loop".to_string(), None);
    let b = add(
        &mut project,
        "B",
        vec![shot(90.0, 10.0), shot(270.0, 9.6), shot(0.0, 5.0)],
    );
    let a = add(&mut project, "A", vec![shot(180.0, 4.0)]);
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 2))
        .unwrap();
    assert!(
        dist(
            xy(&project, StationRef::new(a, 1)),
            xy(&project, StationRef::new(b, 2))
        ) < 1e-9,
        "setup: the merge put A's end on B:2"
    );
    let id = project.next_closure_id;
    plant(
        &mut project,
        (StationRef::new(b, 2), StationRef::new(b, 0)),
        strategy,
        false,
        id,
    );
    (project, b, a)
}

/// #749. In [`legacy_loop`], combining A's end with B's start keeps A and
/// tears the merge A:1-B:2: as a `Batch` closure it closes B's loop, so the
/// Closures Computed View will solve the joined survey, and B holds the
/// station that view is held at, so A (the kept section) is the side that
/// moves, 0.4 m onto B's start (`KeptMovesReason::AdjustmentAnchor`). The
/// torn merge's closure lands on the same two stations as the closure
/// already there (B:2 to B:0, now A:3 to A:1).
///
/// Whatever that closure is, one closure is left on the pair and it is one
/// the solve takes, so the plan's verdict holds: the joined survey is solved
/// and held where B's start was, and the closure-adjusted view did not move
/// (`check_combined`). A legacy closure, or a snap that edits have torn
/// (a `Move` closure no longer aligned), gives way to the torn merge's
/// closure; an ordinary one already there stays (the first of two the solve
/// takes). Before #749 the first closure stayed whatever it was: the legacy
/// one, which the solve ignores, so the view showed the joined survey
/// unadjusted after the plan had moved A for a solved one.
///
/// The plan the station menu offered lists the closure that gives way, by
/// its stations before the combine and as what it was, with the torn merge
/// that replaces it; the combine reports exactly that. An ordinary closure
/// already there is not listed: nothing changes for the solve.
#[test]
fn a_merge_torn_onto_a_legacy_closure_leaves_the_closure_the_solve_takes() {
    for strategy in [LEGACY, EQUAL, MOVE, BATCH, LSQ] {
        let what = format!("B:2 to B:0 {strategy:?}");
        let (project, b, a) = legacy_loop(strategy);
        let planted = project.loop_closures[0].id;
        let candidate = project
            .combine_candidates_at_point(StationRef::new(a, 1))
            .into_iter()
            .find(|c| [c.section_a, c.section_b] == [b, a])
            .unwrap_or_else(|| panic!("{what}: the menu on A's end offers A with B"));
        assert_eq!(candidate.join_kind, CombineJoinKind::Closure, "{what}");
        let plan = plan_keeping(&candidate, a);
        assert_eq!(plan.moving_section, Some(a), "{what}");
        assert_eq!(
            plan.kept_moves_because,
            Some(KeptMovesReason::AdjustmentAnchor),
            "{what}"
        );
        assert!(
            (plan.shift_m - 0.4).abs() < 1e-9,
            "{what}: {}",
            plan.shift_m
        );
        let torn: Vec<_> = plan
            .torn_ties
            .iter()
            .map(|t| (t.station_a, t.station_b, t.kind, t.reason))
            .collect();
        assert_eq!(
            torn,
            vec![(
                StationRef::new(a, 1),
                StationRef::new(b, 2),
                TornTieKind::Merge,
                TornTieReason::Loop
            )],
            "{what}"
        );
        let replaced_kind = match strategy {
            MOVE => Some(CombineReplacedKind::FormerSnap),
            LEGACY | EQUAL => Some(CombineReplacedKind::LegacyClosure),
            _ => None,
        };
        let listed: Vec<CombineReplacedClosure> = replaced_kind
            .map(|kind| CombineReplacedClosure {
                station_a: StationRef::new(b, 2),
                station_b: StationRef::new(b, 0),
                kind,
                by_station_a: StationRef::new(a, 1),
                by_station_b: StationRef::new(b, 2),
                by_kind: CombineRemovedKind::Merge,
            })
            .into_iter()
            .collect();
        assert_eq!(
            plan.closures_replaced,
            (!listed.is_empty()).then(|| listed.clone()),
            "{what}"
        );
        let b0 = xy(&project, StationRef::new(b, 0));

        let mut combined = project.clone();
        let outcome = combined
            .combine_sections_reporting(
                a,
                b,
                &options(Some(CombineJoinEnds {
                    kept: SectionEnd::End,
                    other: SectionEnd::Start,
                })),
            )
            .unwrap();
        assert_eq!(outcome.replaced, listed, "{what}");

        // B's stations follow A's two: B:2 is A:3, B:0 the joined A:1.
        let (joined, b2) = (StationRef::new(a, 1), StationRef::new(a, 3));
        assert!(combined.station_merges.is_empty(), "{what}");
        assert_eq!(combined.loop_closures.len(), 1, "{what}");
        let left = closures_between(&combined, joined, b2);
        assert_eq!(left.len(), 1, "{what}: {left:?}");
        let (id, kept_strategy, aligned) = left[0];
        assert!(
            Project::closure_is_eligible(kept_strategy),
            "{what}: {left:?}"
        );
        assert!(!aligned, "{what}");
        if Project::closure_is_eligible(strategy) {
            assert_eq!(id, planted, "{what}: the first of two the solve takes");
        } else {
            assert_ne!(id, planted, "{what}: the torn merge's closure");
            assert_eq!(kept_strategy, BATCH, "{what}");
        }
        assert!(
            (combined.loop_closures[0].misclosure_distance - 0.4).abs() < 1e-9,
            "{what}: {}",
            combined.loop_closures[0].misclosure_distance
        );
        // The plan's verdict: the joined survey is solved, held where B's
        // start was, and closed there.
        assert_eq!(
            combined.adjustment_anchors_for_tests(),
            vec![joined],
            "{what}"
        );
        assert!(dist(xy(&combined, joined), b0) < 1e-9, "{what}");
        let view = adjusted(&combined);
        assert!(dist(view[&joined], b0) < 1e-9, "{what}");
        assert!(
            dist(view[&b2], view[&joined]) < 1e-6,
            "{what}: the loop is closed"
        );

        check_removed_across_one_shot(
            &project,
            &combined,
            plan,
            &outcome.removed_across_one_shot,
            &what,
        );
        check_replaced(&project, &combined, plan, &outcome.replaced, &what);
        check_combined(&project, &combined, &candidate, a, &what, &[]);
    }
}

/// The two-section loop with a passage beside it. K runs 10 m east; O runs
/// 9.6 m west from 0.4 m short of K's end and is merged onto K's start; a
/// `Batch` closure ties K's end to O's start (the join). X runs 10 m west
/// from K's end, merged onto K's start too, so its start sits on K's end:
/// a snapped (`Move`) closure ties the two, and an ordinary `Batch` one ties
/// X's start to O's start, in the order `snap_first` says.
fn snapped_beside_the_join(snap_first: bool) -> (Project, [SectionId; 3]) {
    let mut project = Project::new("Snapped".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let o = add(&mut project, "O", vec![shot(270.0, 9.6)]);
    let x = add(&mut project, "X", vec![shot(270.0, 10.0)]);
    let at = StationRef::new;
    project.merge_stations(at(o, 1), at(k, 0)).unwrap();
    project.merge_stations(at(x, 1), at(k, 0)).unwrap();
    project
        .create_loop_closure(at(k, 1), at(o, 0), BATCH)
        .unwrap();
    let snap = |project: &mut Project| {
        let snapped = project
            .create_loop_closure(at(x, 0), at(k, 1), MOVE)
            .unwrap();
        assert!(snapped.is_aligned, "setup: X's start sits on K's end");
    };
    if snap_first {
        snap(&mut project);
    }
    project
        .create_loop_closure(at(x, 0), at(o, 0), BATCH)
        .unwrap();
    if !snap_first {
        snap(&mut project);
    }
    (project, [k, o, x])
}

/// A snapped closure and an ordinary one made identical. In
/// [`snapped_beside_the_join`], combining K's end with O's start makes X's
/// two closures (to K's end, snapped; to O's start, ordinary) two closures
/// between X's start and the joined station. The ordinary one stays, in
/// either order and whichever section is kept: it is the one the solve
/// takes, and the plan's model of the joined survey counted it. The snap
/// goes with its rigidity and depth co-location, which nothing in the plan
/// models after the join; before #749 the first of the two stayed, so with
/// the snap first the solve no longer held X's start on the joined station.
/// The plan lists the snap as replaced by the ordinary closure, by their
/// stations before the combine, and the combine reports exactly that.
#[test]
fn a_snapped_and_an_ordinary_closure_made_identical_keep_the_ordinary_one() {
    for snap_first in [true, false] {
        for keep_o in [false, true] {
            let what = format!("snap first {snap_first}, keeping O {keep_o}");
            let (project, [k, o, x]) = snapped_beside_the_join(snap_first);
            let ordinary = project
                .loop_closures
                .iter()
                .find(|c| c.strategy == BATCH && c.station_a.section_id == x)
                .unwrap()
                .id;
            let join = CombineJoinEnds {
                kept: if keep_o {
                    SectionEnd::Start
                } else {
                    SectionEnd::End
                },
                other: if keep_o {
                    SectionEnd::End
                } else {
                    SectionEnd::Start
                },
            };
            let (kept, other) = if keep_o { (o, k) } else { (k, o) };
            let candidate = project
                .combine_candidate_joining(kept, other, Some(join))
                .unwrap();
            assert_eq!(candidate.join_kind, CombineJoinKind::Closure, "{what}");
            let plan = plan_keeping(&candidate, kept);
            assert!(
                !plan
                    .torn_ties
                    .iter()
                    .any(|t| t.kind == TornTieKind::SnappedClosure),
                "{what}: setup: the snap is not torn"
            );
            let listed = vec![CombineReplacedClosure {
                station_a: StationRef::new(x, 0),
                station_b: StationRef::new(k, 1),
                kind: CombineReplacedKind::SnappedClosure,
                by_station_a: StationRef::new(x, 0),
                by_station_b: StationRef::new(o, 0),
                by_kind: CombineRemovedKind::LoopClosure,
            }];
            assert_eq!(plan.closures_replaced, Some(listed.clone()), "{what}");
            let mut combined = project.clone();
            let outcome = combined
                .combine_sections_reporting(kept, other, &options(Some(join)))
                .unwrap();
            assert_eq!(outcome.replaced, listed, "{what}");

            // K's end is the joined station: K:1 keeping K, O:1 keeping O
            // (K's two stations come first).
            let joined = StationRef::new(kept, 1);
            let left = closures_between(&combined, StationRef::new(x, 0), joined);
            assert_eq!(left, vec![(ordinary, BATCH, false)], "{what}");
            let view = adjusted(&combined);
            assert!(
                dist(view[&StationRef::new(x, 0)], view[&joined]) < 1e-6,
                "{what}: the solve holds X's start on the joined station"
            );
            check_removed_across_one_shot(
                &project,
                &combined,
                plan,
                &outcome.removed_across_one_shot,
                &what,
            );
            check_replaced(&project, &combined, plan, &outcome.replaced, &what);
            check_combined(&project, &combined, &candidate, kept, &what, &[]);
        }
    }
}

/// A snapped closure the combine tears lands on a legacy closure's two
/// stations. K runs 10 m east; O runs 9.6 m back west from 0.4 m short of
/// K's end and is merged onto K's start, and a `Batch` closure ties K's end
/// to O's start (the join). X runs the same 9.6 m west from O's start onto
/// K's start, merged there, and its start is snapped to O's start. A legacy
/// closure ties X's start to K's end.
///
/// Combining K's end with O's start moves O (K holds the anchor) and so
/// tears the snap into an ordinary closure between X's start and the joined
/// station, where the legacy closure is too. The torn snap is the one the
/// solve takes, so it stays and the legacy closure goes; the plan lists that,
/// naming the replacement as the snapped closure it was.
#[test]
fn a_snap_the_combine_tears_replaces_a_legacy_closure_on_its_stations() {
    let mut project = Project::new("Torn snap".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let o = add(&mut project, "O", vec![shot(270.0, 9.6)]);
    let x = add(&mut project, "X", vec![shot(270.0, 9.6)]);
    let at = StationRef::new;
    project.merge_stations(at(o, 1), at(k, 0)).unwrap();
    project.merge_stations(at(x, 1), at(k, 0)).unwrap();
    project
        .create_loop_closure(at(k, 1), at(o, 0), BATCH)
        .unwrap();
    let snap = project
        .create_loop_closure(at(x, 0), at(o, 0), MOVE)
        .unwrap();
    assert!(snap.is_aligned, "setup: X's start sits on O's start");
    let legacy = project.next_closure_id;
    plant(&mut project, (at(x, 0), at(k, 1)), LEGACY, false, legacy);

    let join = CombineJoinEnds {
        kept: SectionEnd::End,
        other: SectionEnd::Start,
    };
    let candidate = project.combine_candidate_joining(k, o, Some(join)).unwrap();
    assert_eq!(candidate.join_kind, CombineJoinKind::Closure);
    let plan = plan_keeping(&candidate, k);
    assert_eq!(plan.moving_section, Some(o));
    assert!(
        plan.torn_ties
            .iter()
            .any(|t| t.kind == TornTieKind::SnappedClosure),
        "setup: the snap tears: {:?}",
        plan.torn_ties
    );
    let listed = vec![CombineReplacedClosure {
        station_a: at(x, 0),
        station_b: at(k, 1),
        kind: CombineReplacedKind::LegacyClosure,
        by_station_a: at(x, 0),
        by_station_b: at(o, 0),
        by_kind: CombineRemovedKind::SnappedClosure,
    }];
    assert_eq!(plan.closures_replaced, Some(listed.clone()));

    let mut combined = project.clone();
    let outcome = combined
        .combine_sections_reporting(k, o, &options(Some(join)))
        .unwrap();
    assert_eq!(outcome.replaced, listed);
    assert_eq!(
        closures_between(&combined, at(x, 0), at(k, 1)),
        vec![(snap.id, BATCH, false)],
        "the torn snap stays, as an ordinary closure"
    );
    check_removed_across_one_shot(
        &project,
        &combined,
        plan,
        &outcome.removed_across_one_shot,
        "torn snap",
    );
    check_replaced(&project, &combined, plan, &outcome.replaced, "torn snap");
    check_combined(&project, &combined, &candidate, k, "torn snap", &[]);
}
