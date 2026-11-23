//! Kept and archived stations on the wire (plan 2026-11-23, RS 6 and 7):
//! the shown slot of a loop end and of a foreign slot, a closure's archived
//! end, the Merge dialog's two keeps with each collision's two answers, the
//! Detach, Create Loop Closure and Remove Closure previews, whose station a
//! slot shows, and the identity notice. Fully synthetic data.

use crate::{ClosureEndWire, ShownSlot, StationIdentity, StationRef};

use super::super::archive_tests::shown;
use super::super::ops_test_helpers::{assert_valid, branch, free_line, ok, run};
use super::super::test_helpers::{at, flat, project};
use super::super::{ClosureStrategy, LineId, Project, StationId};

const BATCH: ClosureStrategy = ClosureStrategy::Batch;

fn slot(line: LineId, index: u32) -> StationRef {
    StationRef::new(line, index)
}

fn identity(id: StationId, name: &str, home_line: Option<LineId>) -> StationIdentity {
    StationIdentity {
        id,
        name: name.to_string(),
        comment: None,
        home_line,
    }
}

fn wire_at(p: &Project, at: StationRef) -> crate::Station {
    p.stations_wire(&p.raw_positions())
        .into_iter()
        .find(|s| s.station_ref == at)
        .unwrap()
}

/// Name every station of `line` `{prefix}{index}`, the first one too unless
/// `skip_first` (a branch's start is the junction).
fn name_line(p: &mut Project, line: LineId, prefix: &str, skip_first: bool) {
    let chain = p.line_stations(line).unwrap().to_vec();
    for (i, s) in chain.into_iter().enumerate() {
        if !(skip_first && i == 0) {
            p.station_mut(s).unwrap().name = format!("{prefix}{i}");
        }
    }
}

/// A loop: A runs east 4 x 10 m (A0..A4); B leaves A0 north 10 m, east 40 m
/// and south 10 m (B1..B3), ending on A4's spot unmerged. A measures 40 m,
/// B 60 m.
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

/// Two free lines A (0, 1, 2; 20 m) and B (0, 1, 2; 10 m), each named by
/// index, apart.
fn two_lines() -> (Project, LineId, LineId) {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let b = free_line(&mut p, [100.0, 50.0], run(0.0, 2, 5.0));
    assert_valid(&p);
    (p, a, b)
}

/// RS 6: a loop end's slot shows the kept end's name and Station ID with
/// the loop mark and, off the kept end's native lines, the home-line dot;
/// its node id stays its own. The kept end's own slot carries nothing. The
/// closure says which end it archives, whose identity that end hides and
/// what it is shown as. Injection: leave `shown` off loop ends and the
/// archived slot reads as a native A4.
#[test]
fn a_loop_end_shows_the_kept_identity_with_its_mark() {
    let (mut p, a, b) = loop_survey();
    let (a4, b3) = (at(&p, a, 4), at(&p, b, 3));
    let id = ok(p.create_closure(b3, Some(b), a4, Some(a), BATCH, Some(a4), &[]))
        .value
        .closure
        .id;
    let end = wire_at(&p, slot(b, 3));
    assert_eq!((end.id, end.name.as_str()), (b3, "A4"));
    assert_eq!(
        end.shown,
        Some(ShownSlot {
            id: a4,
            home_line: Some(a),
            foreign: true,
            loop_end: true,
            loop_closure: Some(id),
        })
    );
    assert_eq!(end.shown_id(), a4);
    assert!(end.is_foreign() && end.is_loop_end());
    assert_eq!(wire_at(&p, slot(a, 4)).shown, None);
    assert_eq!(wire_at(&p, slot(b, 2)).shown, None);

    let raw = p.raw_positions();
    let aligned = p.closure_alignment(&raw);
    let closures = p.loop_closures_wire(&raw, &aligned);
    let archive = closures[0].archived.as_ref().unwrap();
    assert_eq!(archive.end, ClosureEndWire::A);
    assert_eq!(archive.station, identity(b3, "B3", Some(b)));
    assert_eq!(archive.shown_as, identity(a4, "A4", Some(a)));
    let json = serde_json::to_value(&closures[0]).unwrap();
    assert_eq!(json["archived"]["end"], serde_json::json!("a"));
}

