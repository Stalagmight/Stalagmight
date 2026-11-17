//! Combine Lines where a join station was moved by hand (#755): a manual
//! station offset, as an older or hand-edited file carries it, on the kept
//! join station, the other one, or both, horizontal and in depth, with a
//! line merged at each join station. The joined station keeps the fixed
//! side's hand move, so the other join station lands on it by the gap as
//! shown: what is merged there moves with the joined station, by that gap,
//! not with its side (`combine_plan.rs`). Merged stations stay one point,
//! the plan is exact, the in-session repair and a reopen move nothing, and
//! no shot changes.

use chrono::Utc;

use super::combine_closure_tests::{add, adjusted, check_combined, dist, options, shot, xy};
use crate::{
    ClosureStrategy, CombineJoinEnds, CombineJoinKind, GpsCoordinates, LineEnd, LineId,
    LoopClosure, Project, ShotData, StationMerge, StationRef, TornTieKind, TornTieReason,
    ZeroDatum,
};

const NONE: (f64, f64, f64) = (0.0, 0.0, 0.0);

/// Two lines meeting at a join, with a line merged at each join
/// station.
struct Fixture {
    project: Project,
    k: LineId,
    o: LineId,
    /// Merged at O's join station.
    t: LineId,
    /// Merged at K's join station.
    u: LineId,
    k_end: LineEnd,
    o_end: LineEnd,
    join_k: StationRef,
    join_o: StationRef,
}

fn end_index(project: &Project, id: LineId, end: LineEnd) -> u32 {
    match end {
        LineEnd::Start => 0,
        LineEnd::End => project.get_line(id).unwrap().shots.len() as u32,
    }
}

/// Move `id` so its station `index` sits exactly on `target` as shown.
fn place(project: &mut Project, id: LineId, index: u32, target: (f64, f64)) {
    let now = xy(project, StationRef::new(id, index));
    project.adjust_line_offset(id, target.0 - now.0, target.1 - now.1);
}

/// A shot with depth readings at both ends.
fn deep(azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    ShotData {
        depth_in,
        depth_out,
        ..shot(azimuth, length)
    }
}

/// A station's depth as shown.
fn depth(project: &Project, at: StationRef) -> f64 {
    project.compute_station_raw_coords(at).unwrap().z
}

/// Plant `hand` on `at` as a file carries it (no command makes one).
fn hand_move(project: &mut Project, at: StationRef, hand: (f64, f64, f64)) {
    if hand != NONE {
        let key = project.key_for_ref(at).unwrap();
        project.station_offsets.insert(key, hand);
    }
}

/// K (east, then east-north-east, from (100, -40)) and O (north, then
/// north-west) meeting at `k_end` and `o_end`: merged, or side by side with
/// their chain positions on one point. `hand_k` and `hand_o` are planted on
/// the join stations; T is merged at O's join station and U at K's, each on
/// its station as shown. K has the lowest id.
fn join(
    k_end: LineEnd,
    o_end: LineEnd,
    merged: bool,
    hand_k: (f64, f64, f64),
    hand_o: (f64, f64, f64),
) -> Fixture {
    let mut project = Project::new("Hand moved".to_string(), None);
    // Every end reads its own depth, so the joined station's average is
    // checked against the four readings it takes.
    let k = add(
        &mut project,
        "K",
        vec![deep(90.0, 10.0, 1.0, 2.0), deep(70.0, 5.0, 2.0, 2.5)],
    );
    let o = add(
        &mut project,
        "O",
        vec![deep(0.0, 4.0, 3.0, 3.5), deep(300.0, 6.0, 3.5, 4.5)],
    );
    project.set_line_offset(k, (100.0, -40.0));
    let join_k = StationRef::new(k, end_index(&project, k, k_end));
    let join_o = StationRef::new(o, end_index(&project, o, o_end));
    hand_move(&mut project, join_k, hand_k);
    hand_move(&mut project, join_o, hand_o);
    let shown_k = xy(&project, join_k);
    if merged {
        place(&mut project, o, join_o.station_index, shown_k);
        project.merge_stations(join_k, join_o).unwrap();
    } else {
        // O's join station's chain position on K's.
        let chain_k = (shown_k.0 - hand_k.0, shown_k.1 - hand_k.1);
        let target = (chain_k.0 + hand_o.0, chain_k.1 + hand_o.1);
        place(&mut project, o, join_o.station_index, target);
    }
    let t = add(&mut project, "T", vec![deep(180.0, 3.0, 6.0, 6.5)]);
    let u = add(&mut project, "U", vec![deep(225.0, 2.0, 8.0, 8.4)]);
    for (branch, at) in [(t, join_o), (u, join_k)] {
        let shown = xy(&project, at);
        place(&mut project, branch, 0, shown);
        project
            .merge_stations(at, StationRef::new(branch, 0))
            .unwrap();
    }
    Fixture {
        project,
        k,
        o,
        t,
        u,
        k_end,
        o_end,
        join_k,
        join_o,
    }
}

