//! Combine Sections at two ends side by side within the tolerance, a fraction
//! of a millimetre apart, or merged ends whose chain positions differ by a
//! hand-moved join station (#740). The combined section hangs the other
//! section's stations off the kept join station, so one side moves by that
//! residual: the plan carries it like a shift, the anchor rule picks the side
//! (a zero datum, the Closures Computed View's anchor), what is merged to the
//! moving side moves along, merged stations stay one point, and a reopen
//! moves nothing. Float dust at the join moves nothing, as before. Each
//! combine is checked against the plan the station menu offered and the
//! closure-adjusted view (`combine_closure_tests::check_combined`).

use super::combine_closure_tests::{add, check_combined, dist, options, shot, xy};
use crate::{
    CombineJoinEnds, CombineJoinKind, CombinePlan, GpsCoordinates, KeptMovesReason, Project,
    SectionEnd, SectionId, StationRef, TornTieKind, TornTieReason, ZeroDatum,
};

/// A residual under the 1 mm tolerance, in both axes: 0.5 mm.
const RESIDUAL: (f64, f64) = (0.0004, -0.0003);

/// K and O meet at `k_end` and `o_end`; N is merged at K's other end and M
/// at O's other end.
struct Pair {
    project: Project,
    k: SectionId,
    o: SectionId,
    n: SectionId,
    m: SectionId,
    k_end: SectionEnd,
    o_end: SectionEnd,
}

fn end_index(project: &Project, id: SectionId, end: SectionEnd) -> u32 {
    match end {
        SectionEnd::Start => 0,
        SectionEnd::End => project.get_section(id).unwrap().shots.len() as u32,
    }
}

fn far(end: SectionEnd) -> SectionEnd {
    match end {
        SectionEnd::Start => SectionEnd::End,
        SectionEnd::End => SectionEnd::Start,
    }
}

/// Move `id` so its station `index` sits exactly on `target`.
fn place(project: &mut Project, id: SectionId, index: u32, target: (f64, f64)) {
    let now = xy(project, StationRef::new(id, index));
    project.adjust_section_offset(id, target.0 - now.0, target.1 - now.1);
}

/// K runs east then east-north-east from (100, -40); O runs north then
/// north-west, placed so its `o_end` sits `residual` (metres, x and y) from
/// K's `k_end`, unmerged. N (one shot north) is merged at K's other end and M
/// (one shot east) at O's other end, so each side carries a section that has
/// to move with it. K has the lowest id.
fn side_by_side(k_end: SectionEnd, o_end: SectionEnd, residual: (f64, f64)) -> Pair {
    let mut project = Project::new("Side by side".to_string(), None);
    let k = add(&mut project, "K", vec![shot(90.0, 10.0), shot(80.0, 5.0)]);
    let o = add(&mut project, "O", vec![shot(0.0, 4.0), shot(300.0, 6.0)]);
    let n = add(&mut project, "N", vec![shot(0.0, 3.0)]);
    let m = add(&mut project, "M", vec![shot(90.0, 3.0)]);
    project.set_section_offset(k, (100.0, -40.0));
    let at = xy(&project, StationRef::new(k, end_index(&project, k, k_end)));
    let o_join = end_index(&project, o, o_end);
    place(
        &mut project,
        o,
        o_join,
        (at.0 + residual.0, at.1 + residual.1),
    );
    for (section, end, branch) in [(k, far(k_end), n), (o, far(o_end), m)] {
        let tip = StationRef::new(section, end_index(&project, section, end));
        let at = xy(&project, tip);
        place(&mut project, branch, 0, at);
        project
            .merge_stations(tip, StationRef::new(branch, 0))
            .unwrap();
    }
    Pair {
        project,
        k,
        o,
        n,
        m,
        k_end,
        o_end,
    }
}

/// Hold `at`'s side with a zero datum where it is (`set_zero_datum` would
/// move a second datum's side onto its GPS position, and the residual with
/// it).
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

