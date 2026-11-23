//! Package RS-FX4 (plan 2026-11-23, kept and archived stations): records
//! carry the identity they were archived under, so a merge then a Detach
//! gives back a merged junction whole (model review finding 4). Fully
//! synthetic data.

use super::ops_test_helpers::{assert_valid, free_line, ok, run};
use super::test_helpers::{at, project};
use super::{LineId, Project, StationId};

/// Give each station of `line` the name `{prefix}{index}`.
fn name_line(p: &mut Project, line: LineId, prefix: &str) {
    let chain = p.line_stations(line).unwrap().to_vec();
    for (i, s) in chain.into_iter().enumerate() {
        p.station_mut(s).unwrap().name = format!("{prefix}{i}");
    }
}

/// Give station `s` a comment and an external id of its own.
fn mark(p: &mut Project, s: StationId, text: &str) {
    let station = p.station_mut(s).unwrap();
    station.comment = Some(format!("{text} note"));
    station.external = Some(format!("{text}-ext"));
}

/// Every slot's node, line by line.
fn chains(p: &Project) -> Vec<(LineId, Vec<StationId>)> {
    p.lines()
        .iter()
        .map(|l| (l.id, l.stations().collect()))
        .collect()
}

/// Junction X (its own line LX native, LX2 named by X's record R from an
/// earlier merge) and a free line LY: `(project, lx, lx2, ly, x, r, y)`.
/// Every identity has a name, a comment and an external id of its own.
fn junction_and_line() -> (
    Project,
    LineId,
    LineId,
    LineId,
    StationId,
    StationId,
    StationId,
) {
    let mut p = project();
    let lx = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let lx2 = free_line(&mut p, [0.0, 50.0], run(90.0, 2, 10.0));
    let ly = free_line(&mut p, [0.0, 100.0], run(90.0, 2, 10.0));
    name_line(&mut p, lx, "X");
    name_line(&mut p, lx2, "R");
    name_line(&mut p, ly, "Y");
    let (x, r, y) = (at(&p, lx, 1), at(&p, lx2, 0), at(&p, ly, 1));
    mark(&mut p, x, "X");
    mark(&mut p, r, "R");
    mark(&mut p, y, "Y");
    ok(p.merge_stations(r, x, x, &[]));
    assert_valid(&p);
    assert_eq!(p.topology().lines_of(x), vec![lx, lx2]);
    assert!(p.record_naming(x, lx2).is_some_and(|rec| rec.id == r));
    (p, lx, lx2, ly, x, r, y)
}

/// Model review finding 4, the exact sequence: X (with R inside it) merged
/// into Y keeping Y, then Detach of LX. X comes back with LX and LX2, R
/// still inside it, and Y is as it was: every station byte for byte
/// (names, comments, externals, ids, records) and every slot. The same for
/// a Detach of LX2 (R's line: R was X's), and for either merge direction
/// (Y's piece moving or staying put). Before RS-FX4, R moved flat onto Y's
/// node, so X came back on LX alone and LX2 kept showing Y.
#[test]
fn detaching_a_line_of_a_merged_junction_brings_the_whole_junction_back() {
    let (before, lx, lx2, _ly, x, r, y) = junction_and_line();
    // `merge_stations(a, b, ..)` moves a's piece onto b's: X's piece moves
    // in the first, Y's in the second (the kept station is the moving one,
    // RS 4.1 step 3).
    for (a, b) in [(x, y), (y, x)] {
        let mut merged = before.clone();
        let out = ok(merged.merge_stations(a, b, y, &[]));
        assert_eq!((out.value.kept, out.value.archived), (y, x));
        assert_valid(&merged);
        assert_eq!(merged.shown_name(at(&merged, lx2, 0)), Some("Y1"));
        for line in [lx, lx2] {
            let naming = merged.detach_naming(y, line, None).unwrap();
            assert!(naming.restored);
            assert_eq!((naming.id, naming.name.as_str()), (x, "X1"));
            assert_eq!(naming.comment.as_deref(), Some("X note"));
            let other = if line == lx { lx2 } else { lx };
            assert_eq!(
                naming.together,
                vec![other],
                "the lines that come off together"
            );
            let mut q = merged.clone();
            let detached = ok(q.detach_line(y, line, &[], None));
            assert_eq!(detached.value.new_station, x);
            assert_valid(&q);
            assert_eq!(
                chains(&q),
                chains(&before),
                "detach of {line} after {a}->{b}"
            );
            assert_eq!(
                q.stations, before.stations,
                "detach of {line} after {a}->{b}"
            );
            // R is still X's record, and a second Detach gives it back.
            assert!(q.record_naming(x, lx2).is_some_and(|rec| rec.id == r));
            ok(q.detach_line(x, lx2, &[], None));
            assert_eq!(q.shown_name(at(&q, lx2, 0)), Some("R0"));
        }
    }
}

/// `junction_and_line()` with X merged into Y keeping Y: `(merged, lx,
/// lx2, ly, x, r, y)`.
fn merged() -> (
    Project,
    LineId,
    LineId,
    LineId,
    StationId,
    StationId,
    StationId,
) {
    let (mut p, lx, lx2, ly, x, r, y) = junction_and_line();
    ok(p.merge_stations(x, y, y, &[]));
    assert_valid(&p);
    (p, lx, lx2, ly, x, r, y)
}

