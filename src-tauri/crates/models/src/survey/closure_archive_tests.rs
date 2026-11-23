//! Loop closures that keep one end (plan 2026-11-23, RS 5): Create with a
//! kept end, C1 and C2 and the "Keep both names" default, the names a
//! closure shows twice (answered or lettered by rule L), every removal path
//! bringing the archived end back, chains, the M-P1-2 normalisation after
//! merges and promotions, promotions keeping the kept end with the same
//! layout, edits through a loop end, same-line closures, crafted cycles in
//! the dialogs' reads, and create-then-remove restoring every shown
//! identity. Fully synthetic data.

use crate::{KeepRefusalReason, StationRef};

use super::archive_tests::{record, shown, slot_bits, undo_is_exact};
use super::ops_test_helpers::{assert_valid, branch, free_line, gps_at, ok, origin, run, xy_all};
use super::test_helpers::{at, flat, project};
use super::{
    ClosureEnd, ClosureKeepChoices, ClosureStrategy, LineId, OpError, Project, RangeDeleteMode,
    SlotOwner, StationId,
};

const BATCH: ClosureStrategy = ClosureStrategy::Batch;

/// Name every station of `line` `{prefix}{index}`, except those an earlier
/// line already shows (its junctions).
fn name_line(p: &mut Project, line: LineId, prefix: &str, skip_first: bool) {
    let chain = p.line_stations(line).unwrap().to_vec();
    for (i, s) in chain.into_iter().enumerate() {
        if skip_first && i == 0 {
            continue;
        }
        p.station_mut(s).unwrap().name = format!("{prefix}{i}");
    }
}

/// A loop: A runs east 4 x 10 m (A0..A4); B leaves A0 north 10 m, east
/// 40 m and south 10 m (B1..B3), ending on A4's spot without being merged
/// with it. A measures 40 m, B 60 m. Every name is unique.
fn loop_survey() -> (Project, LineId, LineId) {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 4, 10.0));
    let a0 = at(&p, a, 0);
    let b = branch(
        &mut p,
        a0,
        vec![flat(0.0, 10.0), flat(90.0, 40.0), flat(180.0, 10.0)],
    );
    name_line(&mut p, a, "A", false);
    name_line(&mut p, b, "B", true);
    assert_valid(&p);
    (p, a, b)
}

/// [`loop_survey`] with a third line C leaving A2 south for 100 m (C1).
fn loop_with_tail() -> (Project, [LineId; 3]) {
    let (mut p, a, b) = loop_survey();
    let a2 = at(&p, a, 2);
    let c = branch(&mut p, a2, vec![flat(180.0, 100.0)]);
    name_line(&mut p, c, "C", true);
    assert_valid(&p);
    (p, [a, b, c])
}

/// A closure B3 - A4 keeping A4 (B3 shows A4 on B).
fn close_b3_on_a4(p: &mut Project, a: LineId, b: LineId) -> super::ClosureId {
    let (a4, b3) = (at(p, a, 4), at(p, b, 3));
    ok(p.create_closure(b3, Some(b), a4, Some(a), BATCH, Some(a4), &[]))
        .value
        .closure
        .id
}

// ----- create ----------------------------------------------------------------

