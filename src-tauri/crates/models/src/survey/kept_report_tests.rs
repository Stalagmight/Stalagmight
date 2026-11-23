//! Package RS-FX1 (plan 2026-11-23, kept and archived stations): every op's
//! preview and report names what it hides, letters by rule L on what each
//! identity is shown on once the closure archives settle, and accepts the
//! answers its preview suggests; a merge, a promotion and a Combine join
//! show the station they keep. One test per farm finding or model review
//! finding, each named in its doc. Fully synthetic data.

use crate::StationRef;

use super::archive_tests::shown;
use super::ops_test_helpers::{assert_valid, branch, free_line, ok, run};
use super::test_helpers::{add, at, project};
use super::{ClosureStrategy, LineId, Project, RangeDeleteMode, StartAt, StationId};

const BATCH: ClosureStrategy = ClosureStrategy::Batch;

/// Give each station of `line` the name `{prefix}{index}`.
fn name_line(p: &mut Project, line: LineId, prefix: &str) {
    let chain = p.line_stations(line).unwrap().to_vec();
    for (i, s) in chain.into_iter().enumerate() {
        p.station_mut(s).unwrap().name = format!("{prefix}{i}");
    }
}

fn comment(p: &mut Project, station: StationId, text: &str) {
    p.station_mut(station).unwrap().comment = Some(text.to_string());
}

/// Farm 56ce8638669d (seed 400002693, "delete shot"), model review finding
/// 1 on the sweep path: a delete leaves a closure from the main line's M0
/// to the loop end S3 of a branch the only bridge, and S3's own closure
/// archives it (shows S1). The promotion keeps M0 (the survivor); before,
/// the fold moved S3's archive onto M0, which then showed S1 on every line:
/// M0 and its comment were shown nowhere and nothing said so. Now that
/// closure drops its archive (RS 1.3), M0 is shown on both lines, and the
/// report names the closure. Injection: drop the `unarchive_for_keep` call
/// in `sweep_orphan_closures_moving`.
#[test]
fn a_promotion_shows_the_station_it_keeps_on_a_loop_ends_lines() {
    let mut p = project();
    let main = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let m2 = at(&p, main, 2);
    let side = branch(&mut p, m2, run(0.0, 3, 10.0));
    name_line(&mut p, main, "M");
    name_line(&mut p, side, "S");
    p.station_mut(m2).unwrap().name = "M2".to_string();
    let (m0, s1, s3) = (at(&p, main, 0), at(&p, side, 1), at(&p, side, 3));
    comment(&mut p, m0, "survivor note");
    let loop_end = ok(p.create_closure(s3, Some(side), s1, Some(side), BATCH, Some(s1), &[]))
        .value
        .closure
        .id;
    // The bridge, end b on M0: with no datum deciding, end a's piece moves,
    // so M0 is the promotion's survivor.
    ok(p.create_closure(s3, Some(side), m0, Some(main), BATCH, None, &[]));
    assert_valid(&p);
    assert_eq!(p.shown(s3), s1);
    let shot = p.line(side).unwrap().shots[0].id;
    let preview = p.preview_delete_shot(side, shot).unwrap();
    let edited = p.delete_shot(side, shot).unwrap();
    assert_valid(&p);
    let promoted = &edited.report.sweep.promoted;
    assert_eq!(promoted.len(), 1, "{promoted:?}");
    assert_eq!(
        (promoted[0].kept, promoted[0].unarchived),
        (m0, Some(loop_end))
    );
    let end = *p.line_stations(side).unwrap().last().unwrap();
    assert_eq!(p.shown(end), m0, "the kept station is shown");
    assert_eq!(p.shown_comment(end), Some("survivor note"));
    assert_eq!(shown(&p, side).last().map(String::as_str), Some("M0"));
    assert!(edited.report.identities.unarchived.contains(&loop_end));
    assert!(edited.report.identities.hidden.is_empty());
    assert!(preview.identities.unarchived.contains(&loop_end));
}

