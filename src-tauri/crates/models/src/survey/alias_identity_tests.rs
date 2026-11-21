//! Per-line names and the alias identity (the real-app demo's I-1 and I-3):
//! a line shows a junction by the name that line knows it by, and a delete
//! or a detach that leaves a station only on lines knowing it by an alias
//! gives it back that alias's identity, with what hangs on it following and
//! Undo exact.

use crate::StationRef;

use super::changeset::testkit::{persisted, same_state};
use super::changeset::{capture, redo, undo};
use super::ops_test_helpers::{assert_valid, free_line, ok, origin, run, xy_all};
use super::test_helpers::{at, project};
use super::{Project, RangeDeleteMode, StationAlias};

const TOL: f64 = 1e-9;

/// Undo then redo the change from `before` to `after`, bit for bit, in the
/// session and through the persisted entry.
fn undo_is_exact(before: &Project, after: &Project) {
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
fn shown(p: &Project, line: crate::LineId) -> Vec<String> {
    p.line_stations(line)
        .unwrap()
        .iter()
        .map(|&s| p.name_on(s, line).unwrap().to_string())
        .collect()
}

/// The demo's W2/W4 survey: Far Room (0, 1, 2, 3) and the Upstream piece
/// (0, 1, 2, 3) merged with Upstream's start moving onto Far Room's end.
/// Far Room's end carries a comment. Returns the project and the two lines.
fn far_room_and_upstream() -> (Project, crate::LineId, crate::LineId) {
    let mut p = project();
    let far = free_line(&mut p, [0.0, 0.0], run(90.0, 3, 10.0));
    let up = free_line(&mut p, [60.0, 40.0], run(0.0, 3, 5.0));
    let (f3, u0) = (at(&p, far, 3), at(&p, up, 0));
    p.station_mut(f3).unwrap().comment = Some("Far Room: large chamber".to_string());
    ok(p.merge_stations(u0, f3));
    assert_valid(&p);
    (p, far, up)
}

/// Every station of `line` sits where `before` had its slot.
fn line_still(before: &Project, after: &Project, line: crate::LineId) {
    let (was, now) = (xy_all(before), xy_all(after));
    for (index, &station) in after.line_stations(line).unwrap().iter().enumerate() {
        let old = at(before, line, index);
        let (a, b) = (was[&old], now[&station]);
        assert!(
            (a[0] - b[0]).abs() < TOL && (a[1] - b[1]).abs() < TOL,
            "station {index} of line {line} moved from {a:?} to {b:?}"
        );
    }
}

/// I-3: after the merge each line shows the junction by its own name, the
/// Station ID is the junction's one id, and no line shows a name twice
/// (Upstream reads 0, 1, 2, 3, not 2, 1, 2, 3). Injection: show the
/// station's own name on every line (`name_on` without the alias) and
/// Upstream reads 2, 1, 2, 3, which `validate` refuses.
#[test]
fn each_line_shows_a_junction_by_its_own_name() {
    let (p, far, up) = far_room_and_upstream();
    let f3 = at(&p, far, 3);
    assert_eq!(at(&p, up, 0), f3, "one station, one Station ID");
    assert_eq!(shown(&p, far), ["0", "1", "2", "3"]);
    assert_eq!(shown(&p, up), ["0", "1", "2", "3"]);
    assert_eq!(p.slot_name(StationRef::new(up, 0)), Some("0"));
    assert_eq!(p.slot_name(StationRef::new(far, 3)), Some("3"));
    assert!(p.station_text(f3, Some(up)).ends_with(" / 0"));
    assert!(p.station_text(f3, Some(far)).ends_with(" / 3"));
}

/// I-1: deleting Far Room's last shot leaves its end station on Upstream
/// alone, which knew it by its alias, so the station goes back to Upstream's
/// own identity (its Station ID and name), as Detach gives it back. Its
/// comment stays (it may hold both lines' notes), nothing moves, and Undo is
/// exact. Injection: skip `restore_alias_identities` in the edit epilogue
/// and Upstream's start keeps Far Room's Station ID.
#[test]
fn deleting_a_lines_last_shot_at_a_junction_gives_the_other_line_its_station_back() {
    let (mut p, far, up) = far_room_and_upstream();
    let f3 = at(&p, far, 3);
    let u0 = p.station(f3).unwrap().aliases[0].id;
    let before = p.clone();
    let last = p.line(far).unwrap().shots[2].id;
    let edited = p.delete_shot(far, last).unwrap();
    assert_valid(&p);
    assert_eq!(edited.report.reidentified, vec![(f3, u0)]);
    assert_eq!(at(&p, up, 0), u0, "Upstream's start has its own id again");
    assert!(p.station(f3).is_none());
    let back = p.station(u0).unwrap();
    assert_eq!(back.name, "0");
    assert!(
        back.aliases.is_empty(),
        "no alias left once it is no junction"
    );
    assert_eq!(back.comment.as_deref(), Some("Far Room: large chamber"));
    assert_eq!(shown(&p, up), ["0", "1", "2", "3"]);
    assert_eq!(shown(&p, far), ["0", "1", "2"]);
    line_still(&before, &p, up);
    line_still(&before, &p, far);
    undo_is_exact(&before, &p);
}

/// The same for the other deletes that take Far Room off the junction:
/// Delete Line and a range delete of its last shots.
#[test]
fn deleting_the_line_or_a_range_gives_the_station_back_too() {
    let (base, far, up) = far_room_and_upstream();
    let u0 = base.station(at(&base, far, 3)).unwrap().aliases[0].id;

    let mut p = base.clone();
    p.delete_line(far).unwrap();
    assert_valid(&p);
    assert_eq!(at(&p, up, 0), u0);
    assert_eq!(shown(&p, up), ["0", "1", "2", "3"]);
    line_still(&base, &p, up);
    undo_is_exact(&base, &p);

    let mut p = base.clone();
    let shots = &p.line(far).unwrap().shots;
    let (first, last) = (shots[1].id, shots[2].id);
    p.delete_shot_range(far, first, last, &RangeDeleteMode::Shift)
        .unwrap();
    assert_valid(&p);
    assert_eq!(at(&p, up, 0), u0);
    assert_eq!(shown(&p, up), ["0", "1", "2", "3"]);
    assert_eq!(shown(&p, far), ["0", "1"]);
    line_still(&base, &p, up);
    undo_is_exact(&base, &p);
}

/// A station a delete leaves on two lines that both knew it by aliases takes
/// the identity of the first of those aliases (the oldest merge); the other
/// line keeps its alias, so both keep the names they showed; the datum on
/// the station follows the new id.
#[test]
fn a_station_left_on_two_alias_lines_takes_the_oldest_aliases_identity() {
    let mut p = project();
    let main = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let j = at(&p, main, 1);
    ok(p.set_datum_at(StationRef::new(main, 1), origin(), None));
    let a = free_line(&mut p, [50.0, 50.0], run(0.0, 2, 5.0));
    let b = free_line(&mut p, [80.0, -50.0], run(180.0, 2, 5.0));
    let (a0, b0) = (at(&p, a, 0), at(&p, b, 0));
    ok(p.merge_stations(a0, j));
    ok(p.merge_stations(b0, j));
    assert_eq!(shown(&p, a), ["0", "1", "2"]);
    assert_eq!(shown(&p, b), ["0", "1", "2"]);
    assert_eq!(shown(&p, main), ["0", "1", "2"]);
    let before = p.clone();

    let edited = p.delete_line(main).unwrap();
    assert_valid(&p);
    assert_eq!(edited.report.reidentified, vec![(j, a0)]);
    assert_eq!((at(&p, a, 0), at(&p, b, 0)), (a0, a0), "still one junction");
    assert_eq!(
        p.station(a0).unwrap().aliases,
        vec![StationAlias {
            id: b0,
            name: Some("0".to_string()),
            line: Some(b),
        }]
    );
    assert_eq!(shown(&p, a), ["0", "1", "2"]);
    assert_eq!(shown(&p, b), ["0", "1", "2"]);
    assert_eq!(p.datums.len(), 1);
    assert_eq!(p.datums[0].station, a0, "the datum follows its station");
    line_still(&before, &p, a);
    line_still(&before, &p, b);
    undo_is_exact(&before, &p);
}

/// Detach of the line that showed the junction by its own name, when the
/// only other line knew it by an alias: the line keeps the junction and the
/// other line gets back its own station (the I-1 rule, from the Detach
/// side); Undo is exact.
#[test]
fn detaching_the_owning_line_gives_the_other_line_its_station_back() {
    let (mut p, far, up) = far_room_and_upstream();
    let f3 = at(&p, far, 3);
    let u0 = p.station(f3).unwrap().aliases[0].id;
    let before = p.clone();
    let out = ok(p.detach_line(f3, far));
    assert_valid(&p);
    assert!(out.value.kept);
    assert_eq!((at(&p, far, 3), at(&p, up, 0)), (f3, u0));
    assert_eq!(
        p.station(f3).unwrap().comment.as_deref(),
        Some("Far Room: large chamber"),
        "the line that keeps the junction keeps its comment"
    );
    assert_eq!(p.station(u0).unwrap().name, "0");
    assert_eq!(shown(&p, up), ["0", "1", "2", "3"]);
    undo_is_exact(&before, &p);
}