/// The owners on `node`: `(record, into)` in list order.
fn owners(p: &Project, node: StationId) -> Vec<(StationId, Option<StationId>)> {
    p.stations[&node]
        .archived
        .iter()
        .map(|rec| (rec.id, rec.into))
        .collect()
}

/// A merge archives the retired station with its records under it (RS 4.1
/// step 2), whichever station's piece moves; keeping the moving station
/// gives the stationary one its records under it (step 3).
#[test]
fn a_merge_archives_the_retired_station_with_its_records_under_it() {
    let (p, _, _, _, x, r, y) = merged();
    assert_eq!(owners(&p, y), [(r, Some(x)), (x, None)]);
    // Y's piece moves onto X's and Y is kept: the node is re-identified, X
    // (stationary) becomes the record and keeps R under it.
    let (mut q, _, _, _, x, r, y) = junction_and_line();
    ok(q.merge_stations(y, x, y, &[]));
    assert_valid(&q);
    assert_eq!(owners(&q, y), [(r, Some(x)), (x, None)]);
}

/// Undo and redo of the merge and of the Detach that gives X back, through
/// the history entry's text (RS 10): each step restores the record owners
/// exactly.
#[test]
fn undo_and_redo_restore_record_owners() {
    use super::changeset::{
        capture, decode_entry, encode_entry, redo, undo, EntryLimits, HistoryEntry, HistoryRefs,
        RecordedPreview,
    };
    let (before, lx, _, _, x, _, y) = junction_and_line();
    let mut merged = before.clone();
    ok(merged.merge_stations(x, y, y, &[]));
    let mut detached = merged.clone();
    ok(detached.detach_line(y, lx, &[], None));
    for (from, to) in [(&before, &merged), (&merged, &detached)] {
        let entry = HistoryEntry {
            timestamp: chrono::DateTime::UNIX_EPOCH,
            kind: "merge_stations".to_string(),
            label: "step".to_string(),
            refs: HistoryRefs::default(),
            preview: RecordedPreview::default(),
            changes: capture(from, to),
        };
        let text = encode_entry(&entry).unwrap();
        let changes = decode_entry(&text, &EntryLimits::default())
            .unwrap()
            .changes;
        let mut live = to.clone();
        undo(&mut live, &changes).unwrap();
        assert_eq!(live.stations, from.stations, "undo");
        assert_eq!(chains(&live), chains(from));
        redo(&mut live, &changes).unwrap();
        assert_eq!(live.stations, to.stations, "redo");
        assert_valid(&live);
    }
}

/// RS 4.2 case 2 with a nested record: detaching Y's own line LY keeps Y
/// there, and the other lines (LX, LX2) come off onto X, the only record
/// directly under Y's identity, with R still inside it. `rest_keep` may
/// name X but not R (R is inside X).
#[test]
fn detaching_the_kept_line_gives_the_rest_the_whole_archived_junction() {
    let (p, lx, lx2, ly, x, r, y) = merged();
    let naming = p.detach_naming(y, ly, None).unwrap();
    assert!(naming.kept);
    assert_eq!(naming.rest_choices, vec![x]);
    assert_eq!(naming.rest_default, Some(x));
    assert!(matches!(
        p.clone().detach_line(y, ly, &[], Some(r)),
        Err(super::OpError::InvalidKeep(id)) if id == r
    ));
    let mut q = p;
    let out = ok(q.detach_line(y, ly, &[], None));
    assert_eq!(out.value.new_station, x);
    assert_valid(&q);
    assert_eq!(q.topology().lines_of(x), vec![lx, lx2]);
    assert_eq!(owners(&q, x), [(r, None)]);
    assert!(q.stations[&y].archived.is_empty());
    assert_eq!(q.shown_name(at(&q, lx2, 0)), Some("X1"));
}

/// RS 4.3 with a nested record: deleting Y's own line leaves the junction
/// on LX and LX2 only, both named by X's subtree; X becomes the junction's
/// identity, R comes under it (still naming LX2), and Y becomes history.
#[test]
fn a_restore_takes_the_whole_archived_junction_back() {
    let (mut p, lx, lx2, ly, x, r, y) = merged();
    p.delete_line(ly).unwrap();
    assert_valid(&p);
    let node = at(&p, lx, 1);
    assert_eq!(node, x, "the node takes X's id");
    assert_eq!(p.shown_name(node), Some("X1"));
    assert_eq!(owners(&p, x), [(r, None), (y, None)]);
    assert!(p.stations[&x].archived[1].is_history());
    assert_eq!(p.shown_name(at(&p, lx2, 0)), Some("X1"));
    assert!(p.record_naming(x, lx2).is_some_and(|rec| rec.id == r));
}