/// Farm c36d3c33f104 (seed 400010005, "delete line"): a promotion merged X
/// (shown as Y through c1) with W (Y's kept end through c2) keeping X, so
/// the closure archives cycled; the sweep lettered in that state, where
/// every node of the cycle shows itself, and gave Y (shown on three lines
/// once the normalisation broke the cycle) the letter against B1, shown on
/// one. The sweep now normalises before rule L. (The seed's own promotion
/// kept W, which RS-FX1's kept-station rule now settles without a cycle;
/// this one keeps X, the loop end.) Injection: move the sweep's
/// `normalise_closure_archives` after its `letter_collisions`.
#[test]
fn the_sweep_letters_by_rule_l_once_the_closure_archives_settle() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let (a0, a1, a2) = (at(&p, a, 0), at(&p, a, 1), at(&p, a, 2));
    let b = branch(&mut p, a2, run(0.0, 2, 10.0));
    let d = branch(&mut p, a0, run(180.0, 1, 10.0));
    let w = at(&p, d, 1);
    let z = add(&mut p, StartAt::Station(w), Vec::new());
    name_line(&mut p, a, "A");
    name_line(&mut p, b, "B");
    p.station_mut(a2).unwrap().name = "A2".to_string();
    p.station_mut(w).unwrap().name = "W".to_string();
    let (b1, b2) = (at(&p, b, 1), at(&p, b, 2));
    // c1: A1 shows B2; the bridge W - A1 archives nothing (end b, A1, is
    // the survivor the promotion keeps); c2: B2 shows W.
    ok(p.create_closure(a1, Some(a), b2, Some(b), BATCH, Some(b2), &[]));
    ok(p.create_closure(w, Some(z), a1, Some(a), BATCH, None, &[]));
    ok(p.create_closure(b2, Some(b), w, Some(z), BATCH, Some(w), &[]));
    // B2's own name, hidden behind c2, repeats B1's.
    p.station_mut(b2).unwrap().name = "B1".to_string();
    assert_valid(&p);
    assert_eq!((p.shown(a1), p.shown(b2)), (w, w));
    let edited = p.delete_line(d).unwrap();
    assert_valid(&p);
    let promoted = &edited.report.sweep.promoted;
    assert_eq!(promoted.len(), 1);
    assert_eq!((promoted[0].kept, promoted[0].unarchived), (a1, None));
    assert_eq!(edited.report.identities.unarchived.len(), 1, "the cycle");
    let letters = &edited.report.identities.lettered;
    assert!(!letters.is_empty(), "{edited:?}");
    for letter in letters {
        let (mine, theirs) = (
            p.lines_shown_on(letter.station).len(),
            p.lines_shown_on(letter.other).len(),
        );
        assert!(mine <= theirs, "{letter:?}: {mine} lines against {theirs}");
    }
    assert_eq!(p.station(b1).unwrap().name, "B1a");
    assert_eq!(p.station(b2).unwrap().name, "B1");
}

/// X (0..3) and Y (0..2), Y's 1 merged into X's 1 (kept, renamed N), a
/// closure from that junction via Y to X's 3 that archives nothing: the
/// shape of `rs_property_tests::detach_into_a_promotion`. Detaching X gives
/// Y its own 1 back on a piece only the closure joins, so the sweep
/// promotes it.
fn junction_with_a_bridge() -> (Project, LineId, LineId, [StationId; 3]) {
    let mut p = project();
    let x = free_line(&mut p, [0.0, 0.0], run(90.0, 3, 10.0));
    let y = free_line(&mut p, [5.0, 40.0], run(0.0, 2, 6.0));
    let (x1, x3, y1) = (at(&p, x, 1), at(&p, x, 3), at(&p, y, 1));
    ok(p.merge_stations_auto(y1, x1, x1));
    p.set_shown_name(x1, "N".to_string());
    ok(p.create_closure(x1, Some(y), x3, Some(x), BATCH, None, &[]));
    assert_valid(&p);
    (p, x, y, [x1, x3, y1])
}

/// Farm 647783847b78 (seed 480000033, "detach line"): the detach's sweep
/// promoted the closure that alone joined the lines that came off, and the
/// promotion archived a commented station shown before (here X's 3, which
/// rule K's name test gives up because Y shows a 3 already). The Detach
/// preview had no field for it. Injection: leave
/// `DetachNaming::comments_hidden` empty.
#[test]
fn the_detach_preview_names_a_comment_its_promotion_hides() {
    let (mut p, x, y, [x1, x3, y1]) = junction_with_a_bridge();
    let y0 = at(&p, y, 0);
    p.set_shown_name(y0, "3".to_string());
    comment(&mut p, x3, "x3 note");
    assert_valid(&p);
    let naming = p.detach_naming(x1, x, None).unwrap();
    let mut q = p.clone();
    let out = q.detach_line(x1, x, &[], None).unwrap();
    assert_valid(&q);
    assert_eq!(out.sweep.promoted.len(), 1);
    assert_eq!(out.sweep.promoted[0].kept, y1, "the name test keeps Y's 1");
    let hidden: Vec<(StationId, &str)> = naming
        .comments_hidden
        .iter()
        .map(|h| (h.station, h.comment.as_str()))
        .collect();
    assert_eq!(hidden, [(x3, "x3 note")]);
    assert_eq!(out.identities.hidden, naming.comments_hidden);
    let wire = p
        .detach_preview_wire(StationRef::new(x, 1), x, None)
        .unwrap();
    assert_eq!(wire.comments_hidden.len(), 1);
    assert_eq!(wire.comments_hidden[0].comment.as_deref(), Some("x3 note"));
}