/// Combine `pair` keeping `kept`, at the pair's join, checked against every
/// invariant `check_combined` holds a combine to (the plan says what moves,
/// merged stations stay one point, the closure-adjusted view stays, a reload
/// changes nothing). Returns the plan the station menu offered and the
/// combined project.
fn combine(pair: &Pair, kept: SectionId, what: &str) -> (CombinePlan, Project) {
    let (other, join) = if kept == pair.k {
        (
            pair.o,
            CombineJoinEnds {
                kept: pair.k_end,
                other: pair.o_end,
            },
        )
    } else {
        (
            pair.k,
            CombineJoinEnds {
                kept: pair.o_end,
                other: pair.k_end,
            },
        )
    };
    let candidate = pair
        .project
        .combine_candidate_joining(kept, other, Some(join))
        .unwrap();
    assert_eq!(candidate.blocked_reason, None, "{what}");
    let plan = if candidate.section_a == kept {
        candidate.plan_keeping_a.clone()
    } else {
        candidate.plan_keeping_b.clone()
    };
    let mut after = pair.project.clone();
    after
        .combine_sections(kept, other, &options(Some(join)))
        .unwrap();
    check_combined(&pair.project, &after, &candidate, kept, what, &[]);
    (plan, after)
}

/// How far `id`'s station 0 moved from `before` to `after` (`id` not one of
/// the two combined sections).
fn moved_by(before: &Project, after: &Project, id: SectionId) -> f64 {
    let at = StationRef::new(id, 0);
    dist(xy(before, at), xy(after, at))
}

/// Every pairing of the two ends, either section kept: the other section
/// moves by the residual and takes the section merged to it along; the kept
/// side, and what is merged to it, stays exactly where it was.
#[test]
fn a_residual_moves_the_other_side_with_what_is_merged_to_it() {
    let residual = RESIDUAL.0.hypot(RESIDUAL.1);
    for k_end in [SectionEnd::Start, SectionEnd::End] {
        for o_end in [SectionEnd::Start, SectionEnd::End] {
            for keep_k in [true, false] {
                let what = format!("K {k_end:?}, O {o_end:?}, keep K {keep_k}");
                let pair = side_by_side(k_end, o_end, RESIDUAL);
                let (kept, other, stays, follows) = if keep_k {
                    (pair.k, pair.o, pair.n, pair.m)
                } else {
                    (pair.o, pair.k, pair.m, pair.n)
                };
                let (plan, after) = combine(&pair, kept, &what);
                assert_eq!(plan.moving_section, Some(other), "{what}");
                assert_eq!(plan.moved_sections, vec![other, follows], "{what}");
                assert_eq!(plan.kept_moves_because, None, "{what}");
                assert!(plan.torn_ties.is_empty(), "{what}");
                assert!((plan.shift_m - residual).abs() < 1e-9, "{what}");
                assert!(
                    (moved_by(&pair.project, &after, follows) - residual).abs() < 1e-9,
                    "{what}: the section merged to the moving side follows it"
                );
                assert!(
                    moved_by(&pair.project, &after, stays) < 1e-12,
                    "{what}: the kept side's stays"
                );
            }
        }
    }
}

/// A zero datum holds the side it is on: on O's side (M, merged to O, or O
/// itself) the kept K moves with N; on K's side (N) O moves with M, as
/// without a datum. The plan says why the kept side moves.
#[test]
fn a_zero_datum_decides_which_side_a_residual_moves() {
    let base = side_by_side(SectionEnd::End, SectionEnd::Start, RESIDUAL);
    let cases = [
        ("M", StationRef::new(base.m, 1), base.k),
        ("O", StationRef::new(base.o, 2), base.k),
        ("N", StationRef::new(base.n, 1), base.o),
    ];
    for (name, datum, moving) in cases {
        let what = format!("datum on {name}");
        let mut pair = side_by_side(SectionEnd::End, SectionEnd::Start, RESIDUAL);
        hold(&mut pair.project, datum);
        let (plan, after) = combine(&pair, pair.k, &what);
        let (follows, stays) = if moving == pair.k {
            (pair.n, pair.m)
        } else {
            (pair.m, pair.n)
        };
        assert_eq!(plan.moving_section, Some(moving), "{what}");
        assert_eq!(plan.moved_sections, vec![moving, follows], "{what}");
        assert_eq!(
            plan.kept_moves_because,
            (moving == pair.k).then_some(KeptMovesReason::ZeroDatum),
            "{what}"
        );
        assert!(
            moved_by(&pair.project, &after, stays) < 1e-12,
            "{what}: the held side stays"
        );
    }
}

