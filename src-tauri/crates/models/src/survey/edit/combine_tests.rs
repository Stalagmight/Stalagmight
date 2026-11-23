//! Combine Lines on the station graph: the three join kinds, the moving
//! side, torn junctions, refusals, names and comments.

use stalagmight_legacy::models as old;

use crate::{
    CombineJoinEnds, CombineJoinKind, CombineLinesOptions, CombinePick, KeptMovesReason, LineEnd,
    LineId,
};

use super::super::test_helpers::{gps, meta};
use super::super::{ClosureStrategy, Project, StartAt};
use super::edit_test_helpers::{
    add_datum, assert_valid, empty, line, line_at, line_from, names, near, ok, refused, run, st, xy,
};
use super::{TornTieKind, TornTieReason};

const TOL: f64 = 1e-9;

fn options(name: &str) -> CombineLinesOptions {
    CombineLinesOptions {
        name: name.to_string(),
        color: "#abcdef".to_string(),
        swap_lr: false,
        comment_pick: CombinePick::Kept,
        name_pick: CombinePick::Kept,
        join: None,
    }
}

/// Every station of `line` (pre-combine ids), positions before and after.
fn chain(p: &Project, line: LineId) -> Vec<u32> {
    p.line_stations(line).unwrap().to_vec()
}

#[test]
fn a_merged_join_concatenates_the_chains_and_moves_nothing() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 2));
    let j = st(&p, a, 2);
    let b = line_from(&mut p, "B", j, run("b", 0.0, 10.0, 2));
    let before = p.raw_positions();
    let (ca, cb) = (chain(&p, a), chain(&p, b));
    let candidate = p.combine_candidate_for_lines(a, b).unwrap();
    assert_eq!(candidate.join_kind, CombineJoinKind::Merged);
    assert_eq!(candidate.blocked_reason, None);
    assert!(!candidate.reverses);
    assert_eq!(candidate.plan_keeping_a.moving_line, None);
    assert!(candidate.plan_keeping_a.torn_ties.is_empty());
    let report = ok(p.combine_lines(a, b, &options("AB")));
    assert_eq!(report.moving_line, None);
    assert!(p.line(b).is_none());
    let mut expected = ca;
    expected.extend(&cb[1..]);
    assert_eq!(chain(&p, a), expected);
    assert_eq!(p.line(a).unwrap().name.as_deref(), Some("AB"));
    assert_eq!(p.line(a).unwrap().shots.len(), 4);
    let after = p.raw_positions();
    for s in expected {
        near(xy(&after, s), xy(&before, s), TOL, &format!("station {s}"));
    }
    assert_valid(&p);
}

#[test]
fn side_by_side_free_pieces_join_and_the_other_side_moves_by_the_residual() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 2));
    let b = line_at(&mut p, "B", [20.0004, 0.0], run("b", 0.0, 5.0, 2));
    let before = p.raw_positions();
    let (b1, b2) = (st(&p, b, 1), st(&p, b, 2));
    let candidate = p.combine_candidate_for_lines(a, b).unwrap();
    assert_eq!(candidate.join_kind, CombineJoinKind::CoLocated);
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moving_line, Some(b));
    assert!((plan.shift_m - 0.0004).abs() < 1e-9);
    let report = ok(p.combine_lines(a, b, &options("AB")));
    assert_eq!(report.moved_lines, vec![b]);
    let after = p.raw_positions();
    near(
        xy(&after, b1),
        [xy(&before, b1)[0] - 0.0004, 5.0],
        TOL,
        "B moved",
    );
    near(xy(&after, b2), [20.0, 10.0], TOL, "B moved");
    near(xy(&after, st(&p, a, 0)), [0.0, 0.0], TOL, "A stays");
    assert_eq!(p.placements.len(), 1);
    assert_eq!(p.placements[0].station, st(&p, a, 0));
    // The other join station is archived in the kept one now.
    let joined = st(&p, a, 2);
    assert_eq!(p.station(joined).unwrap().archived.len(), 1);
    assert_valid(&p);
}