/// Farm e546f002311b (seeds 400000326, 400002173, "detach line"): the
/// detach shows Y's own 1 next to a 1 Y already had, but the promotion its
/// sweep makes folds that 1 into X's 3, so no collision is left and the
/// preview (rule L) lists none. The dialog then sends no rename, and the
/// detach refused it ("Line 1 would show two stations named 1"): renames
/// were judged before the sweep. Now an unanswered collision of the op's
/// own refuses only when it still stands after the sweep (or the sweep
/// lettered it). Injection: refuse in `run_op_answered` right after
/// `answers.apply` when `own_collisions` is not empty.
#[test]
fn a_collision_a_promotion_merges_away_needs_no_answer() {
    let (mut p, x, y, [x1, x3, _]) = junction_with_a_bridge();
    let y2 = at(&p, y, 2);
    p.set_shown_name(y2, "1".to_string());
    assert_valid(&p);
    let naming = p.detach_naming(x1, x, None).unwrap();
    assert!(naming.collisions.is_empty(), "{naming:?}");
    let mut q = p.clone();
    let out = q.detach_line(x1, x, &[], None).unwrap();
    assert_valid(&q);
    assert_eq!(out.sweep.promoted.len(), 1);
    assert_eq!(out.sweep.promoted[0].kept, x3);
    assert_eq!(shown(&q, y), ["0", "3", "1"]);
    // A collision that still stands after the sweep is refused unanswered.
    let (mut r, x, y, [x1, _, _]) = junction_with_a_bridge();
    let y2 = at(&r, y, 2);
    r.set_shown_name(y2, "1".to_string());
    r.closures.clear();
    assert_valid(&r);
    assert_eq!(r.detach_naming(x1, x, None).unwrap().collisions.len(), 1);
    let before = r.clone();
    assert!(matches!(
        r.detach_line(x1, x, &[], None),
        Err(super::OpError::NameTaken { .. })
    ));
    assert_eq!(r, before);
}

/// Farm 7a5ed348b262 (seed 480000143, "delete range"): a Split of an
/// interior range leaves the second half joined only by a loop closure,
/// whose promotion archives the half's first station with its comment. The
/// range preview ran only the gap-close, so the Split choice hid a comment
/// it never named. It now carries the split's identities too
/// (`DeleteRangePreview::split_weld`, the wire's `split_identities`).
/// Injection: set `split_weld` to `None` in `preview_delete_shot_range`.
#[test]
fn the_range_preview_names_what_split_would_hide() {
    let mut p = project();
    let l = free_line(&mut p, [0.0, 0.0], run(90.0, 4, 10.0));
    name_line(&mut p, l, "L");
    let (l0, l3) = (at(&p, l, 0), at(&p, l, 3));
    comment(&mut p, l3, "far note");
    ok(p.create_closure(l3, Some(l), l0, Some(l), BATCH, None, &[]));
    assert_valid(&p);
    let shot = p.line(l).unwrap().shots[2].id;
    let preview = p.preview_delete_shot_range(l, shot, shot).unwrap();
    assert!(!preview.shift_blocked, "the gap-close is offered");
    assert!(
        preview.identities.hidden.is_empty(),
        "the gap-close hides nothing"
    );
    let weld = preview.split_weld.as_ref().unwrap();
    assert_eq!(weld.promoted.len(), 1);
    let hidden: Vec<StationId> = weld.identities.hidden.iter().map(|h| h.station).collect();
    let mode = RangeDeleteMode::Split {
        name_a: "LA".to_string(),
        name_b: "LB".to_string(),
    };
    let mut q = p.clone();
    let edited = q.delete_shot_range(l, shot, shot, &mode).unwrap();
    assert_valid(&q);
    let done: Vec<StationId> = edited
        .report
        .identities
        .hidden
        .iter()
        .map(|h| h.station)
        .collect();
    assert_eq!(done, hidden, "the preview says what the split does");
    assert!(
        hidden.contains(&l3) || hidden.contains(&l0),
        "a promoted end's comment is hidden: {hidden:?}"
    );
    let wire = p.delete_range_impact(&preview);
    let split = wire.split_identities.unwrap();
    assert_eq!(split.hidden.len(), hidden.len());
}

