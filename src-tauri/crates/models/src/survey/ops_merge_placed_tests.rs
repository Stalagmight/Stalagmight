//! The keep of a merge without a dialog where a station is placed onto
//! another (orphan placement, RS 4.1, U-P2-9).

use crate::StationRef;

use super::ops_test_helpers::{assert_valid, free_line, gps_at, ok, origin, run, xy_all};
use super::test_helpers::{at, project};
use super::{LineId, Project};

/// An anchored piece `Survey` (datum on its station 0, 3 x 10 m) and an
/// orphan piece `Orphan` (free, `orphan_shots` x 2 m), apart.
fn anchored_and_orphan(orphan_shots: usize) -> (Project, LineId, LineId) {
    let mut p = project();
    let survey = free_line(&mut p, [0.0, 0.0], run(0.0, 3, 10.0));
    let orphan = free_line(&mut p, [200.0, 0.0], run(90.0, orphan_shots, 2.0));
    ok(p.set_datum_at(StationRef::new(survey, 0), origin(), None));
    (p, survey, orphan)
}

/// The orphan's station is placed onto a station of the anchored survey:
/// that station (the placed one) is kept, even when the orphan's line is
/// the longer one, and the orphan's station is archived on the orphan line.
#[test]
fn an_orphan_placed_onto_the_anchored_survey_keeps_the_target() {
    let (mut p, survey, orphan) = anchored_and_orphan(40);
    let (other, target) = (at(&p, orphan, 0), at(&p, survey, 2));
    assert_eq!(p.placement_merge_keep(other, target), target);
    let out = ok(p.merge_by_placement(other, target));
    assert_eq!((out.value.kept, out.value.archived), (target, other));
    assert_eq!(out.value.moving, other, "the orphan moves");
    let record = p.record_naming(target, orphan).unwrap();
    assert_eq!(record.id, other);
    assert_valid(&p);
}

/// The other station is the placed one (the target's piece is free): the
/// placed station is kept.
#[test]
fn the_placed_station_is_kept_when_only_the_other_is_placed() {
    let (mut p, survey, orphan) = anchored_and_orphan(2);
    let (other, target) = (at(&p, survey, 1), at(&p, orphan, 0));
    assert_eq!(p.placement_merge_keep(other, target), other);
    let out = ok(p.merge_by_placement(other, target));
    assert_eq!(out.value.kept, other);
    assert_valid(&p);
}

/// Both pieces placed: the target is kept.
#[test]
fn both_placed_keeps_the_target() {
    let (mut p, survey, orphan) = anchored_and_orphan(2);
    ok(p.set_datum_at(StationRef::new(orphan, 0), gps_at(200.0, 0.0), None));
    let (other, target) = (at(&p, survey, 1), at(&p, orphan, 1));
    assert_eq!(p.placement_merge_keep(other, target), target);
    assert_eq!(p.placement_merge_keep(target, other), other);
}

/// Neither piece placed: rule K keeps the station on the longer line,
/// whichever is the target, and the raw layout is the same either way.
#[test]
fn neither_placed_keeps_rule_k_and_moves_the_same() {
    let build = || {
        let mut p = project();
        let long = free_line(&mut p, [0.0, 0.0], run(0.0, 4, 10.0));
        let short = free_line(&mut p, [200.0, 0.0], run(90.0, 1, 2.0));
        (p, long, short)
    };
    let (p, long, short) = build();
    let (on_long, on_short) = (at(&p, long, 2), at(&p, short, 0));
    assert_eq!(p.placement_merge_keep(on_short, on_long), on_long);
    assert_eq!(p.placement_merge_keep(on_long, on_short), on_long);

    // The short line's station is the target: it is the stationary one,
    // but the long line's is kept (re-identified in the same op).
    let (mut kept_long, _, _) = build();
    let out = ok(kept_long.merge_by_placement(on_long, on_short));
    assert_eq!(out.value.kept, on_long);
    assert_eq!(out.value.stationary, on_short);
    assert_valid(&kept_long);
    let (mut kept_short, _, _) = build();
    ok(kept_short.merge_stations_auto(on_long, on_short, on_short));
    // Slot by slot (the merged node's id is the kept station's).
    let positions = |p: &Project| -> Vec<[f64; 2]> {
        let xy = xy_all(p);
        p.lines()
            .iter()
            .flat_map(|l| l.stations().map(|s| xy[&s]).collect::<Vec<_>>())
            .collect()
    };
    assert_eq!(
        positions(&kept_long),
        positions(&kept_short),
        "the raw layout is the same for either keep"
    );
}
