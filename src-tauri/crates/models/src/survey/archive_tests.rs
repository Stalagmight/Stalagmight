//! Kept and archived stations (plan 2026-11-23, RS): the merge's keep with a
//! bit-identical raw layout (M-P1-1), the epilogue's restores (RS 4.3) with
//! the comment carry-over, rule K, rule L, the closure archives (shown
//! identity, C1, C2, normalisation, N8 with a same-line closure), records
//! through range moves, tears and Add .stalx, crafted files that report
//! instead of hanging, and Undo exact over all of it. Fully synthetic data.

use crate::{ShotKind, StationRef};

use super::changeset::testkit::{persisted, same_state};
use super::changeset::{capture, redo, undo};
use super::ops_detach::DetachMode;
use super::ops_test_helpers::{
    assert_valid, branch, free_line, gps_at, ok, origin, run, xy_all, z_all,
};
use super::test_helpers::{at, project};
use super::{
    ArchivedStation, ClosureEnd, ClosureStrategy, LineId, Project, RangeDeleteMode, StationId,
    Violation,
};

const TOL: f64 = 1e-9;

/// Undo then redo the change from `before` to `after`, bit for bit, in the
/// session and through the persisted entry.
pub(super) fn undo_is_exact(before: &Project, after: &Project) {
    let changes = capture(before, after);
    for (form, cs) in [
        ("session", changes.clone()),
        ("persisted", persisted(&changes)),
    ] {
        let mut live = after.clone();
        undo(&mut live, &cs).unwrap_or_else(|e| panic!("{form} undo: {e}"));
        same_state(&live, before).unwrap_or_else(|e| panic!("{form} undo: {e}"));
        redo(&mut live, &cs).unwrap_or_else(|e| panic!("{form} redo: {e}"));
        same_state(&live, after).unwrap_or_else(|e| panic!("{form} redo: {e}"));
    }
}

/// The names `line` shows, in chain order.
pub(super) fn shown(p: &Project, line: LineId) -> Vec<String> {
    p.line_stations(line)
        .unwrap()
        .iter()
        .map(|&s| p.shown_name(s).unwrap().to_string())
        .collect()
}

/// A record with a name and the lines it names.
pub(super) fn record(id: StationId, name: &str, lines: Vec<LineId>) -> ArchivedStation {
    ArchivedStation {
        id,
        name: name.to_string(),
        comment: None,
        external: None,
        into: None,
        lines,
    }
}

/// Every slot's raw position, as bits, line by line in chain order.
pub(super) fn slot_bits(p: &Project) -> Vec<[u64; 3]> {
    let positions = p.raw_positions();
    let mut out = Vec::new();
    for line in p.lines() {
        for station in line.stations() {
            let [x, y] = positions.xy(station).unwrap();
            let z = positions.z(station).unwrap();
            out.push([x.to_bits(), y.to_bits(), z.to_bits()]);
        }
    }
    out
}

/// The demo's W2/W4 survey: Far Room (0, 1, 2, 3; 3 x 10 m) and the Upstream
/// piece (0, 1, 2, 3; 3 x 5 m), Upstream's start merged onto Far Room's end,
/// which is kept and carries a comment. Upstream's start shows Far Room's
/// `3` with Far Room's dot (a foreign slot), and Upstream's own `3` keeps its
/// name: a foreign slot counts for no uniqueness (RS-FX5).
fn far_room_and_upstream() -> (Project, LineId, LineId) {
    let mut p = project();
    let far = free_line(&mut p, [0.0, 0.0], run(90.0, 3, 10.0));
    let up = free_line(&mut p, [60.0, 40.0], run(0.0, 3, 5.0));
    let (f3, u0) = (at(&p, far, 3), at(&p, up, 0));
    p.station_mut(f3).unwrap().comment = Some("Far Room: large chamber".to_string());
    let out = ok(p.merge_stations_auto(u0, f3, f3));
    assert!(out.identities.lettered.is_empty());
    assert_valid(&p);
    assert_eq!(shown(&p, up), ["3", "1", "2", "3"]);
    (p, far, up)
}

// ----- merge: keep and geometry -------------------------------------------