/// RS 5.2: a closure keeping one end archives the other: the archived end
/// shows the kept end's name and Station ID on its lines, stays a node of
/// its own (nothing moves), and its own name is free on its line. The
/// dialog's choices, default and preview say so; "Keep both names" archives
/// nothing; a keep that is not an end refuses and changes nothing.
#[test]
fn a_closure_keeps_one_end_and_archives_the_other() {
    let (mut p, a, b) = loop_survey();
    let (a0, a4, b1, b3) = (at(&p, a, 0), at(&p, a, 4), at(&p, b, 1), at(&p, b, 3));
    assert_eq!(
        p.closure_keep_choices(b3, a4),
        ClosureKeepChoices {
            allowed: vec![b3, a4],
            rule_k: Some(b3),
            rule_k_refused: None,
            default: Some(b3),
        },
        "rule K: B is the longer line"
    );
    let naming = p
        .closure_naming((b3, Some(b)), (a4, Some(a)), BATCH, Some(a4))
        .unwrap();
    assert_eq!((naming.kept, naming.archived), (Some(a4), Some(b3)));
    assert_eq!(naming.identity, Some((b3, "B3".to_string(), None)));
    assert_eq!(naming.renamed_lines, vec![b]);
    assert!(naming.collisions.is_empty());

    assert_eq!(
        p.clone()
            .create_closure(b3, Some(b), a4, Some(a), BATCH, Some(a0), &[])
            .err(),
        Some(OpError::InvalidKeep(a0))
    );
    let mut both = p.clone();
    let out = ok(both.create_closure(b3, Some(b), a4, Some(a), BATCH, None, &[]));
    assert_eq!(out.value.closure.archived, None, "keep both names");
    assert_eq!(shown(&both, b), ["A0", "B1", "B2", "B3"]);

    let before = p.clone();
    let positions = xy_all(&p);
    let out = ok(p.create_closure(b3, Some(b), a4, Some(a), BATCH, Some(a4), &[]));
    assert_valid(&p);
    let id = out.value.closure.id;
    assert_eq!(out.value.closure.archived, Some(ClosureEnd::A));
    assert_eq!(xy_all(&p), positions, "a closure moves nothing");
    assert_eq!(shown(&p, a), ["A0", "A1", "A2", "A3", "A4"]);
    assert_eq!(shown(&p, b), ["A0", "B1", "B2", "A4"]);
    assert_eq!(p.shown(b3), a4, "the kept end's Station ID too");
    assert_eq!(
        p.slot_owner(StationRef::new(b, 3)),
        Some(SlotOwner {
            node: b3,
            shown: a4,
            home_line: Some(a),
            foreign: true,
            loop_closure: Some(id),
            own_station: None,
        })
    );
    assert!(
        p.conflicts_with_name(b1, "B3").is_empty(),
        "an archived name is free on its line"
    );
    undo_is_exact(&before, &p);
}

/// RS 5.2: a keep is allowed when the end it archives is not archived by
/// another closure (C1) and the kept end does not already show it (C2).
/// When rule K's end is not allowed the default is "Keep both names"; a
/// refused keep changes nothing.
#[test]
fn a_keep_must_pass_c1_and_c2_else_keep_both_names_is_the_default() {
    let (mut p, [a, b, c]) = loop_with_tail();
    let (a4, b3, c1) = (at(&p, a, 4), at(&p, b, 3), at(&p, c, 1));
    close_b3_on_a4(&mut p, a, b);
    // C1: keeping C's end (rule K: 100 m) would archive B3 a second time.
    // The refusal names that reason: the OTHER end is already a loop end.
    assert_eq!(
        p.closure_keep_choices(b3, c1),
        ClosureKeepChoices {
            allowed: vec![b3],
            rule_k: Some(c1),
            rule_k_refused: Some(KeepRefusalReason::OtherEndArchived),
            default: None,
        }
    );
    let before = p.clone();
    assert_eq!(
        p.create_closure(b3, Some(b), c1, Some(c), BATCH, Some(c1), &[])
            .err(),
        Some(OpError::InvalidKeep(c1))
    );
    assert_eq!(p, before, "a refused keep changes nothing");

    // A4 -> C1: B3 now shows C1 through A4 (a chain).
    ok(p.create_closure(a4, Some(a), c1, Some(c), BATCH, Some(c1), &[]));
    assert_valid(&p);
    assert_eq!((p.shown(b3), p.shown(a4)), (c1, c1));
    assert_eq!(shown(&p, b), ["A0", "B1", "B2", "C1"]);
    assert_eq!(p.lines_shown_on(c1), vec![a, b, c]);
    // C2: keeping B3 would archive C1, which B3 already shows; C1: keeping
    // C1 would archive B3 again. Only "Keep both names" is left.
    assert_eq!(
        p.closure_keep_choices(c1, b3),
        ClosureKeepChoices {
            allowed: vec![],
            rule_k: Some(b3),
            rule_k_refused: Some(KeepRefusalReason::KeptShowsOtherEnd),
            default: None,
        }
    );
    for keep in [b3, c1] {
        assert_eq!(
            p.clone()
                .create_closure(c1, Some(c), b3, Some(b), BATCH, Some(keep), &[])
                .err(),
            Some(OpError::InvalidKeep(keep))
        );
    }
    let out = ok(p.create_closure(c1, Some(c), b3, Some(b), BATCH, None, &[]));
    assert_eq!(out.value.closure.archived, None);
    assert_valid(&p);
}

