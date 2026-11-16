//! Combine Sections where a join station was moved by hand (#755): a manual
//! station offset, as an older or hand-edited file carries it, on the kept
//! join station, the other one, or both, horizontal and in depth, with a
//! section merged at each join station. The joined station keeps the fixed
//! side's hand move, so the other join station lands on it by the gap as
//! shown: what is merged there moves with the joined station, by that gap,
//! not with its side (`combine_plan.rs`). Merged stations stay one point,
//! the plan is exact, the in-session repair and a reopen move nothing, and
//! no shot changes.

use super::combine_closure_tests::{add, check_combined, dist, options, shot, xy};
use crate::{
    CombineJoinEnds, CombineJoinKind, GpsCoordinates, Project, SectionEnd, SectionId, StationRef,
    ZeroDatum,
};

const NONE: (f64, f64, f64) = (0.0, 0.0, 0.0);

/// Two sections meeting at a join, with a section merged at each join
/// station.
struct Fixture {
    project: Project,
    k: SectionId,
    o: SectionId,
    /// Merged at O's join station.
    t: SectionId,
    /// Merged at K's join station.
    u: SectionId,
    k_end: SectionEnd,
    o_end: SectionEnd,
    join_k: StationRef,
    join_o: StationRef,
}

fn end_index(project: &Project, id: SectionId, end: SectionEnd) -> u32 {
    match end {
        SectionEnd::Start => 0,
        SectionEnd::End => project.get_section(id).unwrap().shots.len() as u32,
    }
}

/// Move `id` so its station `index` sits exactly on `target` as shown.
fn place(project: &mut Project, id: SectionId, index: u32, target: (f64, f64)) {
    let now = xy(project, StationRef::new(id, index));
    project.adjust_section_offset(id, target.0 - now.0, target.1 - now.1);
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
    k_end: SectionEnd,
    o_end: SectionEnd,
    merged: bool,
    hand_k: (f64, f64, f64),
    hand_o: (f64, f64, f64),
) -> Fixture {
    let mut project = Project::new("Hand moved".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0), shot(70.0, 5.0)]);
    let o = add(&mut project, "O", vec![shot(0.0, 4.0), shot(300.0, 6.0)]);
    project.set_section_offset(k, (100.0, -40.0));
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
    let t = add(&mut project, "T", vec![shot(180.0, 3.0)]);
    let u = add(&mut project, "U", vec![shot(225.0, 2.0)]);
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
    });
}

/// Every station of every section, as shown.
fn every_station(project: &Project) -> Vec<(StationRef, (f64, f64))> {
    project
        .sections
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
    project.set_section_offset(o, (10.0, 0.0));
    hand_move(&mut project, StationRef::new(o, 0), (0.3, 0.0, 0.0));
    project.set_section_offset(t, (10.3, 0.0));
    project
        .merge_stations(StationRef::new(o, 0), StationRef::new(t, 0))
        .unwrap();
    let candidate = project.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.join_kind, CombineJoinKind::CoLocated);
    assert_eq!(candidate.branches, vec![t]);
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moving_section, Some(o));
    assert_eq!(plan.moved_sections, vec![o], "T is not moved by the shift");
    assert!(plan.shift_m < 1e-12, "the chain gap is 0: {}", plan.shift_m);
    let joined = plan.moved_with_joined_station.clone().unwrap();
    assert_eq!(joined.station, StationRef::new(o, 0));
    assert_eq!(joined.sections, vec![t]);
    assert!((joined.shift_m - 0.3).abs() < 1e-12, "{}", joined.shift_m);
    let change = plan.join_offset_change_m.unwrap();
    assert!((change - 0.3).abs() < 1e-12, "{change}");

    let mut after = project.clone();
    after.combine_sections(k, o, &options(None)).unwrap();
    check_combined(&project, &after, &candidate, k, "0.3 m", &[]);
    let at = |p: &Project, s: SectionId, i: u32| xy(p, StationRef::new(s, i));
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
        .get_section(k)
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

/// Every pairing of the two ends, merged or side by side, either section
/// kept, the side the other section is on free or held by a zero datum, and
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
    for k_end in [SectionEnd::Start, SectionEnd::End] {
        for o_end in [SectionEnd::Start, SectionEnd::End] {
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
    let plan = if candidate.section_a == kept {
        &candidate.plan_keeping_a
    } else {
        &candidate.plan_keeping_b
    };
    // What moves with the joined station: the third section merged at the
    // join station whose hand move is dropped, when the two hand moves
    // differ horizontally past dust on a join that is not merged.
    let dropped_at = if plan.moving_section == Some(kept) {
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
        .map(|j| (j.station, j.sections.clone()));
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
        .combine_sections(kept, other, &options(Some(ends)))
        .unwrap();
    check_combined(project, &after, &candidate, kept, what, &[]);
    // T and U on the joined station, wherever it is in the combined section.
    let joined = after
        .station_merges
        .iter()
        .find_map(|m| {
            let (a, b) = (
                after.resolve_key(&m.station_a)?,
                after.resolve_key(&m.station_b)?,
            );
            match (a.section_id == join.t, b.section_id == join.t) {
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
            "{what}: section {branch} is {apart:.7} m off the joined station"
        );
    }
    let mut repaired = after.clone();
    assert_eq!(
        repaired.realign_merges(),
        0,
        "{what}: the repair fixed a merge"
    );
    assert_eq!(repaired.section_offsets, after.section_offsets, "{what}");
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
    // No shot changes: the combined section's lengths are the two sections'.
    let mut lengths: Vec<f64> = after
        .get_section(kept)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.length)
        .collect();
    let mut was: Vec<f64> = [join.k, join.o]
        .iter()
        .flat_map(|id| {
            project
                .get_section(*id)
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
    let mut join = join(
        SectionEnd::End,
        SectionEnd::Start,
        false,
        (0.3, 0.0, 0.0),
        NONE,
    );
    // Only U, at K's end: take T off O's start.
    let t = join.t;
    join.project
        .station_merges
        .retain(|m| m.station_a.section_id != t && m.station_b.section_id != t);
    // Z runs along O's second shot: merged at O's far end, snapped at O's
    // station 1 (a loop snap, not Z's only tie).
    let z = add(&mut join.project, "Z", vec![shot(300.0, 6.0)]);
    let o1 = StationRef::new(join.o, 1);
    let at = xy(&join.project, o1);
    place(&mut join.project, z, 0, at);
    join.project
        .merge_stations(StationRef::new(join.o, 2), StationRef::new(z, 1))
        .unwrap();
    let snap = crate::LoopClosure {
        id: 90,
        station_a: join.project.key_for_ref(o1).unwrap(),
        station_b: join.project.key_for_ref(StationRef::new(z, 0)).unwrap(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: crate::ClosureStrategy::Move,
        is_aligned: true,
    };
    join.project.loop_closures.push(snap);
    let (k, o) = (join.k, join.o);
    let ends = CombineJoinEnds {
        kept: SectionEnd::End,
        other: SectionEnd::Start,
    };
    let candidate = join
        .project
        .combine_candidate_joining(k, o, Some(ends))
        .unwrap();
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moving_section, None);
    assert!(plan.moved_sections.is_empty());
    assert!(plan.torn_ties.is_empty(), "{:?}", plan.torn_ties);
    assert_eq!(plan.moved_with_joined_station, None);
    let change = plan.join_offset_change_m.unwrap();
    assert!((change - 0.3).abs() < 1e-12, "{change}");
    let mut after = join.project.clone();
    after.combine_sections(k, o, &options(Some(ends))).unwrap();
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