/// RS 4.1 step 3 and M-P1-1: a merge keeps either station with a
/// bit-identical raw layout and depths. The common case that broke it: no
/// datums, line A created first (its placement first in the list, on A:0),
/// line B next (its placement on its start), and A:3 merged onto B:0, so B
/// is the stationary piece with its placement ON the merge station, while
/// rule K keeps A:3, the moving station. Injection: judge "stationary" in
/// `settle_placements` by the re-keyed id (the pre-fix code) and keeping A:3
/// moves B onto A instead.
#[test]
fn a_merge_keeps_either_station_with_a_bit_identical_raw_layout() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 5, 10.0));
    let b = free_line(&mut p, [300.0, 40.0], run(0.0, 4, 5.0));
    let (a0, a3, b0) = (at(&p, a, 0), at(&p, a, 3), at(&p, b, 0));
    assert_eq!(p.placements[0].station, a0, "the moving piece's first");
    assert_eq!(p.placements[1].station, b0, "on the merge station");
    assert_eq!(p.merge_direction(a3, b0), (b0, a3));
    assert_eq!(p.merge_default_keep(a3, b0), a3, "rule K: A is longer");
    let before = xy_all(&p);

    let mut stationary = p.clone();
    let mut moving = p.clone();
    let s = ok(stationary.merge_stations_auto(a3, b0, b0));
    let m = ok(moving.merge_stations_auto(a3, b0, a3));
    assert_valid(&stationary);
    assert_valid(&moving);
    assert_eq!((s.value.kept, s.value.archived), (b0, a3));
    assert_eq!((m.value.kept, m.value.archived), (a3, b0));
    assert_eq!((m.value.stationary, m.value.moving), (b0, a3));
    assert_eq!(m.reidentified, vec![(b0, a3)]);
    assert_eq!(slot_bits(&stationary), slot_bits(&moving), "bit for bit");
    // B stayed put: its start (the merged node) where B:0 was.
    let at_node = moving.raw_positions().xy(a3).unwrap();
    assert!((at_node[0] - before[&b0][0]).abs() < TOL);
    assert!((at_node[1] - before[&b0][1]).abs() < TOL);
    assert_eq!(moving.placements.len(), 1);
    assert_eq!(moving.placements[0].station, a3, "re-keyed, not dropped");
    assert_eq!(
        [
            moving.placements[0].x.to_bits(),
            moving.placements[0].y.to_bits()
        ],
        [
            stationary.placements[0].x.to_bits(),
            stationary.placements[0].y.to_bits()
        ]
    );
    // The kept identity is shown everywhere, the other is a record.
    assert_eq!(moving.shown_name(a3), Some("3"));
    assert_eq!(
        moving.stations[&a3].archived,
        vec![record(b0, "0", vec![b])]
    );
    assert_eq!(
        stationary.stations[&b0].archived,
        vec![record(a3, "3", vec![a])]
    );
    // The previews read depths at the merged node, whatever its id.
    let (ps, pm) = (
        p.merge_preview(a3, b0, b0).unwrap(),
        p.merge_preview(a3, b0, a3).unwrap(),
    );
    assert_eq!(ps.joined_depth_m.to_bits(), pm.joined_depth_m.to_bits());
    assert_eq!(ps.shift_m.to_bits(), pm.shift_m.to_bits());
    assert_eq!((ps.kept, pm.kept, pm.default_keep), (b0, a3, a3));
    assert_eq!(pm.hidden, (b0, "0".to_string(), None));
    undo_is_exact(&p, &moving);
}

/// Keeping the moving station with a datum on the stationary one: the datum
/// stays on the merged node under its new id (`reidentified`), the frame
/// does not change and nothing of the placed piece moves.
#[test]
fn keeping_the_moving_station_keeps_the_datum_and_the_frame() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 5, 10.0));
    let b = free_line(&mut p, [300.0, 40.0], run(0.0, 2, 5.0));
    let (a3, b0, b2) = (at(&p, a, 3), at(&p, b, 0), at(&p, b, 2));
    ok(p.set_datum_at(StationRef::new(b, 0), origin(), None));
    assert_eq!(p.merge_default_keep(a3, b0), b0, "the datum holder");
    let before = xy_all(&p);
    let out = ok(p.merge_stations_auto(a3, b0, a3));
    assert_valid(&p);
    assert_eq!(out.value.kept, a3);
    assert!(!out.anchors.frame_changed);
    assert_eq!(p.datums[0].station, a3);
    assert!((xy_all(&p)[&b2][0] - before[&b2][0]).abs() < TOL);
    assert!((xy_all(&p)[&b2][1] - before[&b2][1]).abs() < TOL);
    assert_eq!(z_all(&p)[&a3].to_bits(), z_all(&p)[&a3].to_bits());
}