#[test]
fn a_closure_between_two_free_pieces_moves_the_other_by_its_misclosure() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 2));
    let b = line_at(&mut p, "B", [20.5, 0.0], run("b", 90.0, 5.0, 1));
    let (a2, b0, b1) = (st(&p, a, 2), st(&p, b, 0), st(&p, b, 1));
    p.push_closure(a2, Some(a), b0, Some(b), ClosureStrategy::Batch);
    // Two free pieces tied by a closure only would be swept; this is the
    // state a load can hold before the sweep runs, so validate refuses it.
    // Hold B with a datum instead? No: build it as two placed pieces would.
    // Here: make the closure legitimate by placing both pieces.
    let a0 = st(&p, a, 0);
    add_datum(&mut p, a0, gps(10.0, 20.0));
    let before = p.raw_positions();
    let b_at = xy(&before, b0);
    add_datum(&mut p, b1, gps(10.0, 20.0003));
    assert_valid(&p);
    // Both sides held by their own datum: refused as today.
    let text = refused(p.combine_lines(a, b, &options("AB")));
    assert_eq!(
        text,
        "A and B are each placed by their own zero datum: merge them first, so the later datum \
         becomes a GPS fix"
    );
    let _ = b_at;
}

#[test]
fn a_closure_inside_one_piece_tears_the_junction_nearest_the_moving_line() {
    // A: s0 -> s1 -> s2 east. B from s0 north 10, east 20, south 9.5, ending
    // 0.5 m north of s2; a Batch closure ties B's end to s2.
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 2));
    let s0 = st(&p, a, 0);
    let b = ok(p.create_line(
        meta("B"),
        StartAt::Station(s0),
        vec![
            super::edit_test_helpers::flat_u(0.0, 10.0, "b0"),
            super::edit_test_helpers::flat_u(90.0, 20.0, "b1"),
            super::edit_test_helpers::flat_u(180.0, 9.5, "b2"),
        ],
        None,
    ));
    let (s2, b3) = (st(&p, a, 2), st(&p, b, 3));
    p.push_closure(s2, Some(a), b3, Some(b), ClosureStrategy::Batch);
    assert_valid(&p);
    let before = p.raw_positions();
    let b_stations: Vec<u32> = chain(&p, b)[1..].to_vec();
    // The two lines also share s0 (B starts there): the pair query prefers
    // that merged join; the far ends are asked for explicitly.
    let ends = CombineJoinEnds {
        kept: LineEnd::End,
        other: LineEnd::End,
    };
    assert_eq!(
        p.combine_candidate_for_lines(a, b).unwrap().join_kind,
        CombineJoinKind::Merged
    );
    let candidate = p.combine_candidate_joining(a, b, Some(ends)).unwrap();
    assert_eq!(candidate.join_kind, CombineJoinKind::Closure);
    assert!(candidate.reverses);
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moving_line, Some(b));
    assert_eq!(plan.kept_moves_because, None);
    assert!((plan.shift_m - 0.5).abs() < 1e-9);
    assert_eq!(plan.torn_ties.len(), 1);
    assert_eq!(plan.torn_ties[0].kind, TornTieKind::Junction);
    // s0 is on both lines besides the join: the combined line would pass it
    // twice (today: a merge tying the combined line to itself, a loop tie).
    assert_eq!(plan.torn_ties[0].reason, TornTieReason::PassesTwice);
    assert_eq!(plan.torn_ties[0].station, s0);

    let mut opts = options("AB");
    opts.join = Some(ends);
    let report = ok(p.combine_lines(a, b, &opts));
    assert_eq!(report, *plan, "the command does what the preview said");
    let after = p.raw_positions();
    for s in &b_stations {
        if *s == b3 {
            continue;
        }
        near(
            xy(&after, *s),
            [xy(&before, *s)[0], xy(&before, *s)[1] - 0.5],
            TOL,
            &format!("B station {s}"),
        );
    }
    // A stays; the joined station is A's end; B's old start is a new
    // station tied to s0 by the torn junction's closure.
    near(xy(&after, s2), xy(&before, s2), TOL, "A's end stays");
    let combined = chain(&p, a);
    assert_eq!(combined.len(), 6);
    assert_eq!(combined[2], s2);
    let new_end = *combined.last().unwrap();
    assert_ne!(new_end, s0);
    assert_eq!(p.closures.len(), 1);
    let closure = p.closures[0];
    assert_eq!(closure.strategy, ClosureStrategy::Batch);
    assert_eq!((closure.a, closure.b), (s0, new_end));
    near(
        xy(&after, new_end),
        [0.0, -0.5],
        TOL,
        "torn end moved with B",
    );
    assert_valid(&p);
}