/// RS-FX5 ("dotted slots don't collide"): a loop end shows its kept end
/// with the loop mark and counts for no uniqueness, so archiving B3 behind
/// A4 shows "A4" on B beside B's own station B1 renamed "A4": the creation
/// needs no answer, letters nothing, and both stay. Before RS-FX5 the
/// creation refused (`NameTaken`) and rule L lettered B1. Injection: count
/// loop ends in `NativeSlots::is_native` and the creation refuses again.
#[test]
fn a_closure_shows_its_kept_end_beside_a_station_of_the_same_name() {
    let (mut p, a, b) = loop_survey();
    let (a4, b1, b3) = (at(&p, a, 4), at(&p, b, 1), at(&p, b, 3));
    p.station_mut(b1).unwrap().name = "A4".to_string();
    assert_valid(&p);
    let naming = p
        .closure_naming((b3, Some(b)), (a4, Some(a)), BATCH, Some(a4))
        .unwrap();
    assert!(naming.collisions.is_empty(), "{:?}", naming.collisions);
    assert!(naming.alternatives.is_empty());
    let mut answered = p.clone();
    let out = ok(answered.create_closure(b3, Some(b), a4, Some(a), BATCH, Some(a4), &[]));
    assert!(out.identities.lettered.is_empty());
    assert_valid(&answered);
    assert_eq!(shown(&answered, b), ["A0", "A4", "B2", "A4"]);
    assert!(answered.is_native_slot(StationRef::new(b, 1)));
    assert!(
        !answered.is_native_slot(StationRef::new(b, 3)),
        "a loop end"
    );
    let out = ok(p.create_closure_auto(b3, Some(b), a4, Some(a), BATCH, Some(a4)));
    assert!(out.identities.lettered.is_empty());
    assert_eq!(p, answered);
}

// ----- remove -----------------------------------------------------------------

/// RS 5.3: Remove Closure brings the archived end's own name back; when the
/// user reused that name on its line meanwhile, the removal needs an answer
/// (the preview's is rule L's: the station new on the line takes the
/// letter), and the form without a dialog letters it.
#[test]
fn removing_a_closure_brings_the_archived_end_back() {
    let (mut p, a, b) = loop_survey();
    let (b1, b3) = (at(&p, b, 1), at(&p, b, 3));
    let id = close_b3_on_a4(&mut p, a, b);
    let mut plain = p.clone();
    let preview = plain.closure_removal(id).unwrap();
    assert_eq!(preview.identity, Some((b3, "B3".to_string(), None)));
    assert_eq!(preview.renamed_lines, vec![b]);
    assert!(preview.collisions.is_empty());
    ok(plain.remove_closure(id, &[]));
    assert_eq!(shown(&plain, b), ["A0", "B1", "B2", "B3"]);

    p.station_mut(b1).unwrap().name = "B3".to_string();
    assert_valid(&p);
    let before = p.clone();
    let preview = p.closure_removal(id).unwrap();
    assert_eq!(preview.collisions.len(), 1);
    let letter = &preview.collisions[0];
    assert_eq!(
        (letter.station, letter.other),
        (b3, b1),
        "tie: B3 is new on B"
    );
    assert_eq!(letter.to, "B3a");
    assert_eq!(
        p.remove_closure(id, &[]).err(),
        Some(OpError::NameTaken {
            line: b,
            name: "B3".to_string()
        })
    );
    assert_eq!(p, before);
    let mut answered = p.clone();
    ok(answered.remove_closure(id, &[(b3, "B end".to_string())]));
    assert_eq!(shown(&answered, b), ["A0", "B3", "B2", "B end"]);
    undo_is_exact(&before, &answered);
    let out = ok(p.remove_closure_auto(id));
    assert_eq!(out.identities.lettered, preview.collisions);
    assert_eq!(shown(&p, b), ["A0", "B3", "B2", "B3a"]);
    assert_valid(&p);
}