/// RS 4.1 step 4: merging the two ends of an archiving closure keeps the
/// closure's kept end whatever `keep` says (the preview names the closure),
/// and its archived end becomes the merge record; the sweep then removes the
/// closure (#738) with nothing to restore.
#[test]
fn merging_the_ends_of_an_archiving_closure_keeps_its_kept_end() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 2, 50.0));
    let b = free_line(&mut p, [500.0, 0.0], run(0.0, 2, 50.0));
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    ok(p.set_datum_at(StationRef::new(b, 0), gps_at(30.0, 100.0), None));
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    let id = ok(p.create_closure(a2, Some(a), b0, Some(b), ClosureStrategy::Batch, None, &[]))
        .value
        .closure
        .id;
    // The closure archives A:2 (kept B:0): A shows "0" at its end, so A's own
    // "0" is renamed first.
    p.station_mut(at(&p, a, 0)).unwrap().name = "start".to_string();
    p.closures[0].archived = Some(ClosureEnd::A);
    assert_valid(&p);
    assert_eq!(p.shown(a2), b0);
    assert_eq!(p.forced_merge_keep(a2, b0), Some((id, b0)));
    assert_eq!(p.merge_default_keep(a2, b0), b0);
    let preview = p.merge_preview(a2, b0, a2).unwrap();
    assert_eq!((preview.kept, preview.keep_forced_by), (b0, Some(id)));
    let out = ok(p.merge_stations(a2, b0, a2, &[]));
    assert_valid(&p);
    assert_eq!((out.value.kept, out.value.keep_forced_by), (b0, Some(id)));
    assert!(p.closures.is_empty(), "#738");
    assert_eq!(p.stations[&b0].archived, vec![record(a2, "2", vec![a])]);
}

/// RS 4.1 step 5: a closure-archived station kept by a merge keeps showing
/// its closure's kept end. When the other station is kept, the closure
/// drops its archive instead, so the kept station is shown on every line
/// (RS 1.3, model review finding 1; deviation RS-FX1): its comment stays
/// shown and the outcome and the preview name the closure.
#[test]
fn a_closure_archived_station_keeps_showing_its_kept_end_after_a_merge() {
    for keep_other in [false, true] {
        let mut p = project();
        let a = free_line(&mut p, [0.0, 0.0], run(90.0, 3, 10.0));
        let b = free_line(&mut p, [100.0, 50.0], run(0.0, 2, 5.0));
        let (a0, a3, b0) = (at(&p, a, 0), at(&p, a, 3), at(&p, b, 0));
        p.set_shown_name(b0, "B0".to_string());
        p.set_shown_comment(b0, Some("kept".to_string()));
        // A same-line closure: A:3 shows A:0 ("0" twice on A, one identity).
        let closure =
            ok(p.create_closure(a3, Some(a), a0, Some(a), ClosureStrategy::Batch, None, &[]))
                .value
                .closure
                .id;
        p.closures[0].archived = Some(ClosureEnd::A);
        assert_valid(&p);
        assert_eq!(shown(&p, a), ["0", "1", "2", "0"]);
        let keep = if keep_other { b0 } else { a3 };
        let preview = p.merge_preview(a3, b0, keep).unwrap();
        let out = ok(p.merge_stations_auto(a3, b0, keep));
        assert_valid(&p);
        let node = out.value.kept;
        if keep_other {
            assert_eq!(p.shown(node), b0, "the kept station is shown");
            assert_eq!(shown(&p, a), ["0", "1", "2", "B0"]);
            assert_eq!(shown(&p, b)[0], "B0");
            assert_eq!(p.shown_comment(node), Some("kept"));
            assert_eq!(out.value.unarchived, Some(closure));
            assert_eq!(out.identities.unarchived, vec![closure]);
            assert_eq!(preview.unarchived, Some(closure));
        } else {
            assert_eq!(p.shown(node), a0, "still shown as A:0");
            assert_eq!(shown(&p, b)[0], "0");
            assert_eq!(out.value.unarchived, None);
            // B0's comment goes with it: the dialog's own `hidden`.
            assert_eq!(preview.hidden.0, b0);
        }
        assert!(preview.comments_hidden.is_empty());
        let candidate = p.merge_preview(at(&p, a, 1), b0, b0);
        assert!(candidate.is_none(), "one piece now");
    }
}

/// RS 4.1 default keep: the station holding a datum, else rule K over the
/// lines each is shown on (M-P2-5: a junction shown on a long line through a
/// record counts that line).
#[test]
fn the_default_keep_is_the_datum_holder_else_rule_k() {
    let mut p = project();
    let long = free_line(&mut p, [0.0, 0.0], run(90.0, 10, 50.0));
    let short = free_line(&mut p, [0.0, 100.0], run(0.0, 1, 5.0));
    let side = free_line(&mut p, [100.0, 100.0], run(0.0, 2, 25.0));
    let (j, s0) = (at(&p, short, 1), at(&p, long, 4));
    // `short`'s end is kept at a junction with `long`: shown on both.
    ok(p.merge_stations_auto(s0, j, j));
    let side0 = at(&p, side, 0);
    assert_eq!(p.lines_shown_on(j), vec![long, short]);
    assert_eq!(
        p.rule_k_nodes(&[j, side0]),
        Some(j),
        "the 500 m line counts"
    );
    assert_eq!(p.merge_default_keep(side0, j), j);
    ok(p.set_datum_at(StationRef::new(side, 0), origin(), None));
    assert_eq!(p.merge_default_keep(side0, j), side0, "the datum holder");
}