/// Hold `at`'s side with a zero datum where it is.
fn hold(project: &mut Project, at: StationRef) {
    let station_ref = project.key_for_ref(at).unwrap();
    project.zero_datums.push(ZeroDatum {
        station_ref,
        gps: GpsCoordinates {
            latitude: 20.0,
            longitude: -87.0,
            depth_reference: 0.0,
        },
        accuracy_m: None,
        entrance: None,
    });
}

/// Every station of every line, as shown.
fn every_station(project: &Project) -> Vec<(StationRef, (f64, f64))> {
    project
        .lines
        .iter()
        .flat_map(|s| (0..=s.shots.len() as u32).map(move |i| StationRef::new(s.id, i)))
        .map(|at| (at, xy(project, at)))
        .collect()
}

/// The issue's shape with a 0.3 m hand move: K's end and O's start side by
/// side, O's start moved 0.3 m east by hand, T merged to it. Before #755 the
/// dialog said only that O's end moves 0.30 m, and the in-session repair
/// then moved T the 0.3 m it never mentioned. Now the plan moves T with the
/// joined station by the 0.3 m and says so; O's other stations stay (the
/// chain gap is 0): the result is exactly the plan, no shot changes, and
/// neither the repair nor a reopen moves anything.
#[test]
fn a_30_cm_hand_move_carries_its_branch_as_planned_and_nothing_after() {
    let mut project = Project::new("Offset".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let o = add(&mut project, "O", vec![shot(90.0, 10.0)]);
    let t = add(&mut project, "T", vec![shot(0.0, 5.0)]);
    project.set_line_offset(o, (10.0, 0.0));
    hand_move(&mut project, StationRef::new(o, 0), (0.3, 0.0, 0.0));
    project.set_line_offset(t, (10.3, 0.0));
    project
        .merge_stations(StationRef::new(o, 0), StationRef::new(t, 0))
        .unwrap();
    let candidate = project.combine_candidate_for_lines(k, o).unwrap();
    assert_eq!(candidate.join_kind, CombineJoinKind::CoLocated);
    assert_eq!(candidate.branches, vec![t]);
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moving_line, Some(o));
    assert_eq!(plan.moved_lines, vec![o], "T is not moved by the shift");
    assert!(plan.shift_m < 1e-12, "the chain gap is 0: {}", plan.shift_m);
    let joined = plan.moved_with_joined_station.clone().unwrap();
    assert_eq!(joined.station, StationRef::new(o, 0));
    assert_eq!(joined.lines, vec![t]);
    assert!((joined.shift_m - 0.3).abs() < 1e-12, "{}", joined.shift_m);
    let change = plan.join_offset_change_m.unwrap();
    assert!((change - 0.3).abs() < 1e-12, "{change}");

    let mut after = project.clone();
    after.combine_lines(k, o, &options(None)).unwrap();
    check_combined(&project, &after, &candidate, k, "0.3 m", &[]);
    let at = |p: &Project, s: LineId, i: u32| xy(p, StationRef::new(s, i));
    // K stays, O's start lands on K's end and its far end stays; T moves the
    // 0.3 m with the joined station, still merged to it.
    for (now, want) in [
        (at(&after, k, 0), (0.0, 0.0)),
        (at(&after, k, 1), (10.0, 0.0)),
        (at(&after, k, 2), (20.0, 0.0)),
        (at(&after, t, 0), (10.0, 0.0)),
        (at(&after, t, 1), (10.0, 5.0)),
    ] {
        assert!(dist(now, want) < 1e-9, "{now:?} is not at {want:?}");
    }
    let lengths: Vec<f64> = after
        .get_line(k)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.length)
        .collect();
    assert_eq!(lengths, vec![10.0, 10.0], "no shot changes");
    assert!(
        after.station_offsets.is_empty(),
        "the joined station keeps K's: none"
    );

    let mut repaired = after.clone();
    assert_eq!(
        repaired.realign_merges(),
        0,
        "the repair has nothing to fix"
    );
    let mut reopened = after.clone();
    reopened.redrive_offsets_after_load();
    for (fixed, what) in [(&repaired, "repair"), (&reopened, "reopen")] {
        for (station, was) in every_station(&after) {
            assert!(
                dist(xy(fixed, station), was) < 1e-9,
                "the {what} moved {station:?}"
            );
        }
    }
}

/// The hand moves the sweep below plants: under a millimetre, float dust,
/// 0.3 m across, 0.2 m in depth alone (merges are horizontal: nothing is
/// carried), and both ways.
const HANDS: [(&str, (f64, f64, f64)); 5] = [
    ("0.6 mm", (0.0006, 0.0, 0.0)),
    ("dust", (4e-7, -3e-7, 0.0)),
    ("0.3 m", (0.21, -0.2142, 0.0)),
    ("depth", (0.0, 0.0, 0.2)),
    ("both ways", (-0.12, 0.07, -0.15)),
];

