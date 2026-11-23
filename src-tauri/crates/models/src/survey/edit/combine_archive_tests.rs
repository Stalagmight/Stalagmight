//! Combine Lines with kept and archived stations (plan 2026-11-23, RS
//! 4.4): the join archives the other join station with its other lines, a
//! closure between the joins forces the kept end (4.1 step 4), tears are
//! restoring detaches behind a tear closure that changes nothing visible
//! (all three detach cases), a deduplicated closure passes its archive on,
//! a shared join keeps a record naming the kept line when neither line was
//! native, and rule L letters on third lines. Fully synthetic data.

use crate::{CombineJoinEnds, CombineLinesOptions, CombinePick, LineEnd, StationRef};

use super::super::archive_tests::{shown, undo_is_exact};
use super::super::{ArchivedStation, ClosureEnd, ClosureId, ClosureStrategy, LineId, Project};
use super::edit_test_helpers::{
    assert_valid, empty, flat_u, line, line_at, line_from, merge, ok, run, st,
};

fn opts(pick: CombinePick, kept: LineEnd, other: LineEnd) -> CombineLinesOptions {
    CombineLinesOptions {
        name: "Joined".to_string(),
        color: "#abcdef".to_string(),
        swap_lr: false,
        comment_pick: pick,
        name_pick: CombinePick::Kept,
        join: Some(CombineJoinEnds { kept, other }),
    }
}

/// Name the stations of `line` `{prefix}{index}`, from chain index `from`.
fn name_line(p: &mut Project, line: LineId, prefix: &str, from: usize) {
    let chain = p.line_stations(line).unwrap().to_vec();
    for (i, s) in chain.into_iter().enumerate().skip(from) {
        p.station_mut(s).unwrap().name = format!("{prefix}{i}");
    }
}

/// K runs 10 m east (K0, K1); O runs 9.6 m west from 0.4 m short of K's
/// end (O0, O1) and ends on K's start, merged there with K's start kept (O1
/// is a record of it, naming O); a Batch closure ties K1 to O0.
fn loop_of_two() -> (Project, LineId, LineId, ClosureId) {
    let mut p = empty();
    let k = line_at(&mut p, "K", [100.0, -40.0], run("k", 90.0, 10.0, 1));
    let o = line_at(&mut p, "O", [109.6, -40.0], run("o", 270.0, 9.6, 1));
    name_line(&mut p, k, "K", 0);
    name_line(&mut p, o, "O", 0);
    let (k0, o1) = (st(&p, k, 0), st(&p, o, 1));
    merge(&mut p, o1, k0);
    let (k1, o0) = (st(&p, k, 1), st(&p, o, 0));
    let id = p.push_closure(k1, Some(k), o0, Some(o), ClosureStrategy::Batch);
    assert_valid(&p);
    assert_eq!(shown(&p, k), ["K0", "K1"]);
    assert_eq!(shown(&p, o), ["O0", "K0"]);
    (p, k, o, id)
}

/// The tear closure of a combine: the one tying `x` and `y` (either way).
fn closure_between(p: &Project, x: u32, y: u32) -> super::super::LoopClosure {
    *p.closures
        .iter()
        .find(|c| (c.a, c.b) == (x, y) || (c.a, c.b) == (y, x))
        .unwrap()
}

/// RS 4.4 tear, a record names the torn line (case 1): keeping K, O moves
/// and is torn off K's start; O's end takes its own station back, and the
/// tear closure shows it as K's start, so the combined line shows what both
/// lines showed. Remove Closure gives O's end its own name back.
#[test]
fn a_tear_restores_the_torn_lines_station_behind_its_closure() {
    let (mut p, k, o, _) = loop_of_two();
    let k0 = st(&p, k, 0);
    let o1 = p.stations[&k0].archived[0].id;
    let before = p.clone();
    let report = ok(p.combine_lines(k, o, &opts(CombinePick::Kept, LineEnd::End, LineEnd::Start)));
    assert_valid(&p);
    assert_eq!(report.moving_line, Some(o));
    let combined = p.line_stations(k).unwrap().to_vec();
    assert_eq!(combined[0], k0);
    assert_eq!(*combined.last().unwrap(), o1, "O's end is its own station");
    assert_eq!(shown(&p, k), ["K0", "K1", "K0"], "nothing visible changes");
    let tear = closure_between(&p, k0, o1);
    assert_eq!(tear.archive(), Some((o1, k0)));
    assert!(p.stations[&k0].archived.is_empty());
    undo_is_exact(&before, &p);
    ok_op(p.remove_closure(tear.id, &[]));
    assert_eq!(shown(&p, k), ["K0", "K1", "O1"]);
    assert_valid(&p);
}