// ----- rule K and rule L -------------------------------------------------

/// Rule K: the longest line (Normal and Jump shots, Reference excluded, as
/// the statistics measure it), ties to the lowest Station ID, a candidate
/// with no line loses.
#[test]
fn rule_k_takes_the_longest_line_then_the_lowest_id() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 2, 10.0));
    let b = free_line(&mut p, [50.0, 0.0], run(0.0, 2, 10.0));
    p.line_mut(b).unwrap().shots[0].kind = ShotKind::Jump;
    assert_eq!(p.line(b).unwrap().measured_length(), 20.0);
    assert_eq!(p.rule_k([(7, vec![a]), (5, vec![b])]), Some(5), "tie");
    p.line_mut(b).unwrap().shots[1].kind = ShotKind::Reference;
    assert_eq!(p.line(b).unwrap().measured_length(), 10.0);
    assert_eq!(p.rule_k([(7, vec![a]), (5, vec![b])]), Some(7));
    assert_eq!(
        p.rule_k([(1, vec![]), (9, vec![b])]),
        Some(9),
        "no line loses"
    );
    assert_eq!(p.rule_k(Vec::new()), None);
}

/// Rule L over many automatic merges: an identity is never lettered when it
/// is shown on more lines than the one it collided with, the letter is free
/// on every line it is shown on, and the survey stays valid.
#[test]
fn rule_l_never_letters_an_identity_shown_on_more_lines() {
    for seed in 0..24_u64 {
        let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut below = |n: usize| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % n as u64) as usize
        };
        let mut p = project();
        let mut lines = Vec::new();
        for i in 0..6 {
            let at_xy = [f64::from(i * 200), 0.0];
            lines.push(free_line(&mut p, at_xy, run(30.0, 2 + below(4), 5.0)));
        }
        let mut lettered = 0;
        for _ in 0..8 {
            let (la, lb) = (lines[below(lines.len())], lines[below(lines.len())]);
            let a = at(&p, la, below(p.line(la).unwrap().station_count()));
            let b = at(&p, lb, below(p.line(lb).unwrap().station_count()));
            let keep = if below(2) == 0 { a } else { b };
            let Ok(out) = p.merge_stations_auto(a, b, keep) else {
                continue;
            };
            assert_valid(&p);
            for letter in &out.identities.lettered {
                lettered += 1;
                let (mine, theirs) = (
                    p.lines_shown_on(letter.station).len(),
                    p.lines_shown_on(letter.other).len(),
                );
                assert!(
                    mine <= theirs,
                    "seed {seed}: {letter:?} ({mine} > {theirs})"
                );
                assert!(p.conflicts_with_name(letter.station, &letter.to).is_empty());
            }
        }
        assert!(p.name_collisions().is_empty());
        let _ = lettered;
    }
}

/// Rule L's letter is the first letter of the name free on every line the
/// lettered station is shown on (`5` -> `5b` past a `5a`), found from
/// per-line cursors (data review finding data-2). When its lines take more
/// than 64 letters in a row between them, it is the first letter no slot of
/// the survey shows, free on its lines too, so a crafted survey whose lines
/// take letters in turn cannot make each search walk every letter: here
/// `5cs` (suffix number 96) is free on both of X's lines but shown on a
/// third line, so X gets `5ct`.
#[test]
fn rule_l_letters_free_on_every_line_and_bounds_its_search() {
    use crate::station_naming::letter_candidate;
    let rename = |p: &mut Project, line: LineId, index: usize, name: String| {
        let id = at(p, line, index);
        if let Some(s) = p.stations.get_mut(&id) {
            s.name = name;
        }
    };
    // Exact: Main shows 5, 5, 5a; the later 5 (newer id, tie) takes 5b.
    let mut p = project();
    let main = free_line(&mut p, [0.0, 0.0], run(10.0, 3, 5.0));
    for (i, name) in ["0", "5", "5", "5a"].iter().enumerate() {
        rename(&mut p, main, i, (*name).to_string());
    }
    let letters = p.letter_collisions_with(std::collections::BTreeSet::new);
    assert_eq!(letters.len(), 1);
    assert_eq!(
        (letters[0].station, letters[0].to.as_str()),
        (at(&p, main, 2), "5b")
    );
    assert!(p.name_collisions().is_empty());

    // Bounded: X (on A and B) collides with Y (on A and two more lines) by
    // 5 on A; A shows the even suffix numbers 0 to 94 (5a, 5c, ..) and B
    // the odd ones 1 to 95, so 96 probes would be needed; C shows 96.
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(10.0, 50, 5.0));
    let (x, y) = (at(&p, a, 1), at(&p, a, 2));
    let b = branch(&mut p, x, run(100.0, 48, 5.0));
    let y1 = branch(&mut p, y, run(200.0, 1, 5.0));
    let y2 = branch(&mut p, y, run(300.0, 1, 5.0));
    let c = free_line(&mut p, [5000.0, 0.0], run(10.0, 1, 5.0));
    rename(&mut p, a, 0, "start".to_string());
    rename(&mut p, a, 1, "5".to_string());
    rename(&mut p, a, 2, "5".to_string());
    for i in 3..=50_u64 {
        rename(&mut p, a, i as usize, letter_candidate("5", 2 * (i - 3)));
    }
    for i in 1..=48_u64 {
        rename(
            &mut p,
            b,
            i as usize,
            letter_candidate("5", 2 * (i - 1) + 1),
        );
    }
    rename(&mut p, y1, 1, "y1".to_string());
    rename(&mut p, y2, 1, "y2".to_string());
    rename(&mut p, c, 0, letter_candidate("5", 96));
    rename(&mut p, c, 1, "c1".to_string());
    assert_eq!(letter_candidate("5", 96), "5cs");
    let letters = p.letter_collisions_with(std::collections::BTreeSet::new);
    assert_eq!(letters.len(), 1);
    assert_eq!((letters[0].station, letters[0].other), (x, y));
    assert_eq!(letters[0].to, "5ct");
    assert!(p.name_collisions().is_empty());
}