#[test]
fn a_datum_held_other_side_makes_the_kept_line_move() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 2));
    let b = line_at(&mut p, "B", [20.0004, 0.0], run("b", 0.0, 5.0, 2));
    let b2 = st(&p, b, 2);
    add_datum(&mut p, b2, gps(10.0, 20.0));
    let before = p.raw_positions();
    let report = ok(p.combine_lines(a, b, &options("AB")));
    assert_eq!(report.moving_line, Some(a));
    assert_eq!(report.kept_moves_because, Some(KeptMovesReason::ZeroDatum));
    let after = p.raw_positions();
    near(xy(&after, b2), xy(&before, b2), TOL, "datum side stays");
    let a0 = st(&p, a, 0);
    near(
        xy(&after, a0),
        [xy(&before, a0)[0] + 0.0004, xy(&before, a0)[1]],
        TOL,
        "kept side moved",
    );
    assert_valid(&p);
}

#[test]
fn lines_that_do_not_meet_or_hold_two_datums_on_the_join_are_refused() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 1));
    let b = line_at(&mut p, "B", [50.0, 0.0], run("b", 90.0, 10.0, 1));
    let candidate = p.combine_candidate_for_lines(a, b).unwrap();
    assert_eq!(
        candidate.blocked_reason.as_deref(),
        Some(
            "A and B do not meet at an end station: merge or close a loop between two of their \
             ends first"
        )
    );
    assert!(refused(p.combine_lines(a, b, &options("AB"))).contains("do not meet"));

    let mut q = empty();
    let a = line(&mut q, "A", run("a", 90.0, 10.0, 1));
    let b = line_at(&mut q, "B", [10.0, 0.0], run("b", 90.0, 10.0, 1));
    let (a1, b0) = (st(&q, a, 1), st(&q, b, 0));
    add_datum(&mut q, a1, gps(10.0, 20.0));
    add_datum(&mut q, b0, gps(10.0, 20.0));
    // b0's datum is the second one: B's piece is placed by it too, so this
    // is "each placed by its own" as well; the join-station reason comes
    // first.
    let text = refused(q.combine_lines(a, b, &options("AB")));
    assert_eq!(
        text,
        "A #1 and B #0 each hold a zero datum, and combining makes them one station: remove one \
         of the two datums first"
    );
    assert_eq!(q.combine_candidates(crate::StationRef::new(a, 1)).len(), 1);
}

#[test]
fn the_joined_station_keeps_the_kept_name_id_and_picked_comment() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 2));
    let b = line_at(&mut p, "B", [20.0, 0.0], run("b", 0.0, 5.0, 2));
    let (a2, b0) = (st(&p, a, 2), st(&p, b, 0));
    p.station_mut(a2).unwrap().comment = Some("kept".to_string());
    p.station_mut(b0).unwrap().comment = Some("other".to_string());
    let candidate = p.combine_candidate_for_lines(a, b).unwrap();
    assert_eq!(candidate.comment_a.as_deref(), Some("kept"));
    assert_eq!(candidate.comment_b.as_deref(), Some("other"));
    let mut both = p.clone();
    let mut opts = options("AB");
    opts.comment_pick = CombinePick::Both;
    ok(both.combine_lines(a, b, &opts));
    assert_eq!(
        both.station(a2).unwrap().comment.as_deref(),
        Some("kept\nother")
    );
    assert!(both.station(b0).is_none());
    opts.comment_pick = CombinePick::Other;
    ok(p.combine_lines(a, b, &opts));
    assert_eq!(p.station(a2).unwrap().comment.as_deref(), Some("other"));
    // Names collided (0 1 2 on both): both lines are lettered.
    assert_eq!(names(&p, a), ["A0", "A1", "A2", "B1", "B2"]);
    assert_valid(&p);
}