/// RS 4.4 tear, the torn line is the junction's only native line (case 2):
/// keeping O, K moves and is torn off its start; K keeps its start, O's end
/// takes its own station back, and the tear closure shows O's end as K's
/// start: nothing visible changes.
#[test]
fn a_tear_of_the_only_native_line_keeps_the_junction_on_it() {
    let (mut p, k, o, _) = loop_of_two();
    let k0 = st(&p, k, 0);
    let o1 = p.stations[&k0].archived[0].id;
    let before = p.clone();
    let report = ok(p.combine_lines(o, k, &opts(CombinePick::Kept, LineEnd::Start, LineEnd::End)));
    assert_valid(&p);
    assert_eq!(report.moving_line, Some(k));
    let combined = p.line_stations(o).unwrap().to_vec();
    assert_eq!(combined[0], k0, "K keeps its start");
    assert_eq!(*combined.last().unwrap(), o1);
    assert_eq!(shown(&p, o), ["K0", "O0", "K0"]);
    assert_eq!(closure_between(&p, k0, o1).archive(), Some((o1, k0)));
    undo_is_exact(&before, &p);
}

/// RS 4.4 tear, the junction is native on both lines (case 3): the torn
/// line gets a new station, and the tear closure shows it as the junction,
/// so B's end still shows A0 on the combined line; Remove Closure shows the
/// new station's own name.
#[test]
fn a_tear_of_a_shared_native_junction_hides_its_copy() {
    let mut p = empty();
    let a = line(&mut p, "A", run("a", 90.0, 10.0, 2));
    let s0 = st(&p, a, 0);
    let b = line_from(
        &mut p,
        "B",
        s0,
        vec![
            flat_u(0.0, 10.0, "b0"),
            flat_u(90.0, 20.0, "b1"),
            flat_u(180.0, 9.5, "b2"),
        ],
    );
    name_line(&mut p, a, "A", 0);
    name_line(&mut p, b, "B", 1);
    let (s2, b3) = (st(&p, a, 2), st(&p, b, 3));
    p.push_closure(s2, Some(a), b3, Some(b), ClosureStrategy::Batch);
    assert_valid(&p);
    let mut options = opts(CombinePick::Kept, LineEnd::End, LineEnd::End);
    options.name = "AB".to_string();
    ok(p.combine_lines(a, b, &options));
    assert_valid(&p);
    let fresh = *p.line_stations(a).unwrap().last().unwrap();
    assert_ne!(fresh, s0);
    let tear = closure_between(&p, s0, fresh);
    assert_eq!(tear.archive(), Some((fresh, s0)));
    let combined = shown(&p, a);
    assert_eq!(combined.first(), combined.last(), "B's end still shows A0");
    assert_eq!(p.shown(fresh), s0);
    // The copy's own name is the prefix plan's (it ran before the tear
    // closure archived it); Remove Closure shows it.
    let removal = p.closure_removal(tear.id).unwrap();
    let own = p.stations[&fresh].name.clone();
    assert_eq!(removal.identity, Some((fresh, own, None)));
    ok_op(p.remove_closure_auto(tear.id));
    assert_eq!(p.shown(fresh), fresh);
    assert_eq!(shown(&p, a).last(), Some(&p.stations[&fresh].name));
    assert_valid(&p);
}