/// A different hand move for the other join station, when both carry one.
fn second(hand: (f64, f64, f64)) -> (f64, f64, f64) {
    (0.5 * hand.1 - 0.0003, -0.5 * hand.0, -0.5 * hand.2)
}

/// Every pairing of the two ends, merged or side by side, either line
/// kept, the side the other line is on free or held by a zero datum, and
/// a hand move on the kept join station, the other one, both (different), or
/// both the same (a split's), of each size in [`HANDS`]: every combine is
/// what its plan says (`check_combined`: the side that moves moves by the
/// plan's shift with what is merged to it, what is merged at the join station
/// whose hand move is dropped moves with the joined station, merged stations
/// stay one point, a reopen moves nothing), the in-session repair moves
/// nothing, the joined station keeps the fixed side's hand move, and no shot
/// changes.
#[test]
fn hand_moves_on_the_join_for_every_pairing() {
    let mut combined = 0;
    for k_end in [LineEnd::Start, LineEnd::End] {
        for o_end in [LineEnd::Start, LineEnd::End] {
            for merged in [false, true] {
                for held in [false, true] {
                    for (size, hand) in HANDS {
                        for (on, hand_k, hand_o) in [
                            ("K", hand, NONE),
                            ("O", NONE, hand),
                            ("both", hand, second(hand)),
                            ("same", hand, hand),
                        ] {
                            for keep_k in [true, false] {
                                let what = format!(
                                    "K {k_end:?}, O {o_end:?}, merged {merged}, held {held}, \
                                     {size} on {on}, keep K {keep_k}"
                                );
                                let mut join = join(k_end, o_end, merged, hand_k, hand_o);
                                if held {
                                    hold(&mut join.project, StationRef::new(join.t, 1));
                                }
                                check_one(&join, keep_k, hand_k, hand_o, &what);
                                combined += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(combined, 640);
}

/// One combine of [`hand_moves_on_the_join_for_every_pairing`].
fn check_one(
    join: &Fixture,
    keep_k: bool,
    hand_k: (f64, f64, f64),
    hand_o: (f64, f64, f64),
    what: &str,
) {
    let project = &join.project;
    let (kept, other, ends) = if keep_k {
        (
            join.k,
            join.o,
            CombineJoinEnds {
                kept: join.k_end,
                other: join.o_end,
            },
        )
    } else {
        (
            join.o,
            join.k,
            CombineJoinEnds {
                kept: join.o_end,
                other: join.k_end,
            },
        )
    };
    let candidate = project
        .combine_candidate_joining(kept, other, Some(ends))
        .unwrap();
    assert_eq!(candidate.blocked_reason, None, "{what}");
    let plan = if candidate.line_a == kept {
        &candidate.plan_keeping_a
    } else {
        &candidate.plan_keeping_b
    };
    // What moves with the joined station: the third line merged at the
    // join station whose hand move is dropped, when the two hand moves
    // differ horizontally past dust on a join that is not merged.
    let dropped_at = if plan.moving_line == Some(kept) {
        if keep_k {
            join.join_k
        } else {
            join.join_o
        }
    } else if keep_k {
        join.join_o
    } else {
        join.join_k
    };
    let (fixed_hand, dropped_hand) = if dropped_at == join.join_k {
        (hand_o, hand_k)
    } else {
        (hand_k, hand_o)
    };
    let dust = Project::OFFSET_DUST_M;
    let carried = !candidate.merged
        && ((hand_k.0 - hand_o.0).abs() > dust || (hand_k.1 - hand_o.1).abs() > dust);
    let branch = if dropped_at == join.join_o {
        join.t
    } else {
        join.u
    };
    let want = carried.then(|| (dropped_at, vec![branch]));
    let got = plan
        .moved_with_joined_station
        .as_ref()
        .map(|j| (j.station, j.lines.clone()));
    assert_eq!(got, want, "{what}");
    let difference = {
        let d = (
            fixed_hand.0 - dropped_hand.0,
            fixed_hand.1 - dropped_hand.1,
            fixed_hand.2 - dropped_hand.2,
        );
        (d.0 * d.0 + d.1 * d.1 + d.2 * d.2).sqrt()
    };
    // The old note, which the dialog shows for a join that is not merged.
    assert_eq!(
        plan.join_offset_change_m.is_some(),
        !candidate.merged && difference > 0.001,
        "{what}: {:?}",
        plan.join_offset_change_m
    );

    let mut after = project.clone();
    after
        .combine_lines(kept, other, &options(Some(ends)))
        .unwrap();
    check_combined(project, &after, &candidate, kept, what, &[]);
    // T and U on the joined station, wherever it is in the combined line.
    let joined = after
        .station_merges
        .iter()
        .find_map(|m| {
            let (a, b) = (
                after.resolve_key(&m.station_a)?,
                after.resolve_key(&m.station_b)?,
            );
            match (a.line_id == join.t, b.line_id == join.t) {
                (true, false) => Some(b),
                (false, true) => Some(a),
                _ => None,
            }
        })
        .unwrap();
    for branch in [join.t, join.u] {
        let apart = dist(xy(&after, joined), xy(&after, StationRef::new(branch, 0)));
        assert!(
            apart < 2.0 * Project::OFFSET_DUST_M,
            "{what}: line {branch} is {apart:.7} m off the joined station"
        );
    }
    let mut repaired = after.clone();
    assert_eq!(
        repaired.realign_merges(),
        0,
        "{what}: the repair fixed a merge"
    );
    assert_eq!(repaired.line_offsets, after.line_offsets, "{what}");
    // The joined station keeps the fixed side's hand move, and nothing else
    // carries one.
    let kept_hand: Vec<(f64, f64, f64)> = after.station_offsets.values().copied().collect();
    let want_hand = if fixed_hand == NONE {
        vec![]
    } else {
        vec![fixed_hand]
    };
    assert_eq!(kept_hand, want_hand, "{what}");
    assert_eq!(
        after.station_offset_for_ref(joined),
        (fixed_hand != NONE).then_some(fixed_hand),
        "{what}"
    );
    // Depth: the joined station, T and U are one point and read the average
    // of the four depth readings there (the shot on each side of the joined
    // station, T's and U's first); the joined station adds the hand move in
    // depth it keeps, the fixed side's, and T and U have none.
    let readings = [
        join.join_k,
        join.join_o,
        StationRef::new(join.t, 0),
        StationRef::new(join.u, 0),
    ]
    .map(|at| project.compute_station_depth(&[at]));
    let mean = readings.iter().sum::<f64>() / 4.0;
    for (at, want) in [
        (joined, mean + fixed_hand.2),
        (StationRef::new(join.t, 0), mean),
        (StationRef::new(join.u, 0), mean),
    ] {
        let now = depth(&after, at);
        assert!(
            (now - want).abs() < 1e-9,
            "{what}: {at:?} at depth {now}, not {want}"
        );
    }
    // No shot changes: the combined line's lengths are the two lines'.
    let mut lengths: Vec<f64> = after
        .get_line(kept)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.length)
        .collect();
    let mut was: Vec<f64> = [join.k, join.o]
        .iter()
        .flat_map(|id| {
            project
                .get_line(*id)
                .unwrap()
                .shots
                .iter()
                .map(|s| s.length)
        })
        .collect();
    lengths.sort_by(f64::total_cmp);
    was.sort_by(f64::total_cmp);
    assert_eq!(lengths, was, "{what}");
}

/// The hand moves differ but nothing is merged at the join station whose hand
/// move the joined station drops (O's start): U is merged at K's end, which
/// keeps its 0.3 m. Nothing moves, as before #755: O's end lands on the
/// joined station (the plan's "O's end moves 0.30 m"), U stays on it, and a
/// snap from O to Z (which also runs along O) stays snapped, as no side
/// moves.
#[test]
fn nothing_merged_at_the_dropped_join_station_moves_nothing() {
    let mut join = join(LineEnd::End, LineEnd::Start, false, (0.3, 0.0, 0.0), NONE);
    // Only U, at K's end: take T off O's start.
    let t = join.t;
    join.project
        .station_merges
        .retain(|m| m.station_a.line_id != t && m.station_b.line_id != t);
    // Z runs along O's second shot: merged at O's far end, snapped at O's
    // station 1 (a loop snap, not Z's only tie).
    let z = add(&mut join.project, "Z", vec![deep(300.0, 6.0, 3.5, 4.5)]);
    let o1 = StationRef::new(join.o, 1);
    let at = xy(&join.project, o1);
    place(&mut join.project, z, 0, at);
    join.project
        .merge_stations(StationRef::new(join.o, 2), StationRef::new(z, 1))
        .unwrap();
    let snap = LoopClosure {
        id: 90,
        station_a: join.project.key_for_ref(o1).unwrap(),
        station_b: join.project.key_for_ref(StationRef::new(z, 0)).unwrap(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Move,
        is_aligned: true,
    };
    join.project.loop_closures.push(snap);
    let (k, o) = (join.k, join.o);
    let ends = CombineJoinEnds {
        kept: LineEnd::End,
        other: LineEnd::Start,
    };
    let candidate = join
        .project
        .combine_candidate_joining(k, o, Some(ends))
        .unwrap();
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moving_line, None);
    assert!(plan.moved_lines.is_empty());
    assert!(plan.torn_ties.is_empty(), "{:?}", plan.torn_ties);
    assert_eq!(plan.moved_with_joined_station, None);
    let change = plan.join_offset_change_m.unwrap();
    assert!((change - 0.3).abs() < 1e-12, "{change}");
    let mut after = join.project.clone();
    after.combine_lines(k, o, &options(Some(ends))).unwrap();
    check_combined(
        &join.project,
        &after,
        &candidate,
        k,
        "dropped, unmerged",
        &[],
    );
    assert!(
        after.loop_closures.iter().all(|c| c.is_aligned),
        "the snap stays"
    );
    let mut repaired = after.clone();
    assert_eq!(repaired.realign_merges(), 0);
}

/// The issue's shape with a line C merged off T (T's far end) and Z
/// merged off C, Z's far end snapped to O's station 1: T, C and Z move with
/// the joined station by the 0.3 m while O stays (#755), so the snap between
/// Z and O can no longer hold and becomes an ordinary loop closure. Nothing
/// else tears it: Z is on O's side, and O moves by no chain gap.
#[test]
fn a_snap_between_what_moves_with_the_joined_station_and_its_side_tears() {
    let mut project = Project::new("Apart".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let o = add(&mut project, "O", vec![shot(90.0, 10.0)]);
    let t = add(&mut project, "T", vec![shot(0.0, 5.0)]);
    let c = add(&mut project, "C", vec![shot(90.0, 9.7)]);
    let z = add(&mut project, "Z", vec![shot(180.0, 5.0)]);
    project.set_line_offset(o, (10.0, 0.0));
    hand_move(&mut project, StationRef::new(o, 0), (0.3, 0.0, 0.0));
    let chain = [
        (t, StationRef::new(o, 0)),
        (c, StationRef::new(t, 1)),
        (z, StationRef::new(c, 1)),
    ];
    for (branch, at) in chain {
        let shown = xy(&project, at);
        place(&mut project, branch, 0, shown);
        project
            .merge_stations(at, StationRef::new(branch, 0))
            .unwrap();
    }
    let (z1, o1) = (StationRef::new(z, 1), StationRef::new(o, 1));
    assert!(dist(xy(&project, z1), xy(&project, o1)) < 1e-9, "setup");
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: project.key_for_ref(z1).unwrap(),
        station_b: project.key_for_ref(o1).unwrap(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Move,
        is_aligned: true,
    });
    project.next_closure_id = 2;
    let candidate = project.combine_candidate_for_lines(k, o).unwrap();
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moved_lines, vec![o]);
    let joined = plan.moved_with_joined_station.clone().unwrap();
    assert_eq!(joined.lines, vec![t, c, z], "T and what hangs off it");
    assert!((joined.shift_m - 0.3).abs() < 1e-12);
    let torn: Vec<_> = plan
        .torn_ties
        .iter()
        .map(|tie| (tie.kind, tie.reason, tie.station_a, tie.station_b))
        .collect();
    assert_eq!(
        torn,
        vec![(TornTieKind::SnappedClosure, TornTieReason::Loop, z1, o1)]
    );

    let mut after = project.clone();
    after.combine_lines(k, o, &options(None)).unwrap();
    check_combined(&project, &after, &candidate, k, "apart", &[]);
    for id in [t, c, z] {
        for i in 0..=1 {
            let at = StationRef::new(id, i);
            let (was, now) = (xy(&project, at), xy(&after, at));
            assert!(dist(now, (was.0 - 0.3, was.1)) < 1e-9, "{at:?} at {now:?}");
        }
    }
    let closure = &after.loop_closures[0];
    assert!(!closure.is_aligned, "the snap is an ordinary closure now");
    assert!((closure.horizontal_misclosure - 0.3).abs() < 1e-9);
    let mut repaired = after.clone();
    assert_eq!(repaired.realign_merges(), 0);
    let mut reopened = after.clone();
    reopened.redrive_offsets_after_load();
    for (station, was) in every_station(&after) {
        assert!(
            dist(xy(&reopened, station), was) < 1e-9,
            "the reopen moved {station:?}"
        );
    }
}

/// The dust edge (#755): the chain gap 0.9 um east, and O's start moved by
/// hand so the two join stations as shown are 1.8 um apart (past the 1 um
/// float dust the offset solve corrects nothing below), or 0.9 um apart the
/// other way (within it). Past dust, T moves with the joined station and
/// lands on it exactly; within it nothing moves and T stays within the dust,
/// as merged ends do. Either way a reopen and the repair move nothing.
#[test]
fn hand_moves_at_the_float_dust_edge_leave_nothing_to_heal() {
    let dust = Project::OFFSET_DUST_M;
    for (what, hand, carried) in [
        ("past dust", -0.9 * dust, true),
        ("within", 1.8 * dust, false),
    ] {
        let mut project = Project::new("Dust".to_string(), None);
        let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
        let o = add(&mut project, "O", vec![shot(90.0, 10.0)]);
        let t = add(&mut project, "T", vec![shot(0.0, 5.0)]);
        project.set_line_offset(o, (10.0 - 0.9 * dust, 0.0));
        let o0 = StationRef::new(o, 0);
        hand_move(&mut project, o0, (hand, 0.0, 0.0));
        let shown = xy(&project, o0);
        place(&mut project, t, 0, shown);
        project.merge_stations(o0, StationRef::new(t, 0)).unwrap();
        let candidate = project.combine_candidate_for_lines(k, o).unwrap();
        let plan = &candidate.plan_keeping_a;
        assert_eq!(
            plan.moved_with_joined_station
                .as_ref()
                .map(|j| j.lines.clone()),
            carried.then(|| vec![t]),
            "{what}"
        );
        let mut after = project.clone();
        after.combine_lines(k, o, &options(None)).unwrap();
        check_combined(&project, &after, &candidate, k, what, &[]);
        let (joined, t0) = (
            xy(&after, StationRef::new(k, 1)),
            xy(&after, StationRef::new(t, 0)),
        );
        if carried {
            assert!(
                dist(joined, t0) < 1e-12,
                "{what}: T is {:e} m off",
                dist(joined, t0)
            );
        } else {
            assert!(
                (joined.0 - t0.0).abs() <= dust && (joined.1 - t0.1).abs() <= dust,
                "{what}: T is {:?} off",
                (joined.0 - t0.0, joined.1 - t0.1)
            );
        }
        let mut repaired = after.clone();
        assert_eq!(repaired.realign_merges(), 0, "{what}");
        let mut reopened = after.clone();
        reopened.redrive_offsets_after_load();
        for (station, was) in every_station(&after) {
            assert!(
                dist(xy(&reopened, station), was) < 1e-12,
                "{what}: the reopen moved {station:?}"
            );
        }
    }
}

/// A merge loop, which only an older or hand-edited file holds
/// (`merge_stations` refuses one, and no load heals it): T runs from O's
/// start, moved 0.3 m east by hand, to O's far end, and is merged at both.
/// T cannot move with the joined station and stay on O's far end, so it
/// stays with O, and its merge at the join becomes a loop closure carrying
/// the 0.3 m: no merge is left open for the repair or a reopen.
#[test]
fn a_merge_loop_at_a_hand_moved_join_tears_there_instead_of_pulling_apart() {
    let mut project = Project::new("Merge loop".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0)]);
    let o = add(&mut project, "O", vec![shot(90.0, 10.0), shot(90.0, 10.0)]);
    let t = add(&mut project, "T", vec![shot(90.0, 19.7)]);
    project.set_line_offset(o, (10.0, 0.0));
    let o0 = StationRef::new(o, 0);
    hand_move(&mut project, o0, (0.3, 0.0, 0.0));
    let shown = xy(&project, o0);
    place(&mut project, t, 0, shown);
    project.merge_stations(o0, StationRef::new(t, 0)).unwrap();
    let (t1, o2) = (StationRef::new(t, 1), StationRef::new(o, 2));
    assert!(dist(xy(&project, t1), xy(&project, o2)) < 1e-9, "setup");
    assert!(
        project.merge_stations(t1, o2).is_err(),
        "the app refuses the loop"
    );
    project.record_station_merge(StationMerge {
        station_a: project.key_for_ref(t1).unwrap(),
        station_b: project.key_for_ref(o2).unwrap(),
        merged_at: Utc::now(),
    });
    let mut loaded = project.clone();
    assert_eq!(
        loaded.redrive_offsets_after_load(),
        0,
        "a reload keeps the loop"
    );
    assert_eq!(loaded.station_merges.len(), 2);

    let candidate = project.combine_candidate_for_lines(k, o).unwrap();
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moved_with_joined_station, None, "T stays with O");
    let torn: Vec<_> = plan
        .torn_ties
        .iter()
        .map(|tie| (tie.kind, tie.reason, tie.station_a, tie.station_b))
        .collect();
    assert_eq!(
        torn,
        vec![(
            TornTieKind::Merge,
            TornTieReason::Loop,
            o0,
            StationRef::new(t, 0)
        )]
    );
    let mut after = project.clone();
    after.combine_lines(k, o, &options(None)).unwrap();
    check_combined(&project, &after, &candidate, k, "merge loop", &[]);
    assert!(dist(xy(&after, t1), xy(&after, StationRef::new(k, 3))) < 1e-9);
    let closure = after.loop_closures.last().unwrap();
    assert!((closure.horizontal_misclosure - 0.3).abs() < 1e-9);
    let mut repaired = after.clone();
    assert_eq!(repaired.realign_merges(), 0);
    let mut reopened = after.clone();
    reopened.redrive_offsets_after_load();
    for (station, was) in every_station(&after) {
        assert!(
            dist(xy(&reopened, station), was) < 1e-9,
            "the reopen moved {station:?}"
        );
    }
}

/// A loop closure from the join station whose hand move is dropped (O's
/// start, 0.3 m) to K's start, and one from T's far end (T moving with the
/// joined station) to K's station 1: both ends of each move apart, the
/// first also when O does not move at all (nothing merged at O's start, so
/// nothing moves). After the combine every closure's figures are what a
/// fresh measurement gives.
#[test]
fn closures_whose_ends_the_hand_move_moves_apart_are_measured_again() {
    for branch in [false, true] {
        let mut project = Project::new("Measure".to_string(), None);
        let k = add(&mut project, "K", vec![shot(90.0, 5.0), shot(90.0, 5.0)]);
        let o = add(&mut project, "O", vec![shot(90.0, 10.0)]);
        project.set_line_offset(o, (10.0, 0.0));
        let o0 = StationRef::new(o, 0);
        hand_move(&mut project, o0, (0.3, 0.0, 0.0));
        let mut ends = vec![(o0, StationRef::new(k, 0))];
        if branch {
            let t = add(&mut project, "T", vec![shot(0.0, 5.0)]);
            let shown = xy(&project, o0);
            place(&mut project, t, 0, shown);
            project.merge_stations(o0, StationRef::new(t, 0)).unwrap();
            ends.push((StationRef::new(t, 1), StationRef::new(k, 1)));
        }
        // Planted as a file holds them (`create_loop_closure` wants the two
        // stations connected first), measured as they are.
        for (id, (a, b)) in ends.into_iter().enumerate() {
            let m = project.compute_closure_metrics(a, b).unwrap();
            project.loop_closures.push(LoopClosure {
                id: id as u64 + 1,
                station_a: project.key_for_ref(a).unwrap(),
                station_b: project.key_for_ref(b).unwrap(),
                path_length: m.path_length,
                misclosure_distance: m.misclosure_distance,
                error_percentage: m.error_percentage,
                horizontal_misclosure: m.horizontal_misclosure,
                vertical_misclosure: m.vertical_misclosure,
                strategy: ClosureStrategy::Batch,
                is_aligned: false,
            });
        }
        project.next_closure_id = 3;
        let mut after = project.clone();
        after.combine_lines(k, o, &options(None)).unwrap();
        // Measured as the combine measures: with no closure to run through.
        let mut bare = after.clone();
        bare.loop_closures.clear();
        for closure in &after.loop_closures {
            let (a, b) = (
                after.resolve_key(&closure.station_a).unwrap(),
                after.resolve_key(&closure.station_b).unwrap(),
            );
            let fresh = bare.compute_closure_metrics(a, b).unwrap();
            for (stored, want, field) in [
                (closure.path_length, fresh.path_length, "path length"),
                (
                    closure.misclosure_distance,
                    fresh.misclosure_distance,
                    "misclosure",
                ),
                (
                    closure.horizontal_misclosure,
                    fresh.horizontal_misclosure,
                    "horizontal misclosure",
                ),
                (
                    closure.vertical_misclosure,
                    fresh.vertical_misclosure,
                    "vertical misclosure",
                ),
            ] {
                assert!(
                    (stored - want).abs() < 1e-9,
                    "branch {branch}: {a:?}-{b:?} {field} {stored}, measured {want}"
                );
            }
        }
    }
}

/// The Closures Computed View across a combine at two join stations moved
/// by hand by different amounts, with a branch merged at one, every expected
/// position worked out here rather than read off the plan (#759).
///
/// K runs 20 m east from the origin and holds the view (lowest id, station
/// 0); L closes a loop on K's first shot (north 1 m, east 10.2 m, south 1 m,
/// a loop closure to K:1), so the view solves K's survey. O runs 8 m north
/// from K's end and T 6 m east from O's start, merged there as shown. K's
/// end is moved (0.2, -0.1) by hand and O's start (-0.15, 0.25). Either O's
/// start is side by side with K's end (their chain positions on one point,
/// unmerged) or merged to it (one point as shown, so their chain positions
/// are the hand moves' difference apart).
///
/// Keeping K, the rules say: K's side holds the view's station, so O's side
/// moves; the joined station keeps K's hand move; O's stations follow O's
/// chain from K's end. Side by side, the chain gap is 0, so O's far end
/// stays, and T, merged at the dropped station, moves with the joined
/// station by the shown gap. Merged, O moves by the chain gap and T, one
/// point with the joined station, stays. The view then holds K and L where
/// it held them, shows the joined station where it showed K's end, hangs O
/// off the joined station's adjusted chain position, and keeps T on it.
#[test]
fn a_combine_at_two_hand_moved_join_stations_moves_the_view_as_worked_out() {
    let (hand_k, hand_o) = ((0.2, -0.1), (-0.15, 0.25));
    for merged in [false, true] {
        let what = if merged {
            "merged join"
        } else {
            "side by side"
        };
        let mut project = Project::new("Two hand moves".to_string(), None);
        let k = add(&mut project, "K", vec![shot(90.0, 10.0), shot(90.0, 10.0)]);
        let l = add(
            &mut project,
            "L",
            vec![shot(0.0, 1.0), shot(90.0, 10.2), shot(180.0, 1.0)],
        );
        let o = add(&mut project, "O", vec![shot(0.0, 8.0)]);
        let t = add(&mut project, "T", vec![shot(90.0, 6.0)]);
        project
            .merge_stations(StationRef::new(k, 0), StationRef::new(l, 0))
            .unwrap();
        project
            .create_loop_closure(
                StationRef::new(l, 3),
                StationRef::new(k, 1),
                ClosureStrategy::Batch,
            )
            .unwrap();
        let (k_end, o_start) = (StationRef::new(k, 2), StationRef::new(o, 0));
        hand_move(&mut project, k_end, (hand_k.0, hand_k.1, 0.0));
        hand_move(&mut project, o_start, (hand_o.0, hand_o.1, 0.0));
        let shown_k = (20.0 + hand_k.0, hand_k.1);
        assert!(dist(xy(&project, k_end), shown_k) < 1e-9, "{what}");
        if merged {
            place(&mut project, o, 0, shown_k);
            project.merge_stations(k_end, o_start).unwrap();
        } else {
            place(&mut project, o, 0, (20.0 + hand_o.0, hand_o.1));
        }
        let shown_o = xy(&project, o_start);
        place(&mut project, t, 0, shown_o);
        project
            .merge_stations(o_start, StationRef::new(t, 0))
            .unwrap();

        let before = adjusted(&project);
        // The view before: K's survey solved, held at K's start.
        assert!(
            dist(before[&StationRef::new(k, 0)], (0.0, 0.0)) < 1e-9,
            "{what}"
        );
        let k1 = before[&StationRef::new(k, 1)];
        assert!(
            dist(k1, (10.0, 0.0)) > 1e-3,
            "{what}: the loop is not adjusted"
        );
        // K's end is a spur off K:1: its adjusted chain is K:1's plus 10 m east.
        let joined_shown = (k1.0 + 10.0 + hand_k.0, k1.1 + hand_k.1);
        assert!(dist(before[&k_end], joined_shown) < 1e-9, "{what}");

        let candidate = project.combine_candidate_for_lines(k, o).unwrap();
        let plan = &candidate.plan_keeping_a;
        assert_eq!(candidate.line_a, k, "{what}");
        assert_eq!(plan.moving_line, Some(o), "{what}: O's side moves");
        let mut after = project.clone();
        after.combine_lines(k, o, &options(None)).unwrap();
        check_combined(&project, &after, &candidate, k, what, &[]);

        // The Raw Data View, worked out.
        let raw = |at: StationRef| xy(&after, at);
        let (o_far, t_start, t_far) = (
            StationRef::new(k, 3),
            StationRef::new(t, 0),
            StationRef::new(t, 1),
        );
        let t_at = if merged { shown_o } else { shown_k };
        for (at, want) in [
            (StationRef::new(k, 0), (0.0, 0.0)),
            (StationRef::new(k, 1), (10.0, 0.0)),
            (k_end, shown_k),
            (o_far, (20.0, 8.0)),
            (t_start, t_at),
            (t_far, (t_at.0 + 6.0, t_at.1)),
        ] {
            assert!(
                dist(raw(at), want) < 1e-9,
                "{what}: {at:?} is at {:?} in the Raw Data View, not {want:?}",
                raw(at)
            );
        }

        // The Closures Computed View, worked out from the one before.
        let now = adjusted(&after);
        let joined_chain = (k1.0 + 10.0, k1.1);
        let mut want: Vec<(StationRef, (f64, f64))> = vec![
            (k_end, joined_shown),
            (o_far, (joined_chain.0, joined_chain.1 + 8.0)),
            (t_start, joined_shown),
            (t_far, (joined_shown.0 + 6.0, joined_shown.1)),
        ];
        for i in 0..=1 {
            want.push((StationRef::new(k, i), before[&StationRef::new(k, i)]));
        }
        for i in 0..=3 {
            want.push((StationRef::new(l, i), before[&StationRef::new(l, i)]));
        }
        for (at, want) in want {
            assert!(
                dist(now[&at], want) < 1e-9,
                "{what}: {at:?} is at {:?} in the Closures Computed View, {:.6} m off {want:?}",
                now[&at],
                dist(now[&at], want)
            );
        }
    }
}