/// RS 7: the Merge dialog gets both stations, the default keep, and for
/// each keep the lines that will show the kept name, the station hidden,
/// and every collision with rule L's answer and the other one (rename the
/// kept station). Applying either answer as the command's renames merges;
/// no answer refuses. Injection: drop the alternative and the dialog can
/// offer only one answer.
#[test]
fn the_merge_preview_offers_both_keeps_and_both_answers() {
    let (p, a, b) = two_lines();
    let (a0, a2, b0, b2) = (at(&p, a, 0), at(&p, a, 2), at(&p, b, 0), at(&p, b, 2));
    let result = p
        .check_merge_connectivity_wire(slot(a, 2), slot(b, 0))
        .unwrap();
    let preview = result.merge_preview.unwrap();
    assert_eq!(preview.candidates.len(), 2);
    assert_eq!(preview.candidates[0].station, slot(a, 2));
    assert_eq!(preview.candidates[0].identity, identity(a2, "2", Some(a)));
    assert_eq!(preview.candidates[1].identity, identity(b0, "0", Some(b)));
    assert_eq!(preview.default_keep, a2, "rule K: A is the longer line");
    assert_eq!(preview.keep_forced_by, None);
    let keeps: Vec<StationId> = preview.keeps.iter().map(|k| k.keep).collect();
    assert_eq!(keeps, [a2, b0]);

    // Keeping B's 0: A's own 0 repeats on A.
    let keep_b = &preview.keeps[1];
    assert_eq!(keep_b.hidden, identity(a2, "2", Some(a)));
    assert_eq!(keep_b.renamed_lines, [a]);
    assert_eq!(keep_b.collisions.len(), 1);
    let collision = &keep_b.collisions[0];
    assert_eq!((collision.line_id, collision.name.as_str()), (a, "0"));
    assert_eq!(collision.letter.station.id, a0, "rule L letters A's 0");
    assert_eq!(collision.letter.to, "0a");
    assert_eq!(collision.alternative.station.id, b0, "or the kept station");
    assert_eq!(collision.alternative.to, "0a");

    // Keeping A's 2: B's own 2 repeats on B.
    let keep_a = &preview.keeps[0];
    assert_eq!(keep_a.hidden, identity(b0, "0", Some(b)));
    assert_eq!(keep_a.collisions[0].letter.station.id, b2);
    assert_eq!(keep_a.collisions[0].alternative.station.id, a2);

    for answer in [&collision.letter, &collision.alternative] {
        let mut q = p.clone();
        let renames = [(answer.station.id, answer.to.clone())];
        ok(q.merge_stations(a2, b0, b0, &renames));
        assert_valid(&q);
        assert_eq!(q.shown_name(answer.station.id), Some("0a"));
    }
    assert!(p.clone().merge_stations(a2, b0, b0, &[]).is_err());
}

/// RS 4.2, 7: after a merge the Detach preview names what comes back with
/// its comment; a name the user reused on its line since is a collision
/// with both answers. Injection: drop the comment and the dialog cannot
/// say "with its comment".
#[test]
fn the_detach_preview_names_what_comes_back_and_its_collisions() {
    let (mut p, a, b) = two_lines();
    let (a1, a2, b0) = (at(&p, a, 1), at(&p, a, 2), at(&p, b, 0));
    p.station_mut(a2).unwrap().comment = Some("rope end".to_string());
    ok(p.merge_stations_auto(a2, b0, b0));
    // A's own 2 is archived: its name is free on A.
    p.set_shown_name(a1, "2".to_string());
    assert_valid(&p);
    let preview = p.detach_preview_wire(slot(a, 2), a, None).unwrap();
    assert!(preview.restored);
    assert_eq!((preview.station_id, preview.name.as_str()), (a2, "2a"));
    assert_eq!(preview.comment.as_deref(), Some("rope end"));
    assert!(preview.together.is_empty() && preview.rest_choices.is_empty());
    assert_eq!(preview.collisions.len(), 1);
    let collision = &preview.collisions[0];
    assert_eq!((collision.line_id, collision.name.as_str()), (a, "2"));
    let pair = [
        collision.letter.station.id,
        collision.alternative.station.id,
    ];
    assert!(pair.contains(&a1) && pair.contains(&a2), "{collision:?}");
    let mut q = p.clone();
    let renames = [(
        collision.alternative.station.id,
        collision.alternative.to.clone(),
    )];
    ok(q.detach_line(b0, a, &renames, None));
    assert_valid(&q);
}