/// K's side and O's side are one survey already, merged through a one-shot
/// connector C from N's end to M's end, so the combine closes a loop: the
/// merge on it nearest the moving section becomes a loop closure, which the
/// Closures Computed View solves, held at K's side (K has the lowest id).
/// Keeping K, O moves; keeping O, O moves all the same, because moving K
/// would shift the whole adjusted survey by the residual (the
/// `check_combined` oracle compares that view before and after).
#[test]
fn the_adjustment_anchor_decides_which_side_a_residual_moves() {
    for keep_k in [true, false] {
        let what = format!("keep K {keep_k}");
        let mut pair = side_by_side(SectionEnd::End, SectionEnd::Start, RESIDUAL);
        let (from, to) = (
            xy(&pair.project, StationRef::new(pair.n, 1)),
            xy(&pair.project, StationRef::new(pair.m, 1)),
        );
        let azimuth = (to.0 - from.0)
            .atan2(to.1 - from.1)
            .to_degrees()
            .rem_euclid(360.0);
        let c = add(&mut pair.project, "C", vec![shot(azimuth, dist(from, to))]);
        place(&mut pair.project, c, 0, from);
        pair.project
            .merge_stations(StationRef::new(pair.n, 1), StationRef::new(c, 0))
            .unwrap();
        pair.project
            .merge_stations(StationRef::new(pair.m, 1), StationRef::new(c, 1))
            .unwrap();
        let component = pair.project.get_connected_section_ids(pair.k);
        let anchor = pair.project.adjustment_anchor_of(&component).unwrap();
        assert_eq!(anchor.section_id, pair.k, "{what}: held at K's side");

        let kept = if keep_k { pair.k } else { pair.o };
        let (plan, _) = combine(&pair, kept, &what);
        assert_eq!(plan.moving_section, Some(pair.o), "{what}");
        assert_eq!(plan.moved_sections, vec![pair.o], "{what}");
        assert_eq!(
            plan.kept_moves_because,
            (!keep_k).then_some(KeptMovesReason::AdjustmentAnchor),
            "{what}"
        );
        // O's merge to M closes the loop: it becomes the loop's closure.
        assert_eq!(plan.torn_ties.len(), 1, "{what}");
        let torn = plan.torn_ties[0];
        assert_eq!(torn.kind, TornTieKind::Merge, "{what}");
        assert_eq!(torn.reason, TornTieReason::Loop, "{what}");
        assert_eq!(
            [torn.station_a.section_id, torn.station_b.section_id],
            [pair.o, pair.m],
            "{what}"
        );
    }
}

/// Zero datums hold both sides (on N and on M, so neither join section
/// carries one and the combine is allowed): O, which carries none, moves by
/// the residual alone, and its merge to the held M becomes a loop closure
/// (a closure may tie two anchored groups; a merge may not), as it does for
/// a shift past the tolerance.
#[test]
fn both_sides_held_tear_the_moving_sides_merge_for_a_residual() {
    let mut pair = side_by_side(SectionEnd::End, SectionEnd::Start, RESIDUAL);
    hold(&mut pair.project, StationRef::new(pair.n, 1));
    hold(&mut pair.project, StationRef::new(pair.m, 1));
    let (plan, after) = combine(&pair, pair.k, "both held");
    assert_eq!(plan.moving_section, Some(pair.o));
    assert_eq!(plan.moved_sections, vec![pair.o]);
    assert_eq!(plan.kept_moves_because, None);
    assert_eq!(plan.torn_ties.len(), 1);
    let torn = plan.torn_ties[0];
    assert_eq!(torn.reason, TornTieReason::ZeroDatum);
    assert_eq!(
        [torn.station_a.section_id, torn.station_b.section_id],
        [pair.o, pair.m]
    );
    for held in [pair.m, pair.n] {
        assert!(moved_by(&pair.project, &after, held) < 1e-12);
    }
    assert_eq!(after.station_merges.len(), 1, "only N's merge is left");
}