// ----- the epilogue: restores (RS 4.3) -------------------------------------

/// RS 4.3: deleting Far Room's last shot leaves its end on Upstream alone,
/// which a record names, so the node takes that record's identity (Station
/// ID and name), as Detach would give it back. The record had no comment, so
/// Far Room's comment moves with the node (nothing the user saw is lost);
/// the old identity stays as a history record. Nothing moves; Undo is exact.
/// Injection: skip `restore_archived_identities` in the edit epilogue and
/// the delete is refused (A4).
#[test]
fn a_delete_restores_the_record_and_carries_the_comment_over() {
    let (mut p, far, up) = far_room_and_upstream();
    let f3 = at(&p, far, 3);
    let u0 = p.station(f3).unwrap().archived[0].id;
    let before = p.clone();
    let last = p.line(far).unwrap().shots[2].id;
    let edited = p.delete_shot(far, last).unwrap();
    assert_valid(&p);
    assert_eq!(edited.report.reidentified, vec![(f3, u0)]);
    let restored = &edited.report.identities.restored;
    assert_eq!(restored.len(), 1);
    assert!(restored[0].comment_moved && restored[0].comment_hidden.is_none());
    assert_eq!(at(&p, up, 0), u0, "Upstream's start has its own id again");
    assert!(p.station(f3).is_none());
    let back = p.station(u0).unwrap();
    assert_eq!(back.name, "0");
    assert_eq!(back.comment.as_deref(), Some("Far Room: large chamber"));
    assert_eq!(back.archived, vec![record(f3, "3", vec![])], "history");
    assert_eq!(shown(&p, up), ["0", "1", "2", "3"], "nothing was lettered");
    undo_is_exact(&before, &p);
}

/// RS 4.3 with two comments: the restored record's own comment is shown, and
/// the old one stays in the history record and is named by the report
/// (U-P1-3).
#[test]
fn a_delete_restore_names_the_comment_it_hides() {
    let (mut p, far, up) = far_room_and_upstream();
    let f3 = at(&p, far, 3);
    p.station_mut(f3).unwrap().archived[0].comment = Some("Upstream: sump".to_string());
    let edited = p.delete_line(far).unwrap();
    assert_valid(&p);
    let u0 = at(&p, up, 0);
    assert_eq!(p.stations[&u0].comment.as_deref(), Some("Upstream: sump"));
    let restored = &edited.report.identities.restored;
    assert!(!restored[0].comment_moved);
    assert_eq!(
        restored[0].comment_hidden.as_deref(),
        Some("Far Room: large chamber")
    );
    assert_eq!(
        p.stations[&u0].archived[0].comment.as_deref(),
        Some("Far Room: large chamber")
    );
}