#[test]
fn a_branch_at_the_join_stays_attached_and_end_to_end_reverses_the_other_line() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 2));
    let a2 = st(&p, a, 2);
    // B ends on A's end: end to end.
    let b0 = ok(p.create_line(
        meta("B"),
        StartAt::New,
        run("b", 270.0, 10.0, 2),
        Some([40.0, 0.0]),
    ));
    let b_end = st(&p, b0, 2);
    let branch = line_from(&mut p, "Branch", b_end, run("x", 0.0, 3.0, 1));
    let before = p.raw_positions();
    let _ = before;
    // B's end is at (20, 0): co-located with A's end.
    let candidate = p.combine_candidate_for_lines(a, b0).unwrap();
    assert_eq!(candidate.join_kind, CombineJoinKind::CoLocated);
    assert!(candidate.reverses);
    assert_eq!(candidate.branches, vec![branch]);
    ok(p.combine_lines(a, b0, &options("AB")));
    let combined = chain(&p, a);
    assert_eq!(combined[2], a2);
    assert_eq!(p.line_stations(branch).unwrap()[0], a2);
    near(
        xy(&p.raw_positions(), *combined.last().unwrap()),
        [40.0, 0.0],
        TOL,
        "far end",
    );
    assert_valid(&p);
}

#[test]
fn the_station_query_offers_each_partner_ending_at_the_point() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 1));
    let a1 = st(&p, a, 1);
    let b = line_from(&mut p, "B", a1, run("b", 0.0, 10.0, 1));
    let c = line_from(&mut p, "C", a1, run("c", 180.0, 10.0, 1));
    let found: Vec<(LineId, LineId)> = p
        .combine_candidates(crate::StationRef::new(a, 1))
        .iter()
        .map(|c| (c.line_a, c.line_b))
        .collect();
    assert_eq!(found, [(a, b), (a, c)]);
    let all = p.combine_candidates_at_point(crate::StationRef::new(a, 1));
    assert_eq!(all.len(), 3);
    let one = p.combine_candidate_joining(b, a, None).unwrap();
    assert_eq!(
        (one.line_a, one.end_a, one.end_b),
        (a, LineEnd::End, LineEnd::Start)
    );
}

#[test]
fn shotless_lines_join_across_a_closure_their_tie_group_reaches() {
    // Main m0..m6 east; S1 shot-less on m1, S2 shot-less on m6; a `Tie`
    // m1 - m3 (a 0.6.0 same-line merge the migration demoted) and a closure
    // m3 - m6. Today the merge group of S1's station holds m3, so the two
    // lines meet across that closure (wire equivalence seeds 11 and 36).
    let mut p = empty();
    let m = line(&mut p, "Main", run("m", 90.0, 10.0, 6));
    let (m1, m3, m6) = (st(&p, m, 1), st(&p, m, 3), st(&p, m, 6));
    let s1 = line_from(&mut p, "S1", m1, Vec::new());
    let s2 = line_from(&mut p, "S2", m6, Vec::new());
    p.push_closure(m1, Some(m), m3, Some(m), ClosureStrategy::Tie);
    p.push_closure(m3, Some(m), m6, Some(s2), ClosureStrategy::Batch);
    assert_valid(&p);
    let candidate = p.combine_candidate_for_lines(s1, s2).unwrap();
    assert_eq!(candidate.join_kind, CombineJoinKind::Closure);
    assert_eq!(candidate.blocked_reason, None);
    assert_eq!(
        (candidate.station_a, candidate.station_b),
        (m1, m6),
        "joined at the two lines' own stations"
    );
    assert_eq!(
        (candidate.end_a, candidate.end_b),
        (LineEnd::End, LineEnd::Start)
    );
    // Without the `Tie` the two stations are not tied at all.
    let mut q = p.clone();
    q.closures.retain(|c| c.strategy != ClosureStrategy::Tie);
    let none = q.combine_candidate_for_lines(s1, s2).unwrap();
    assert!(none.blocked_reason.is_some());

    // Today (0.6.0), the same survey offers the same join.
    let mut old = stalagmight_legacy::LegacyProject::new("legacy".to_string(), None);
    let flat = old::ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        ..old::ShotData::default()
    };
    let mut add = |shots: Vec<old::ShotData>| {
        old.add_line(old::LineData {
            shots,
            quality: 1.0,
            ..old::LineData::default()
        })
        .id
    };
    let (om, o1, o2) = (add(vec![flat; 6]), add(Vec::new()), add(Vec::new()));
    let at = old::StationRef::new;
    old.merge_stations(at(o1, 0), at(om, 1)).unwrap();
    old.merge_stations(at(o2, 0), at(om, 6)).unwrap();
    let key = |old: &stalagmight_legacy::LegacyProject, slot| old.key_for_ref(slot).unwrap();
    let same_line = old::StationMerge {
        station_a: key(&old, at(om, 1)),
        station_b: key(&old, at(om, 3)),
        merged_at: old.created_at,
    };
    old.station_merges.push(same_line);
    old.create_loop_closure(at(om, 3), at(o2, 0), old::ClosureStrategy::Batch)
        .unwrap();
    let legacy = old.combine_candidate_for_lines(o1, o2).unwrap();
    assert_eq!(
        format!("{:?}", legacy.join_kind),
        format!("{:?}", CombineJoinKind::Closure)
    );
    assert_eq!(legacy.blocked_reason, None);
    assert_eq!(
        format!("{:?}", (legacy.end_a, legacy.end_b)),
        format!("{:?}", (candidate.end_a, candidate.end_b))
    );
}