/// A record that loses its last line hands the records under it to its
/// owner (A5, A6): after LX goes, X is history and R is under Y directly,
/// so a Detach of LX2 gives R back.
#[test]
fn a_record_that_becomes_history_hands_its_records_up() {
    let (mut p, lx, lx2, _, x, r, y) = merged();
    p.delete_line(lx).unwrap();
    assert_valid(&p);
    assert_eq!(owners(&p, y), [(r, None), (x, None)]);
    assert!(p.stations[&y].archived[1].is_history());
    let mut q = p.clone();
    ok(q.detach_line(y, lx2, &[], None));
    assert_valid(&q);
    assert_eq!(q.shown_name(at(&q, lx2, 0)), Some("R0"));
    assert!(q.stations[&y].archived.iter().any(|rec| rec.id == x));
}

/// Add .stalx shifts record owners with the record ids.
#[test]
fn append_shifts_record_owners() {
    let (source, _, _, _, x, r, y) = merged();
    let mut target = project();
    free_line(&mut target, [500.0, 0.0], run(0.0, 3, 5.0));
    let base = target.counters.next_station_id;
    ok(target.append_project(&source));
    assert_valid(&target);
    assert_eq!(
        owners(&target, y + base),
        [(r + base, Some(x + base)), (x + base, None)]
    );
}

/// A6 on a model: an owner that is no record of the node, the record
/// itself, a history record or a cycle is reported, each by name; a cycle
/// through all of a station's records is reported in linear time: 4x the
/// records (25,000 then 100,000) take about 4x the time
/// ([`crate::linear_time`], RS-FX10). Injection: walking each record's
/// chain to its end with no memo, a quadratic walk of the cycle, took 16x
/// the time at 4x the records here.
#[test]
fn validate_reports_every_owner_fault() {
    use super::Violation;
    let (p, _, _, _, x, r, y) = merged();
    let with = |into: Option<StationId>, x_into: Option<StationId>, x_history: bool| {
        let mut q = p.clone();
        let s = q.station_mut(y).unwrap();
        s.archived[0].into = into;
        s.archived[1].into = x_into;
        if x_history {
            s.archived[1].lines.clear();
        }
        q.validate()
    };
    assert!(
        with(Some(999), None, false).contains(&Violation::DanglingArchivedOwner {
            station: y,
            archived: r,
            into: 999
        })
    );
    assert!(
        with(Some(r), None, false).contains(&Violation::DanglingArchivedOwner {
            station: y,
            archived: r,
            into: r
        })
    );
    let cycle = with(Some(x), Some(r), false);
    assert!(cycle.contains(&Violation::ArchivedOwnerCycle {
        station: y,
        archived: r
    }));
    assert!(cycle.contains(&Violation::ArchivedOwnerCycle {
        station: y,
        archived: x
    }));
    assert!(
        with(Some(x), None, true).contains(&Violation::ArchivedUnderHistory {
            station: y,
            archived: r,
            into: x
        })
    );

    let long = |n: usize| {
        let mut long = p.clone();
        let first = long.counters.next_station_id;
        let count = n as StationId;
        let s = long.station_mut(y).unwrap();
        for i in 0..count {
            let mut rec = s.archived[1].clone();
            rec.id = first + i;
            rec.lines.clear();
            rec.into = Some(first + (i + 1) % count);
            s.archived.push(rec);
        }
        long.counters.next_station_id = first + count;
        (long, n)
    };
    crate::linear_time::assert_linear("an owner cycle", 25_000, long, |(long, n)| {
        let started = std::time::Instant::now();
        let found = long.validate();
        let elapsed = started.elapsed();
        let cycles = found
            .iter()
            .filter(|v| matches!(v, Violation::ArchivedOwnerCycle { .. }))
            .count();
        assert_eq!(cycles, *n);
        elapsed
    });
}

/// The record helpers keep list order: a subtree taken out keeps its
/// records' relative order, those under its top re-rooted; a record taken
/// alone hands the records under it to its own owner.
#[test]
fn record_helpers_keep_order_and_owners() {
    use super::{ArchivedStation, Station};
    let rec = |id, into| ArchivedStation {
        id,
        name: format!("r{id}"),
        comment: None,
        external: None,
        into,
        lines: vec![id],
    };
    let mut s = Station::new(1, "N");
    // 10 under 20, 30 under 10, 40 alone, 20 under the node.
    s.archived = vec![
        rec(10, Some(20)),
        rec(40, None),
        rec(30, Some(10)),
        rec(20, None),
    ];
    assert_eq!(s.record_top(30).map(|t| t.id), Some(20));
    assert_eq!(s.subtree_lines(20), vec![10, 30, 20]);
    let mut taken = s.clone();
    let (head, under) = taken.take_subtree(20).unwrap();
    assert_eq!(head.id, 20);
    assert_eq!(
        under.iter().map(|r| (r.id, r.into)).collect::<Vec<_>>(),
        [(10, None), (30, Some(10))]
    );
    assert_eq!(taken.archived, vec![rec(40, None)]);
    let mut alone = s;
    alone.take_record(10).unwrap();
    assert_eq!(
        alone
            .archived
            .iter()
            .map(|r| (r.id, r.into))
            .collect::<Vec<_>>(),
        [(40, None), (30, Some(20)), (20, None)]
    );
}