/// RS 4.3 by rule K: a station a delete leaves on two lines, each named by a
/// record, takes the identity of the record on the longer line; the other
/// record stays (its line still shows the junction with a dot); the datum
/// follows the new id; Undo is exact.
#[test]
fn a_delete_restore_picks_the_record_on_the_longest_line() {
    let mut p = project();
    let main = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let j = at(&p, main, 1);
    ok(p.set_datum_at(StationRef::new(main, 1), origin(), None));
    let a = free_line(&mut p, [50.0, 50.0], run(0.0, 2, 5.0));
    let b = free_line(&mut p, [80.0, -50.0], run(180.0, 2, 8.0));
    let (a0, b0) = (at(&p, a, 0), at(&p, b, 0));
    ok(p.merge_stations_auto(a0, j, j));
    ok(p.merge_stations_auto(b0, j, j));
    let before = p.clone();
    let edited = p.delete_line(main).unwrap();
    assert_valid(&p);
    assert_eq!(edited.report.reidentified, vec![(j, b0)], "B is longer");
    assert_eq!((at(&p, a, 0), at(&p, b, 0)), (b0, b0), "still one junction");
    let archived = &p.station(b0).unwrap().archived;
    assert_eq!(archived[0], record(a0, "0", vec![a]));
    assert_eq!(archived[1].id, j);
    assert!(archived[1].is_history());
    assert!(p.is_foreign_slot(StationRef::new(a, 0)));
    assert!(!p.is_foreign_slot(StationRef::new(b, 0)));
    assert_eq!(p.datums[0].station, b0, "the datum follows its station");
    undo_is_exact(&before, &p);
}

/// The epilogue enforces A4 on EVERY node, not only those that lost a slot:
/// a node whose records name all its lines (as a crafted state or a Combine
/// could leave it) is restored by the next command, whatever it edits.
#[test]
fn the_epilogue_restores_every_node_without_a_native_line() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let j = at(&p, a, 2);
    let b = branch(&mut p, j, run(0.0, 1, 5.0));
    let (r1, r2) = (p.mint_station_id().unwrap(), p.mint_station_id().unwrap());
    let station = p.station_mut(j).unwrap();
    station.archived = vec![record(r1, "A's", vec![a]), record(r2, "B's", vec![b])];
    assert!(p
        .validate()
        .contains(&Violation::NoNativeLine { station: j }));
    let other = free_line(&mut p, [500.0, 0.0], run(0.0, 1, 1.0));
    let out = ok(p.set_datum_at(StationRef::new(other, 0), origin(), None));
    assert_valid(&p);
    assert_eq!(out.identities.restored.len(), 1);
    assert_eq!(out.reidentified, vec![(j, r1)], "A is longer");
    assert_eq!(p.stations[&r1].name, "A's");
}

// ----- detach and tears -----------------------------------------------------

/// Merge (either keep) then Detach of the archived station's line restores
/// every shown identity and record as it was: the property of RS 12 on one
/// survey, both ways round.
#[test]
fn merge_then_detach_of_the_archived_line_restores_every_identity() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 4, 10.0));
    let b = free_line(&mut p, [100.0, 50.0], run(0.0, 2, 5.0));
    let (a4, b0) = (at(&p, a, 4), at(&p, b, 0));
    // Names no line repeats, so nothing collides either way.
    p.station_mut(b0).unwrap().name = "B start".to_string();
    p.station_mut(b0).unwrap().comment = Some("B's start".to_string());
    for (keep, archived_line) in [(a4, b), (b0, a)] {
        let mut q = p.clone();
        ok(q.merge_stations(a4, b0, keep, &[]));
        let node = keep;
        let out = ok(q.detach_line(node, archived_line, &[], None));
        assert_valid(&q);
        assert!(out.value.restored);
        assert_eq!(q.stations, p.stations, "keep {keep}");
        assert_eq!(shown(&q, a), shown(&p, a));
        assert_eq!(shown(&q, b), shown(&p, b));
    }
}

/// RS 4.3 "a slot moves to a new node" (a Combine tear): the fresh node
/// takes the record naming the torn line, so the record never goes stale
/// while its line still passes a node there.
#[test]
fn a_tear_gives_the_torn_line_its_record() {
    let (mut p, far, up) = far_room_and_upstream();
    let f3 = at(&p, far, 3);
    let u0 = p.stations[&f3].archived[0].id;
    let report = p.detach_ids(f3, up, DetachMode::Tear).unwrap();
    assert_eq!(report.new_station, u0);
    assert!(report.restored);
    assert_eq!(at(&p, up, 0), u0);
    assert_eq!(p.stations[&u0].name, "0");
    assert!(p.stations[&f3].archived.is_empty());
}

// ----- records through splits, range moves and Add .stalx -----------------