/// RS 5.3: removing the middle closure of a chain X -> Y -> K shows Y again
/// on Y's lines and on X's (X now shows Y through its own closure).
#[test]
fn removing_the_middle_of_a_chain_shows_it_again() {
    let (mut p, [a, b, c]) = loop_with_tail();
    let (a4, b3, c1) = (at(&p, a, 4), at(&p, b, 3), at(&p, c, 1));
    close_b3_on_a4(&mut p, a, b);
    let middle = ok(p.create_closure(a4, Some(a), c1, Some(c), BATCH, Some(c1), &[]))
        .value
        .closure
        .id;
    assert_eq!(p.nodes_showing(c1), vec![a4, b3, c1]);
    let preview = p.closure_removal(middle).unwrap();
    assert_eq!(preview.identity, Some((a4, "A4".to_string(), None)));
    assert_eq!(preview.renamed_lines, vec![a, b]);
    ok(p.remove_closure(middle, &[]));
    assert_valid(&p);
    assert_eq!((p.shown(b3), p.shown(a4)), (a4, a4));
    assert_eq!(shown(&p, a)[4], "A4");
    assert_eq!(shown(&p, b)[3], "A4");
}

/// RS 5.3, deleting the kept end: the closure goes with its station and the
/// archived end shows its own name again; a name reused meanwhile is
/// lettered by rule L (no dialog), on the station new on the line.
#[test]
fn deleting_the_kept_end_brings_the_archived_end_back() {
    let (mut p, a, b) = loop_survey();
    let (a4, b1, b3) = (at(&p, a, 4), at(&p, b, 1), at(&p, b, 3));
    close_b3_on_a4(&mut p, a, b);
    p.station_mut(b1).unwrap().name = "B3".to_string();
    assert_valid(&p);
    let last = p.line(a).unwrap().shots[3].id;
    let edited = p.delete_shot(a, last).unwrap();
    assert_valid(&p);
    assert!(p.station(a4).is_none());
    assert!(p.closures.is_empty());
    assert_eq!(p.shown(b3), b3);
    assert_eq!(shown(&p, b), ["A0", "B3", "B2", "B3a"]);
    let lettered = &edited.report.identities.lettered;
    assert_eq!(lettered.len(), 1);
    assert_eq!((lettered[0].station, lettered[0].other), (b3, b1));
}

/// RS 5.3, the #737 removal: an edit that leaves an archiving closure
/// across one shot removes it, and its archived end shows its own name.
#[test]
fn a_closure_left_across_one_shot_gives_its_end_back() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 5, 10.0));
    name_line(&mut p, a, "A", false);
    let (a1, a4) = (at(&p, a, 1), at(&p, a, 4));
    ok(p.create_closure(a4, Some(a), a1, Some(a), BATCH, Some(a1), &[]));
    assert_eq!(shown(&p, a), ["A0", "A1", "A2", "A3", "A1", "A5"]);
    let (first, last) = (
        p.line(a).unwrap().shots[1].id,
        p.line(a).unwrap().shots[2].id,
    );
    let edited = p
        .delete_shot_range(a, first, last, &RangeDeleteMode::Shift)
        .unwrap();
    assert_valid(&p);
    assert_eq!(edited.report.removed_across_one_shot.len(), 1);
    assert!(p.closures.is_empty());
    assert_eq!(shown(&p, a), ["A0", "A1", "A4", "A5"]);
}