/// RS 4.4 join: the other join station is archived in the kept one with its
/// native lines but the absorbed one (here a third line X), so it is not
/// history: the comment pick does not apply (its comment stays in its
/// record), X shows the joined station with a dot, and Detach gives X its
/// own station back.
#[test]
fn a_join_archives_the_other_join_station_with_its_other_lines() {
    let mut p = empty();
    let k = line(&mut p, "K", run("k", 90.0, 10.0, 2));
    let o = line_at(&mut p, "O", [20.0, 0.0], run("o", 0.0, 10.0, 2));
    let o0 = st(&p, o, 0);
    let x = line_from(&mut p, "X", o0, run("x", 90.0, 5.0, 1));
    name_line(&mut p, k, "K", 0);
    name_line(&mut p, o, "O", 0);
    name_line(&mut p, x, "X", 1);
    let k2 = st(&p, k, 2);
    p.station_mut(k2).unwrap().comment = Some("kept".to_string());
    p.station_mut(o0).unwrap().comment = Some("other".to_string());
    assert_valid(&p);
    ok(p.combine_lines(
        k,
        o,
        &opts(CombinePick::Other, LineEnd::End, LineEnd::Start),
    ));
    assert_valid(&p);
    assert_eq!(st(&p, k, 2), k2);
    assert_eq!(p.stations[&k2].comment.as_deref(), Some("kept"));
    assert_eq!(
        p.stations[&k2].archived,
        vec![ArchivedStation {
            id: o0,
            name: "O0".to_string(),
            comment: Some("other".to_string()),
            external: None,
            lines: vec![x],
        }]
    );
    assert_eq!(shown(&p, x), ["K2", "X1"]);
    assert!(p.is_foreign_slot(StationRef::new(x, 0)));
    let out = ok_op(p.detach_line(k2, x, &[], None));
    assert!(out.value.restored);
    assert_eq!(shown(&p, x), ["O0", "X1"]);
    assert_eq!(p.stations[&o0].comment.as_deref(), Some("other"));
    assert_valid(&p);
}

/// RS 4.1 step 4 on the Combine join: a closure between the two join
/// stations that archives the kept line's keeps the other's identity on the
/// joined node (the kept one becomes history, the absorbed line was its
/// only native line), and the report names the closure.
#[test]
fn a_closure_archiving_the_kept_join_keeps_the_other_join() {
    let (mut p, k, o, join) = loop_of_two();
    let (k1, o0) = (st(&p, k, 1), st(&p, o, 0));
    let c = p.closures.iter_mut().find(|c| c.id == join).unwrap();
    c.archived = Some(ClosureEnd::A);
    assert_valid(&p);
    assert_eq!(shown(&p, k), ["K0", "O0"]);
    let report = ok(p.combine_lines(k, o, &opts(CombinePick::Kept, LineEnd::End, LineEnd::Start)));
    assert_valid(&p);
    assert_eq!(report.join_keep_forced_by, Some(join));
    assert_eq!(st(&p, k, 1), o0, "the joined node is O's start");
    assert!(p.station(k1).is_none());
    let record = p.stations[&o0]
        .archived
        .iter()
        .find(|r| r.id == k1)
        .unwrap();
    assert!(record.is_history());
    assert_eq!(shown(&p, k), ["K0", "O0", "K0"]);
    assert!(report.reidentified.contains(&(k1, o0)));
}

/// RS 4.4 dedupe (#749): when the combine leaves two closures on one pair
/// and drops the one that archived an end, its archive moves to the one
/// that stays, so the end still shows what it showed.
#[test]
fn a_deduplicated_closure_passes_its_archive_on() {
    let mut p = empty();
    let k = line(&mut p, "K", run("k", 90.0, 10.0, 1));
    let (k0, k1) = (st(&p, k, 0), st(&p, k, 1));
    let y = line_from(
        &mut p,
        "Y",
        k0,
        vec![
            flat_u(0.0, 5.0, "y-0"),
            flat_u(90.0, 10.0, "y-1"),
            flat_u(180.0, 5.0, "y-2"),
        ],
    );
    let (y2, y3) = (st(&p, y, 2), st(&p, y, 3));
    let at = super::edit_test_helpers::xy(&p.raw_positions(), k1);
    let o = line_at(&mut p, "O", at, run("o", 0.0, 5.0, 1));
    name_line(&mut p, k, "K", 0);
    name_line(&mut p, y, "Y", 1);
    name_line(&mut p, o, "O", 0);
    let o1 = st(&p, o, 1);
    merge(&mut p, o1, y2);
    let o0 = st(&p, o, 0);
    let legacy = p.push_closure(
        y3,
        Some(y),
        k1,
        Some(k),
        ClosureStrategy::ProportionalByLength,
    );
    let solved = p.push_closure(y3, Some(y), o0, Some(o), ClosureStrategy::Batch);
    p.closures[0].archived = Some(ClosureEnd::A);
    assert_valid(&p);
    assert_eq!(shown(&p, y)[3], "K1");
    let report = ok(p.combine_lines(k, o, &opts(CombinePick::Kept, LineEnd::End, LineEnd::Start)));
    assert_valid(&p);
    assert_eq!(report.closures_replaced.len(), 1);
    assert_eq!(report.closures_replaced[0].dropped.id, legacy);
    assert!(p.closures.iter().all(|c| c.id != legacy));
    let stays = p.closures.iter().find(|c| c.id == solved).unwrap();
    assert_eq!(stays.archive(), Some((y3, k1)));
    assert_eq!(shown(&p, y)[3], "K1");
}