/// RS 4.4 range moves: a split delete moves the tail's stations to the new
/// line, and the records naming the old line at those stations follow (like
/// `via` hints).
#[test]
fn a_range_split_repoints_the_records_of_the_stations_it_moves() {
    let mut p = project();
    let main = free_line(&mut p, [0.0, 0.0], run(90.0, 6, 10.0));
    let m5 = at(&p, main, 5);
    let side = free_line(&mut p, [55.0, 30.0], run(0.0, 2, 5.0));
    let s2 = at(&p, side, 2);
    // Main's station 5 merged into Side's end, kept: a record names Main.
    ok(p.merge_stations_auto(m5, s2, s2));
    assert_eq!(p.stations[&s2].archived[0].lines, vec![main]);
    let shots = &p.line(main).unwrap().shots;
    let (first, last) = (shots[1].id, shots[2].id);
    let edited = p
        .delete_shot_range(
            main,
            first,
            last,
            &RangeDeleteMode::Split {
                name_a: "Front".to_string(),
                name_b: "Back".to_string(),
            },
        )
        .unwrap();
    assert_valid(&p);
    let back = edited.value.new_line.unwrap();
    assert_eq!(p.stations[&s2].archived[0].lines, vec![back]);
    let out = ok(p.detach_line(s2, back, &[], None));
    assert_eq!(out.value.new_station, m5, "the record still restores");
}

/// RS 4.4 Add .stalx: archived ids are shifted, their lines remapped, and a
/// closure keeps its archived end.
#[test]
fn add_stalx_shifts_records_and_keeps_closure_archives() {
    let (mut source, far, up) = far_room_and_upstream();
    let (u1, u3) = (at(&source, up, 1), at(&source, up, 3));
    ok(source.create_closure(
        u3,
        Some(up),
        u1,
        Some(up),
        ClosureStrategy::Batch,
        None,
        &[],
    ));
    source.closures[0].archived = Some(ClosureEnd::B);
    source.station_mut(u1).unwrap().name = "mid".to_string();
    assert_valid(&source);
    let mut p = project();
    free_line(&mut p, [0.0, 0.0], run(0.0, 2, 1.0));
    let base = p.counters.next_station_id;
    let out = ok(p.append_project(&source));
    assert_valid(&p);
    let line_of = |old: LineId| {
        out.value
            .line_map
            .iter()
            .find(|(from, _)| *from == old)
            .unwrap()
            .1
    };
    let junction = at(&source, far, 3) + base;
    let records = &p.stations[&junction].archived;
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].id,
        source.stations[&(junction - base)].archived[0].id + base
    );
    assert_eq!(records[0].lines, vec![line_of(up)]);
    let copied = p.closures.last().unwrap();
    assert_eq!(copied.archived, Some(ClosureEnd::B));
    assert_eq!(p.shown(u1 + base), u3 + base);
}

// ----- closures: shown identity, C1, C2, N8 ---------------------------------

/// Three placed pieces with closures X -> K (X archived) and K -> Y (K
/// archived): X shows Y through K. Merging X with Y would make the archives
/// cycle (M-P1-2's second sequence): the normalisation drops the later
/// archive (list order), reports it, and the survey validates; the names the
/// merge shows anew are lettered by rule L.
#[test]
fn a_merge_that_would_cycle_the_closure_archives_drops_the_later_one() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 2, 10.0));
    let b = free_line(&mut p, [500.0, 0.0], run(0.0, 1, 10.0));
    let c = free_line(&mut p, [-500.0, 0.0], run(0.0, 2, 10.0));
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    ok(p.set_datum_at(StationRef::new(b, 0), gps_at(500.0, 0.0), None));
    ok(p.set_datum_at(StationRef::new(c, 0), gps_at(-500.0, 0.0), None));
    let (x, k, y) = (at(&p, a, 2), at(&p, b, 1), at(&p, c, 2));
    let c1 = ok(p.create_closure(x, Some(a), k, Some(b), ClosureStrategy::Batch, None, &[]))
        .value
        .closure
        .id;
    let c2 = ok(p.create_closure(k, Some(b), y, Some(c), ClosureStrategy::Batch, None, &[]))
        .value
        .closure
        .id;
    p.closures[0].archived = Some(ClosureEnd::A);
    p.closures[1].archived = Some(ClosureEnd::A);
    assert_valid(&p);
    assert_eq!((p.shown(x), p.shown(k), p.shown(y)), (y, y, y));
    assert_eq!(p.nodes_showing(y), vec![x, k, y]);
    // Keeping Y: X's closure c1 drops its archive so Y is shown on X's line
    // (RS 1.3, deviation RS-FX1), and K keeps showing Y through c2.
    let mut q = p.clone();
    let out = ok(q.merge_stations_auto(x, y, y));
    assert_valid(&q);
    assert_eq!(out.identities.unarchived, vec![c1]);
    assert_eq!((q.shown(out.value.kept), q.shown(k)), (y, y));
    assert!(q
        .closures
        .iter()
        .any(|cl| cl.id == c2 && cl.archived.is_some()));
    // Keeping X, which shows Y through K: c1 still archives the merged node,
    // and c2 would close the cycle, so the normalisation drops c2's.
    let out = ok(p.merge_stations_auto(x, y, x));
    assert_valid(&p);
    assert_eq!(out.identities.unarchived, vec![c2]);
    let node = out.value.kept;
    assert_eq!(p.shown(node), k, "c1 still archives the merged node");
    assert_eq!(p.shown(k), k);
    assert!(p
        .closures
        .iter()
        .any(|cl| cl.id == c1 && cl.archived.is_some()));
}