// ----- normalisation after node merges (M-P1-2) ------------------------------

/// Two free loops, each with a closure archiving its B3 / E3 (kept: its
/// A4 / D4), names unique across both.
fn two_archived_loops() -> (Project, [LineId; 4], [super::ClosureId; 2]) {
    let (mut p, a, b) = loop_survey();
    let first = close_b3_on_a4(&mut p, a, b);
    let d = free_line(&mut p, [0.0, 500.0], run(90.0, 4, 10.0));
    let d0 = at(&p, d, 0);
    let e = branch(
        &mut p,
        d0,
        vec![flat(0.0, 10.0), flat(90.0, 40.0), flat(180.0, 10.0)],
    );
    name_line(&mut p, d, "D", false);
    name_line(&mut p, e, "E", true);
    let (d4, e3) = (at(&p, d, 4), at(&p, e, 3));
    let second = ok(p.create_closure(e3, Some(e), d4, Some(d), BATCH, Some(d4), &[]))
        .value
        .closure
        .id;
    assert_valid(&p);
    (p, [a, b, d, e], [first, second])
}

/// M-P1-2, C1 through a merge: merging two closure-archived stations would
/// leave the merged node archived twice. The closure archiving the station
/// not kept drops its archive and is reported, so the merged node shows
/// what the kept station showed, on both lines (RS 1.3, deviation RS-FX1:
/// the normalisation's list order no longer decides against the keep).
#[test]
fn merging_two_closure_archived_stations_keeps_the_kept_ones_archive() {
    let (p, [a, b, d, e], [first, second]) = two_archived_loops();
    let (a4, b3, d4, e3) = (at(&p, a, 4), at(&p, b, 3), at(&p, d, 4), at(&p, e, 3));
    for (keep, stays, goes, shows, name) in
        [(b3, first, second, a4, "A4"), (e3, second, first, d4, "D4")]
    {
        let mut q = p.clone();
        let out = ok(q.merge_stations_auto(b3, e3, keep));
        assert_valid(&q);
        assert_eq!(out.identities.unarchived, vec![goes], "keep {keep}");
        let node = out.value.kept;
        assert_eq!(q.shown(node), shows);
        assert_eq!(shown(&q, b)[3], name);
        assert_eq!(shown(&q, e)[3], name);
        assert!(q
            .closures
            .iter()
            .any(|c| c.id == stays && c.archived.is_some()));
        assert!(q
            .closures
            .iter()
            .any(|c| c.id == goes && c.archived.is_none()));
        undo_is_exact(&p, &q);
    }
}

/// M-P1-2, C1 through a promotion: the two loops are placed by datums and
/// tied by a closure between the two archived ends; removing one datum
/// promotes that tie to a merge, and the normalisation settles the two
/// archives on the merged node as a merge does.
#[test]
fn a_promotion_of_two_archived_ends_keeps_the_earlier_archive() {
    let (mut p, [a, b, d, e], [first, second]) = two_archived_loops();
    let (a4, b3, e3) = (at(&p, a, 4), at(&p, b, 3), at(&p, e, 3));
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    ok(p.set_datum_at(StationRef::new(d, 0), gps_at(0.0, 500.0), None));
    let tie = ok(p.create_closure(b3, Some(b), e3, Some(e), BATCH, None, &[]))
        .value
        .closure
        .id;
    assert_valid(&p);
    let out = ok(p.remove_datum(1));
    assert_valid(&p);
    assert_eq!(out.sweep.promoted.len(), 1);
    let promotion = &out.sweep.promoted[0];
    assert_eq!(promotion.closure.id, tie);
    assert_eq!(out.identities.unarchived, vec![second]);
    assert_eq!(p.shown(promotion.kept), a4);
    assert_eq!(shown(&p, e)[3], "A4");
    assert!(p
        .closures
        .iter()
        .any(|c| c.id == first && c.archived.is_some()));
}