/// RS 4.4 shared join where neither line is native (a third line X is):
/// the record naming the kept line keeps naming the combined line (it
/// still shows the junction through it, and Detach still gives that
/// station back), the record naming the absorbed line becomes history, and
/// A3 and A4 hold.
#[test]
fn a_shared_join_named_by_two_records_keeps_the_kept_lines() {
    let mut p = empty();
    let x = line(&mut p, "X", run("x", 0.0, 10.0, 2));
    let k = line_at(&mut p, "K", [-20.0, 10.0], run("k", 90.0, 10.0, 2));
    let o = line_at(&mut p, "O", [5.0, 30.0], run("o", 90.0, 10.0, 2));
    name_line(&mut p, x, "X", 0);
    name_line(&mut p, k, "K", 0);
    name_line(&mut p, o, "O", 0);
    let (j, k2) = (st(&p, x, 1), st(&p, k, 2));
    ok_op(p.merge_stations_auto(k2, j, j));
    let o0 = st(&p, o, 0);
    ok_op(p.merge_stations_auto(o0, j, j));
    assert_valid(&p);
    assert!(!p.is_native(j, k) && !p.is_native(j, o) && p.is_native(j, x));
    ok(p.combine_lines(k, o, &opts(CombinePick::Kept, LineEnd::End, LineEnd::Start)));
    assert_valid(&p);
    let records = &p.stations[&j].archived;
    let of = |id: u32| records.iter().find(|r| r.id == id).unwrap();
    assert_eq!(of(k2).lines, vec![k]);
    assert!(of(o0).is_history());
    assert_eq!(shown(&p, k)[2], "X1");
}

/// RS 4.4: the prefix plan renames a junction of the kept line, and its new
/// name repeats on a third line; rule L letters the station shown on fewer
/// lines (the third line's own), never the junction, and the report says
/// so.
#[test]
fn a_renamed_junction_letters_the_third_lines_station_by_rule_l() {
    let mut p = empty();
    let k = line(&mut p, "K", run("k", 90.0, 10.0, 2));
    let o = line_at(&mut p, "O", [20.0, 0.0], run("o", 0.0, 10.0, 2));
    let k1 = st(&p, k, 1);
    let x = line_from(&mut p, "X", k1, run("x", 180.0, 5.0, 1));
    let x1 = st(&p, x, 1);
    p.station_mut(x1).unwrap().name = "A1".to_string();
    assert_valid(&p);
    let report = ok(p.combine_lines(k, o, &opts(CombinePick::Kept, LineEnd::End, LineEnd::Start)));
    assert_valid(&p);
    assert_eq!(p.stations[&k1].name, "A1", "the prefix plan's name");
    assert_eq!(shown(&p, x), ["A1", "A1a"]);
    let lettered = &report.identities.lettered;
    assert_eq!(lettered.len(), 1);
    assert_eq!((lettered[0].station, lettered[0].other), (x1, k1));
}

fn ok_op<T>(
    result: Result<super::super::OpOutcome<T>, super::super::OpError>,
) -> super::super::OpOutcome<T> {
    match result {
        Ok(outcome) => outcome,
        Err(error) => panic!("op refused: {error}"),
    }
}

/// M-P1-2 on the Combine join: each join station is archived by a closure
/// of its own line, so the joined node would be archived twice (C1); the
/// normalisation keeps the earlier closure's archive and reports the other.
#[test]
fn a_join_of_two_archived_ends_keeps_the_earlier_archive() {
    let mut p = empty();
    let k = line(&mut p, "K", run("k", 90.0, 10.0, 2));
    let o = line_at(&mut p, "O", [20.0, 0.0], run("o", 0.0, 10.0, 2));
    name_line(&mut p, k, "K", 0);
    name_line(&mut p, o, "O", 0);
    let (k0, k2, o0, o2) = (st(&p, k, 0), st(&p, k, 2), st(&p, o, 0), st(&p, o, 2));
    let first = ok_op(p.create_closure(
        k2,
        Some(k),
        k0,
        Some(k),
        ClosureStrategy::Batch,
        Some(k0),
        &[],
    ))
    .value
    .closure
    .id;
    let second = ok_op(p.create_closure(
        o0,
        Some(o),
        o2,
        Some(o),
        ClosureStrategy::Batch,
        Some(o2),
        &[],
    ))
    .value
    .closure
    .id;
    assert_eq!(shown(&p, k), ["K0", "K1", "K0"]);
    assert_eq!(shown(&p, o), ["O2", "O1", "O2"]);
    let report = ok(p.combine_lines(k, o, &opts(CombinePick::Kept, LineEnd::End, LineEnd::Start)));
    assert_valid(&p);
    assert_eq!(report.identities.unarchived, vec![second]);
    assert_eq!(p.shown(k2), k0);
    assert!(p
        .closures
        .iter()
        .any(|c| c.id == first && c.archive() == Some((k2, k0))));
    assert_eq!(shown(&p, k), ["K0", "K1", "K0", "O1", "O2"]);
}