/// Merged ends whose join station was moved by hand (a manual station
/// offset on K's) are one point, but their chain positions differ by that
/// offset: the combined section hangs O's stations off K's chain, so O moves
/// by it, and M, merged to O, moves along. The joined station keeps K's
/// offset and stays where both were; nothing moves on a reopen.
#[test]
fn merged_ends_moved_by_hand_carry_their_residual_too() {
    let mut pair = side_by_side(SectionEnd::End, SectionEnd::Start, (0.0, 0.0));
    let (k, o, m, n) = (pair.k, pair.o, pair.m, pair.n);
    let join_k = StationRef::new(k, 2);
    let key = pair.project.key_for_ref(join_k).unwrap();
    pair.project
        .station_offsets
        .insert(key, (RESIDUAL.0, RESIDUAL.1, 0.0));
    pair.project
        .merge_stations(join_k, StationRef::new(o, 0))
        .unwrap();
    let before = pair.project.clone();
    let joined_at = xy(&before, join_k);
    let candidate = before.combine_candidate_for_sections(k, o).unwrap();
    assert_eq!(candidate.join_kind, CombineJoinKind::Merged);
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moving_section, Some(o));
    assert_eq!(plan.moved_sections, vec![o, m]);
    assert!((plan.shift_m - RESIDUAL.0.hypot(RESIDUAL.1)).abs() < 1e-9);

    let mut after = before.clone();
    after
        .combine_sections(
            k,
            o,
            &options(Some(CombineJoinEnds {
                kept: SectionEnd::End,
                other: SectionEnd::Start,
            })),
        )
        .unwrap();
    assert!(
        dist(xy(&after, join_k), joined_at) < 1e-12,
        "the join stays"
    );
    assert!(moved_by(&before, &after, n) < 1e-12, "K's side stays");
    assert!(
        (moved_by(&before, &after, m) - plan.shift_m).abs() < 1e-9,
        "M follows O"
    );
    // O's far end is K's station 4 now, still merged to M's start.
    assert!(
        dist(
            xy(&after, StationRef::new(k, 4)),
            xy(&after, StationRef::new(m, 0))
        ) < 1e-9
    );
    let mut reopened = after.clone();
    reopened.redrive_offsets_after_load();
    assert_eq!(reopened.section_offsets, after.section_offsets);
}

/// The residual a plan leaves is float dust, under the bound the offset
/// solve corrects nothing below (`OFFSET_DUST_M` in each axis), and for that
/// the combine is exactly what it was before #740: nothing moves but the
/// other section's stations, by the dust; the sections merged to it keep
/// their offsets bit for bit, and a reopen changes no offset. Just past the
/// bound the residual moves a side, with what is merged to it.
#[test]
fn a_residual_of_float_dust_moves_nothing_as_before() {
    let dust = Project::OFFSET_DUST_M;
    for (what, residual) in [
        ("coincident", (0.0, 0.0)),
        ("dust", (0.6 * dust, -0.6 * dust)),
    ] {
        let pair = side_by_side(SectionEnd::End, SectionEnd::Start, residual);
        let (plan, after) = combine(&pair, pair.k, what);
        assert_eq!(plan.moving_section, None, "{what}");
        assert!(plan.moved_sections.is_empty(), "{what}");
        assert_eq!(plan.kept_moves_because, None, "{what}");
        assert_eq!(plan.shift_m.to_bits(), 0, "{what}: no shift");
        assert!(plan.torn_ties.is_empty(), "{what}");
        for id in [pair.k, pair.n, pair.m] {
            let (was, now) = (pair.project.section_offset(id), after.section_offset(id));
            assert_eq!(
                (was.0.to_bits(), was.1.to_bits()),
                (now.0.to_bits(), now.1.to_bits()),
                "{what}: section {id} moved"
            );
        }
        let mut reopened = after.clone();
        reopened.redrive_offsets_after_load();
        assert_eq!(reopened.section_offsets, after.section_offsets, "{what}");
    }
    // Merged ends with no offset between them: one point, nothing to carry.
    let mut pair = side_by_side(SectionEnd::End, SectionEnd::Start, (0.0, 0.0));
    pair.project
        .merge_stations(StationRef::new(pair.k, 2), StationRef::new(pair.o, 0))
        .unwrap();
    let (plan, _) = combine(&pair, pair.k, "merged");
    assert_eq!(plan.moving_section, None, "merged");

    let pair = side_by_side(SectionEnd::End, SectionEnd::Start, (2.0 * dust, 0.0));
    let (plan, _) = combine(&pair, pair.k, "past dust");
    assert_eq!(plan.moving_section, Some(pair.o));
    assert_eq!(plan.moved_sections, vec![pair.o, pair.m]);
}