// ----- promotions -----------------------------------------------------------

/// RS 5.4: a promotion is a merge that keeps the closure's kept end (the
/// other end becomes the merge record), and today's survivor when the
/// closure archived nothing; geometry first, so the raw layout is
/// bit-identical whichever end is kept.
#[test]
fn a_promotion_keeps_the_closures_kept_end_with_the_same_layout() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 2, 50.0));
    let b = free_line(&mut p, [500.0, 0.0], run(0.0, 2, 50.0));
    name_line(&mut p, a, "A", false);
    name_line(&mut p, b, "B", false);
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    ok(p.set_datum_at(StationRef::new(b, 0), gps_at(30.0, 100.0), None));
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    let mut layouts = Vec::new();
    for (keep, kept, archived, line) in [
        (None, a2, b0, b),
        (Some(a2), a2, b0, b),
        (Some(b0), b0, a2, a),
    ] {
        let mut q = p.clone();
        ok(q.create_closure(a2, Some(a), b0, Some(b), BATCH, keep, &[]));
        let before = q.clone();
        let out = ok(q.remove_datum(1));
        assert_valid(&q);
        let promotion = &out.sweep.promoted[0];
        assert_eq!((promotion.survivor, promotion.retired), (a2, b0));
        assert_eq!((promotion.kept, promotion.archived), (kept, archived));
        assert!(q.station(archived).is_none());
        let name = if archived == a2 { "A2" } else { "B0" };
        assert_eq!(
            q.stations[&kept].archived,
            vec![record(archived, name, vec![line])]
        );
        assert!(q.closures.is_empty());
        layouts.push(slot_bits(&q));
        undo_is_exact(&before, &q);
    }
    assert!(layouts.windows(2).all(|w| w[0] == w[1]), "bit for bit");
}

// ----- edits through a loop end (RS 5.4) ----------------------------------------

/// A datum set through a loop end stays on that node (its own position) and
/// says which station it is shown as; a rename or a comment through it is
/// the kept end's, on every line that shows it.
#[test]
fn edits_through_a_loop_end_address_what_it_shows() {
    let (mut p, a, b) = loop_survey();
    let (a4, b3) = (at(&p, a, 4), at(&p, b, 3));
    close_b3_on_a4(&mut p, a, b);
    let set = ok(p.set_datum_at(StationRef::new(b, 3), origin(), None));
    assert_eq!(set.value.shown_as, Some(a4));
    assert_eq!(p.datums[set.value.index].station, b3);
    assert_eq!(p.datums[set.value.index].via, Some(b));
    assert!(p.datums.iter().all(|d| d.station != a4));
    assert_valid(&p);

    p.set_shown_name(b3, "Sump".to_string());
    p.set_shown_comment(b3, Some("deep".to_string()));
    assert_valid(&p);
    assert_eq!(p.stations[&a4].name, "Sump");
    assert_eq!(p.stations[&b3].name, "B3", "the loop end's own name stays");
    assert_eq!(shown(&p, a)[4], "Sump");
    assert_eq!(shown(&p, b)[3], "Sump");
    assert_eq!(p.shown_comment(b3), Some("deep"));
    assert_eq!(p.stations[&b3].comment, None);
    p.set_shown_comment(b3, Some(String::new()));
    assert_eq!(p.stations[&a4].comment, None, "an empty comment is none");
}