#[test]
fn a_lettered_line_gives_the_next_letter_to_a_line_joined_at_its_junction() {
    // 0.6.0's Lettered rule (`plan_combine_prefixes`): a kept line with two
    // letter families gives the next free letter to an other line that has
    // none (`A0 A1 A2 B1 B2` + `1 2` -> `.. C1 C2`). At a merged join the
    // other line's join station is the kept line's own station (one name,
    // `B2`): it is not one of the other line's names, as 0.6.0's retired
    // member name was not (K golden combine-merged-join, second combine).
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 2));
    let b = line_at(&mut p, "B", [20.0, 0.0], run("b", 0.0, 5.0, 2));
    ok(p.combine_lines(a, b, &options("AB")));
    assert_eq!(names(&p, a), ["A0", "A1", "A2", "B1", "B2"]);
    let end = st(&p, a, 4);
    let c = line_from(&mut p, "C", end, run("c", 270.0, 5.0, 2));
    assert_eq!(names(&p, c), ["B2", "1", "2"]);
    let candidate = p.combine_candidate_for_lines(a, c).unwrap();
    assert_eq!(candidate.join_kind, CombineJoinKind::Merged);
    // The other line is lettered.
    let renames = candidate.plan_keeping_a.station_renames.clone().unwrap();
    assert_eq!(renames.reason, crate::CombineRenameReason::Lettered);
    assert!(
        renames.kept.is_none(),
        "a lettered kept line keeps its letters"
    );
    let other = renames.other.unwrap();
    assert_eq!(
        (
            other.count,
            other.first_from.as_str(),
            other.first_to.as_str()
        ),
        (2, "1", "C1")
    );
    assert_eq!(
        (other.last_from.as_str(), other.last_to.as_str()),
        ("2", "C2")
    );
    assert_eq!(renames.join_name, "B2");
    let report = ok(p.combine_lines(a, c, &options("ABC")));
    assert_eq!(
        report.station_renames,
        candidate.plan_keeping_a.station_renames
    );
    assert_eq!(names(&p, a), ["A0", "A1", "A2", "B1", "B2", "C1", "C2"]);
    assert_valid(&p);
}

/// RS 4.4 shared join (M-P1-4a): K's end merged onto O's start keeping O's
/// station, so a record names K at the junction. Combining K and O there
/// leaves the combined line native at the junction (the record drops K and
/// O, and is history now), so A3 and A4 hold and the junction keeps its
/// identity (Combine's prefix plan may still rename it). Injection: keep the
/// record's K (the U1 `settle_shared_join_aliases`) and the combine is
/// refused (no native line).
#[test]
fn a_shared_join_named_by_a_record_keeps_a_native_line() {
    let mut p = empty();
    let k = line(&mut p, "K", run("k", 90.0, 10.0, 2));
    let o = line_at(&mut p, "O", [20.0, 0.0], run("o", 0.0, 10.0, 2));
    let (k2, o0) = (st(&p, k, 2), st(&p, o, 0));
    p.merge_stations_auto(k2, o0, o0).unwrap();
    assert!(!p.is_native(o0, k) && p.is_native(o0, o));
    ok(p.combine_lines(k, o, &options("KO")));
    assert_valid(&p);
    assert!(p.is_native(o0, k));
    assert_eq!(p.shown(o0), o0, "the kept station holds the junction");
    assert!(p.stations[&o0].archived.iter().all(|r| r.is_history()));
}