/// Farm 4f602aeda8e9 (seed 480000296, "delete line"): deleting line L
/// leaves junction N on L2 only, so N takes back L2's own station R (RS
/// 4.3) and N's comment moves to it; R's piece is joined only by a loop
/// closure, whose promotion archives R, comment and all. The report named
/// the move and the promotion but no hidden comment. Its identities now
/// name every comment the op hides, wherever a restore moved it
/// (`HiddenComment::held_by`). Injection: return `self` unchanged from
/// `IdentityReport::with_hidden`.
#[test]
fn delete_line_names_a_comment_a_restore_moved_and_a_promotion_hid() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let a1 = at(&p, a, 1);
    let l = branch(&mut p, a1, run(0.0, 2, 10.0));
    let l2 = free_line(&mut p, [5.0, 40.0], run(90.0, 1, 10.0));
    name_line(&mut p, a, "A");
    name_line(&mut p, l, "L");
    name_line(&mut p, l2, "R");
    p.station_mut(a1).unwrap().name = "A1".to_string();
    let (a2, n, r0) = (at(&p, a, 2), at(&p, l, 2), at(&p, l2, 0));
    comment(&mut p, n, "junction note");
    ok(p.merge_stations_auto(r0, n, n));
    ok(p.create_closure(n, Some(l2), a2, Some(a), BATCH, None, &[]));
    assert_valid(&p);
    let edited = p.delete_line(l).unwrap();
    assert_valid(&p);
    let restored = &edited.report.identities.restored;
    assert_eq!(restored.len(), 1);
    assert!(restored[0].comment_moved);
    assert_eq!(edited.report.sweep.promoted.len(), 1);
    let hidden = &edited.report.identities.hidden;
    assert_eq!(hidden.len(), 1, "{hidden:?}");
    assert_eq!(
        (hidden[0].station, hidden[0].comment.as_str()),
        (n, "junction note")
    );
    assert!(p.is_archived_id(hidden[0].held_by));
    let notice = p.identity_notice(&edited.report.identities);
    assert_eq!(notice.hidden.len(), 1);
}

/// Model review finding 2: a loop closure kept at a junction through the
/// line a record names (its archived end on a third line T) moves with that
/// line when Detach gives the record back, so T shows the restored station
/// there instead of the junction's. The Detach preview listed only the
/// junction's own lines. It now lists every slot shown differently
/// elsewhere. Injection: return an empty list from
/// `Project::shown_elsewhere`.
#[test]
fn the_detach_preview_names_slots_shown_differently_elsewhere() {
    let mut p = project();
    let x = free_line(&mut p, [0.0, 0.0], run(90.0, 3, 10.0));
    let y = free_line(&mut p, [5.0, 40.0], run(0.0, 2, 6.0));
    name_line(&mut p, x, "X");
    name_line(&mut p, y, "Y");
    let (x1, y1, y2) = (at(&p, x, 1), at(&p, y, 1), at(&p, y, 2));
    ok(p.merge_stations_auto(y1, x1, x1));
    let t = branch(&mut p, y2, run(90.0, 2, 5.0));
    name_line(&mut p, t, "T");
    p.station_mut(y2).unwrap().name = "Y2".to_string();
    let t2 = at(&p, t, 2);
    ok(p.create_closure(x1, Some(y), t2, Some(t), BATCH, Some(x1), &[]));
    assert_valid(&p);
    assert_eq!(shown(&p, t), ["Y2", "T1", "X1"]);
    let naming = p.detach_naming(x1, y, None).unwrap();
    assert!(naming.restored);
    let elsewhere: Vec<(StationRef, StationId, StationId)> = naming
        .shown_elsewhere
        .iter()
        .map(|e| (e.slot, e.before.0, e.after.0))
        .collect();
    assert_eq!(elsewhere, [(StationRef::new(t, 2), x1, y1)]);
    let mut q = p.clone();
    ok(q.detach_line(x1, y, &[], None));
    assert_eq!(shown(&q, t), ["Y2", "T1", "Y1"], "as the preview says");
    let wire = p
        .detach_preview_wire(StationRef::new(y, 1), y, None)
        .unwrap();
    assert_eq!(wire.shown_elsewhere.len(), 1);
    assert_eq!(wire.shown_elsewhere[0].after.name, "Y1");
}