/// N8 allows two slots of one shown identity on a line (a same-line
/// closure), and a name another station shows there still collides; the
/// normalisation keeps a valid archive.
#[test]
fn a_same_line_closure_shows_one_identity_twice() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 4, 10.0));
    let (a0, a1, a4) = (at(&p, a, 0), at(&p, a, 1), at(&p, a, 4));
    ok(p.create_closure(a4, Some(a), a0, Some(a), ClosureStrategy::Batch, None, &[]));
    p.closures[0].archived = Some(ClosureEnd::A);
    assert_valid(&p);
    assert_eq!(shown(&p, a), ["0", "1", "2", "3", "0"]);
    assert!(p.is_closure_archived(a4));
    assert!(
        !p.is_foreign_slot(StationRef::new(a, 4)),
        "A is native at A:0"
    );
    assert_eq!(
        p.conflicts_with_name(a1, "0"),
        vec![super::NameConflict { line: a, other: a0 }]
    );
    assert!(p.clone().normalise_closure_archives().is_empty());
}

/// C1 and C2 in a crafted file are reported, and nothing that resolves a
/// shown identity hangs on a cycle (the resolution is bounded): `shown`
/// returns the node itself, N8 and the wire still run.
#[test]
fn crafted_closure_archives_report_instead_of_hanging() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 4, 10.0));
    let (a0, a2, a4) = (at(&p, a, 0), at(&p, a, 2), at(&p, a, 4));
    p.push_closure(a0, None, a4, None, ClosureStrategy::Batch);
    p.push_closure(a4, None, a0, None, ClosureStrategy::Batch);
    p.push_closure(a0, None, a2, None, ClosureStrategy::Batch);
    p.closures[0].archived = Some(ClosureEnd::A); // a0 -> a4
    p.closures[1].archived = Some(ClosureEnd::A); // a4 -> a0: a cycle
    p.closures[2].archived = Some(ClosureEnd::A); // a0 again: C1
    let violations = p.validate();
    let (c0, c1, c2) = (p.closures[0].id, p.closures[1].id, p.closures[2].id);
    assert!(violations.contains(&Violation::StationArchivedTwice {
        station: a0,
        first: c0,
        second: c2
    }));
    assert!(violations.contains(&Violation::ClosureArchiveCycle {
        closure: c0,
        station: a0
    }));
    assert!(violations.contains(&Violation::ClosureArchiveCycle {
        closure: c1,
        station: a4
    }));
    assert_eq!(p.shown(a0), a0, "a cycle resolves to the node itself");
    let _ = p.stations_wire(&p.raw_positions());
    let _ = p.name_collisions();
    let mut fixed = p.clone();
    assert_eq!(fixed.normalise_closure_archives(), vec![c1, c2]);
    assert!(fixed
        .validate()
        .iter()
        .all(|v| !matches!(v, Violation::ClosureArchiveCycle { .. })));
}

/// A1: an archived id is unique among live and archived ids, below 2^31 and
/// below the counter; A5: a record with no line is history, shown nowhere.
#[test]
fn a1_and_a5_hold_for_records() {
    let (mut p, far, up) = far_room_and_upstream();
    // Upstream's own 3 sits beside the dotted 3 (RS-FX5); once the record
    // is history below, Upstream's slot is native and would repeat it.
    let u3 = at(&p, up, 3);
    p.station_mut(u3).unwrap().name = "own 3".to_string();
    let f3 = at(&p, far, 3);
    let u0 = p.stations[&f3].archived[0].id;
    let mut twice = p.clone();
    let f0 = at(&twice, far, 0);
    twice
        .station_mut(f0)
        .unwrap()
        .archived
        .push(record(u0, "copy", vec![]));
    assert!(twice.validate().iter().any(|v| matches!(
        v,
        Violation::ArchivedIdCollision { archived, .. } if *archived == u0
    )));
    p.station_mut(f3).unwrap().archived[0].lines.clear();
    assert_valid(&p);
    assert!(p.stations[&f3].archived[0].is_history());
    assert!(
        !p.nodes_showing(f3).contains(&u0),
        "history is shown nowhere"
    );
    assert!(p.is_archived_id(u0));
}