/// RS 5.4: a same-line closure keeping one end shows that station at both
/// slots of the line (N8 allows one identity twice); the loop end is not
/// foreign there (the line is native at the kept end), and Remove Closure
/// gives it back.
#[test]
fn a_same_line_closure_with_a_keep_shows_one_station_twice() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 4, 10.0));
    name_line(&mut p, a, "A", false);
    let (a0, a4) = (at(&p, a, 0), at(&p, a, 4));
    let id = ok(p.create_closure(a4, Some(a), a0, Some(a), BATCH, Some(a0), &[]))
        .value
        .closure
        .id;
    assert_valid(&p);
    assert_eq!(shown(&p, a), ["A0", "A1", "A2", "A3", "A0"]);
    let owner = p.slot_owner(StationRef::new(a, 4)).unwrap();
    assert!(!owner.foreign);
    assert_eq!(owner.loop_closure, Some(id));
    assert!(p.name_collisions().is_empty());
    ok(p.remove_closure(id, &[]));
    assert_eq!(shown(&p, a), ["A0", "A1", "A2", "A3", "A4"]);
}

// ----- properties and crafted files -------------------------------------------

/// RS 12: on a small survey, every closure that can be created between two
/// of its stations, with every allowed keep and with "Keep both names",
/// is undone exactly, and when it letters nothing Remove Closure restores
/// every shown identity and record.
#[test]
fn create_then_remove_restores_every_shown_identity() {
    let (p, lines) = loop_with_tail();
    let stations: Vec<StationId> = p.stations.keys().copied().collect();
    let names = |q: &Project| -> Vec<Vec<String>> { lines.iter().map(|&l| shown(q, l)).collect() };
    let mut created = 0;
    for (i, &x) in stations.iter().enumerate() {
        for &y in &stations[i + 1..] {
            for keep in [None, Some(x), Some(y)] {
                let mut q = p.clone();
                let Ok(out) = q.create_closure_auto(x, None, y, None, BATCH, keep) else {
                    continue;
                };
                assert_valid(&q);
                undo_is_exact(&p, &q);
                created += 1;
                if !out.identities.lettered.is_empty() {
                    continue;
                }
                ok(q.remove_closure(out.value.closure.id, &[]));
                assert_eq!(q.stations, p.stations, "{x} {y} keep {keep:?}");
                assert_eq!(names(&q), names(&p), "{x} {y} keep {keep:?}");
            }
        }
    }
    assert!(created > 40, "{created}");
}

/// C1 and C2 broken in a crafted file: the dialogs' reads (the keep choices,
/// the slot owner, the default merge keep, rule K, the create and remove
/// previews) return instead of hanging on the cycle.
#[test]
fn crafted_archive_cycles_do_not_hang_the_dialogs() {
    let (mut p, a, b) = loop_survey();
    let (a2, a4, b2, b3) = (at(&p, a, 2), at(&p, a, 4), at(&p, b, 2), at(&p, b, 3));
    p.push_closure(b3, None, a4, None, BATCH);
    p.push_closure(a4, None, b3, None, BATCH);
    p.closures[0].archived = Some(ClosureEnd::A); // b3 -> a4
    p.closures[1].archived = Some(ClosureEnd::A); // a4 -> b3: a cycle
    assert!(!p.validate().is_empty());
    let id = p.closures[0].id;
    let _ = p.closure_keep_choices(b3, a2);
    let _ = p.closure_keep_choices(a4, b2);
    let _ = p.slot_owner(StationRef::new(b, 3));
    let _ = p.lines_shown_on(a4);
    let _ = p.nodes_showing(b3);
    let _ = p.rule_k_nodes(&[a4, b3, a2]);
    let _ = p.merge_default_keep(a4, b3);
    // The previews run the op on a copy, whose epilogue normalises.
    let _ = p.closure_naming((a2, None), (b2, None), BATCH, Some(a2));
    let _ = p.closure_removal(id);
    let mut fixed = p.clone();
    assert_eq!(fixed.normalise_closure_archives(), vec![p.closures[1].id]);
    assert_valid(&fixed);
}