/// Model review finding 2, `shown_before` in cases 1 and 2: a junction a
/// loop closure archives shows its kept end K on every line; detaching the
/// line a record names gives it the record (case 1), so it no longer shows
/// K, and the preview says so (it said it only in case 3). Injection:
/// restore `!report.restored && !report.kept &&` in `detach_naming`.
#[test]
fn the_detach_preview_says_a_line_stops_showing_a_loop_closures_station() {
    let mut p = project();
    let x = free_line(&mut p, [0.0, 0.0], run(90.0, 3, 10.0));
    let y = free_line(&mut p, [5.0, 40.0], run(0.0, 2, 6.0));
    name_line(&mut p, x, "X");
    name_line(&mut p, y, "Y");
    let (x0, x2, y1) = (at(&p, x, 0), at(&p, x, 2), at(&p, y, 1));
    ok(p.merge_stations_auto(y1, x2, x2));
    ok(p.create_closure(x2, Some(x), x0, Some(x), BATCH, Some(x0), &[]));
    assert_valid(&p);
    assert_eq!(p.shown(x2), x0);
    let naming = p.detach_naming(x2, y, None).unwrap();
    assert!(naming.restored);
    assert_eq!(naming.shown_before, Some(x0));
}

/// Model review finding 4: merging junction X (its own line LX native, LX2
/// named by X's record R) into Y keeping Y moves R onto Y's node; Detach of
/// LX later gives X back on LX alone, and LX2 keeps showing Y. The merge
/// preview now names LX2 (`not_given_back`), so the dialog does not promise
/// more than Detach gives. Injection: leave `MergeMove::not_given_back`
/// empty.
#[test]
fn the_merge_preview_names_the_lines_detach_does_not_give_back() {
    let mut p = project();
    let lx = free_line(&mut p, [0.0, 0.0], run(90.0, 1, 10.0));
    let lx2 = free_line(&mut p, [0.0, 50.0], run(90.0, 1, 10.0));
    let ly = free_line(&mut p, [0.0, 100.0], run(90.0, 1, 10.0));
    name_line(&mut p, lx, "X");
    name_line(&mut p, lx2, "R");
    name_line(&mut p, ly, "Y");
    let (x1, r0, y1) = (at(&p, lx, 1), at(&p, lx2, 0), at(&p, ly, 1));
    ok(p.merge_stations_auto(r0, x1, x1));
    assert_valid(&p);
    let preview = p.merge_preview(x1, y1, y1).unwrap();
    assert_eq!(preview.hidden.0, x1);
    assert_eq!(preview.not_given_back, vec![lx2]);
    let wire = p.merge_keep_wire(&preview);
    assert_eq!(wire.not_given_back, vec![lx2]);
    // As the preview says: Detach of LX gives X back on LX only.
    let out = ok(p.merge_stations(x1, y1, y1, &[]));
    let node = out.value.kept;
    ok(p.detach_line(node, lx, &[], None));
    assert_eq!(p.topology().lines_of(x1), vec![lx]);
    assert_eq!(p.shown(at(&p, lx2, 0)), y1);
}

/// `Project::hidden_comments` (RS 12): a comment shown before is hidden when
/// an identity no slot shows keeps it (here a loop end a closure archives);
/// not when another identity shows it now, alone or as one part of a
/// Combine "Both" join (`kept`, a newline, `other`); a text that merely
/// contains it is no such part. Injection: test `contains` instead of
/// `holds_text` fails the last case.
#[test]
fn a_comment_is_hidden_unless_another_station_shows_it() {
    let mut before = project();
    let a = free_line(&mut before, [0.0, 0.0], run(90.0, 3, 10.0));
    name_line(&mut before, a, "A");
    let (a0, a3) = (at(&before, a, 0), at(&before, a, 3));
    comment(&mut before, a3, "x");
    assert_valid(&before);
    let mut after = before.clone();
    ok(after.create_closure(a3, Some(a), a0, Some(a), BATCH, Some(a0), &[]));
    assert_eq!(after.shown(a3), a0);
    let hidden = after.hidden_comments(&before);
    assert_eq!(
        hidden
            .iter()
            .map(|h| (h.station, h.held_by))
            .collect::<Vec<_>>(),
        [(a3, a3)]
    );
    assert_eq!(
        (hidden[0].name.as_str(), hidden[0].comment.as_str()),
        ("A3", "x")
    );
    for (shown, is_hidden) in [
        ("x", false),
        ("A0 note\nx", false),
        ("x\nA0 note", false),
        ("xx", true),
    ] {
        comment(&mut after, a0, shown);
        assert_eq!(
            !after.hidden_comments(&before).is_empty(),
            is_hidden,
            "{shown:?}"
        );
    }
}