/// RS 5.2, 7: the Create Loop Closure preview lists both ends, the allowed
/// keeps, rule K's end as the default and what each choice does, "Keep both
/// names" last (no name changes).
#[test]
fn the_closure_preview_lists_every_choice() {
    let (p, a, b) = loop_survey();
    let (a4, b3) = (at(&p, a, 4), at(&p, b, 3));
    let naming = p
        .loop_closure_naming_wire(slot(b, 3), slot(a, 4), BATCH)
        .unwrap();
    assert_eq!(naming.ends[0].identity, identity(b3, "B3", Some(b)));
    assert_eq!(naming.ends[1].identity, identity(a4, "A4", Some(a)));
    assert_eq!(naming.allowed, [b3, a4]);
    assert_eq!(
        (naming.rule_k, naming.default_keep),
        (Some(b3), Some(b3)),
        "B is the longer line"
    );
    let keeps: Vec<Option<StationId>> = naming.options.iter().map(|o| o.keep).collect();
    assert_eq!(keeps, [Some(b3), Some(a4), None]);
    assert_eq!(naming.options[0].hidden, Some(identity(a4, "A4", Some(a))));
    assert_eq!(naming.options[0].renamed_lines, [a]);
    assert_eq!(naming.options[1].hidden, Some(identity(b3, "B3", Some(b))));
    assert_eq!(naming.options[1].renamed_lines, [b]);
    assert_eq!(naming.options[2].hidden, None);
    assert!(naming.options[2].renamed_lines.is_empty());
}

/// RS 5.3, 7: Remove Closure says what comes back on which slot and the
/// name it repeats when the user reused it, with both answers; the
/// command's notice without a dialog names the letter rule L gave.
/// Whose station a loop end shows: the kept end's, on every line it is
/// shown on, through the closure.
#[test]
fn remove_closure_names_what_comes_back_and_the_owner_of_a_loop_end() {
    let (mut p, a, b) = loop_survey();
    let (a4, b1, b3) = (at(&p, a, 4), at(&p, b, 1), at(&p, b, 3));
    let id = ok(p.create_closure(b3, Some(b), a4, Some(a), BATCH, Some(a4), &[]))
        .value
        .closure
        .id;
    let owner = p.station_owner_wire(slot(b, 3)).unwrap();
    assert_eq!(owner.node, b3);
    assert_eq!(owner.shown, identity(a4, "A4", Some(a)));
    assert_eq!(owner.lines, [a, b]);
    assert!(owner.foreign);
    assert_eq!(owner.loop_closure, Some(id));
    assert_eq!(owner.own_station, None);

    // B3's name is free on B while the closure hides it.
    p.set_shown_name(b1, "B3".to_string());
    assert_valid(&p);
    let naming = p.remove_closure_naming_wire(id).unwrap();
    assert_eq!(naming.archived_slot, Some(slot(b, 3)));
    assert_eq!(naming.comes_back, Some(identity(b3, "B3", Some(b))));
    assert_eq!(naming.shown_as, Some(identity(a4, "A4", Some(a))));
    assert_eq!(naming.renamed_lines, [b]);
    assert_eq!(naming.collisions.len(), 1);
    assert_eq!(naming.collisions[0].name, "B3");

    let mut q = p.clone();
    let out = ok(q.remove_closure_auto(id));
    let notice = q.identity_notice(&out.identities);
    assert_eq!(notice.lettered.len(), 1, "{notice:?}");
    assert_eq!(notice.lettered[0].from, "B3");
    assert!(notice.restored.is_empty() && notice.unarchived.is_empty());
    assert_eq!(shown(&q, b).len(), 4);
}

/// RS 5.4: the slot of a line whose own station a merge archived is foreign:
/// a rename there renames the kept station on every line, and the line's
/// own station is named for "Detach to get it back".
#[test]
fn a_foreign_slot_names_its_owner_and_its_own_station() {
    let (mut p, a, b) = two_lines();
    let (a2, b0) = (at(&p, a, 2), at(&p, b, 0));
    ok(p.merge_stations_auto(a2, b0, b0));
    let owner = p.station_owner_wire(slot(a, 2)).unwrap();
    assert_eq!(owner.node, b0);
    assert_eq!(owner.shown, identity(b0, "0", Some(b)));
    assert_eq!(owner.lines, [a, b]);
    assert!(owner.foreign);
    assert_eq!(owner.loop_closure, None);
    assert_eq!(owner.own_station, Some(identity(a2, "2", Some(a))));
    let native = p.station_owner_wire(slot(b, 0)).unwrap();
    assert!(!native.foreign && native.own_station.is_none());
    assert_eq!(p.records_wire(b0).len(), 1);
}