/// Model review finding 3: at a co-located join whose other join station
/// O1 is also on a third line T, the join archives O1 with T (RS 4.4: its
/// native lines but the absorbed one), so T shows K2 from then on and O1's
/// comment is hidden with it; the comment pick (here Other) does not apply,
/// since O1 does not become history. Nothing said so. The report now names
/// O1's record (`join_archived`, on the wire too) and its hidden comment.
/// Injection: set `join_archived` to `None` in `run_combine`.
#[test]
fn a_join_station_kept_for_a_third_line_is_reported() {
    let mut p = empty();
    let k = line_at(&mut p, "K", [0.0, 0.0], run("k", 90.0, 10.0, 2));
    let o = line_at(&mut p, "O", [30.0, 0.0], run("o", 270.0, 10.0, 1));
    name_line(&mut p, k, "K", 0);
    name_line(&mut p, o, "O", 0);
    let (k2, o1) = (st(&p, k, 2), st(&p, o, 1));
    let t = line_from(&mut p, "T", o1, run("t", 0.0, 5.0, 1));
    name_line(&mut p, t, "T", 1);
    p.station_mut(k2).unwrap().comment = Some("kept note".to_string());
    p.station_mut(o1).unwrap().comment = Some("other note".to_string());
    assert_valid(&p);
    let before = p.clone();
    let report = ok(p.combine_lines(k, o, &opts(CombinePick::Other, LineEnd::End, LineEnd::End)));
    assert_valid(&p);
    let archived = report
        .join_archived
        .clone()
        .expect("O1 stays archived for T");
    assert_eq!((archived.id, archived.lines.clone()), (o1, vec![t]));
    assert_eq!(archived.comment.as_deref(), Some("other note"));
    assert_eq!(
        p.stations[&k2].comment.as_deref(),
        Some("kept note"),
        "the pick does not apply"
    );
    assert_eq!(shown(&p, t)[0], "K2");
    let hidden: Vec<(u32, &str)> = report
        .identities
        .hidden
        .iter()
        .map(|h| (h.station, h.comment.as_str()))
        .collect();
    assert_eq!(hidden, [(o1, "other note")]);
    let plan = before.combine_plan_wire(&report);
    assert_eq!(plan.join_archived.map(|r| r.lines), Some(vec![t]));
}

/// Model review finding 1 at a Combine join: the other line's join station
/// O2 is a loop end (its closure shows O0), and the kept K2 is kept. The
/// fold moved the closure's archive onto the joined node, which then showed
/// O0 on both lines, so K2 was shown nowhere. The closure now drops its
/// archive (RS 1.3) and the report names it. Injection: drop the
/// `unarchive_for_keep` call in `run_combine`.
#[test]
fn a_join_shows_the_kept_join_station_over_a_loop_end() {
    let mut p = empty();
    let k = line_at(&mut p, "K", [0.0, 0.0], run("k", 90.0, 10.0, 2));
    let o = line_at(&mut p, "O", [40.0, 0.0], run("o", 270.0, 10.0, 2));
    name_line(&mut p, k, "K", 0);
    name_line(&mut p, o, "O", 0);
    let (k2, o0, o2) = (st(&p, k, 2), st(&p, o, 0), st(&p, o, 2));
    let id = p.push_closure(o2, Some(o), o0, Some(o), ClosureStrategy::Batch);
    p.closures[0].archived = Some(ClosureEnd::A);
    assert_valid(&p);
    assert_eq!(p.shown(o2), o0);
    let report = ok(p.combine_lines(k, o, &opts(CombinePick::Kept, LineEnd::End, LineEnd::End)));
    assert_valid(&p);
    assert_eq!(p.shown(k2), k2, "the kept join station is shown");
    assert!(report.identities.unarchived.contains(&id));
    assert!(p.closures.iter().all(|c| c.archived.is_none()));
}
