//! The adversarial review's (R1) scenarios, run through the M2 commands:
//! detach with a closure on the detached member (F4), a placement datum and
//! a GPS fix merged then detached (F4), a hub detach (F4, declared change),
//! frame removal with three entrance pieces on a curved Earth (F2), and Add
//! with a free piece and a distant frame (F2, in `append_tests`).

use crate::{gps_to_local, DatumRole, StationRef};

use super::m3_stubs::datum_roles;
use super::ops_test_helpers::{
    assert_shifted, assert_valid, branch, free_line, gps_at, gps_from, gps_local, near, ok, origin,
    run, xy, xy_all,
};
use super::test_helpers::{at, project};
use super::ClosureStrategy;

#[test]
fn detaching_a_line_whose_member_held_a_closure_promotes_it_and_the_line_follows() {
    // L1 east (2 x 10 m); L3 north from L1's end, X = its end at (20, 20);
    // L2 north from L1:1, the junction S at (10, 0). Closure c joins S (via
    // L2) to X. Today the closure hangs off L2's member of the merge: an
    // unmerge leaves L2 attached by c alone, the sweep promotes c, and L2
    // moves onto X by the misclosure and is merged there.
    let mut p = project();
    let l1 = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let (s, l1_end) = (at(&p, l1, 1), at(&p, l1, 2));
    let l3 = branch(&mut p, l1_end, run(0.0, 2, 10.0));
    let l2 = branch(&mut p, s, run(0.0, 2, 10.0));
    let x = at(&p, l3, 2);
    let c = ok(p.create_closure(s, Some(l2), x, Some(l3), ClosureStrategy::Batch))
        .value
        .closure;
    let before = xy_all(&p);
    let l2_rest: Vec<_> = p.line_stations(l2).unwrap()[1..].to_vec();

    let out = ok(p.unmerge_station_pair(StationRef::new(l1, 1), StationRef::new(l2, 0)));
    assert_valid(&p);
    let fresh = out.value.new_station;
    assert_eq!(out.value.moved_closure_ends.len(), 1);
    assert_eq!(out.sweep.promoted.len(), 1);
    let promotion = &out.sweep.promoted[0];
    assert_eq!(promotion.closure.id, c.id);
    assert_eq!((promotion.survivor, promotion.retired), (x, fresh));
    assert!(p.closures.is_empty());
    assert_eq!(p.line(l2).unwrap().start, x, "L2 now starts on X");
    assert_shifted(&before, &xy_all(&p), 1e-9, |station| {
        if l2_rest.contains(&station) {
            [10.0, 20.0]
        } else {
            [0.0, 0.0]
        }
    });
}

#[test]
fn a_placement_datum_and_a_fix_merged_then_detached_put_the_fixs_piece_on_its_gps() {
    // The #741 state: A placed by its datum (the frame); B joined at A's end,
    // the junction holds a GPS fix set from B's slot, 20 m off the survey.
    // Detaching B promotes the fix, and B's piece jumps onto its GPS.
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(0.0, 2, 50.0));
    ok(p.set_datum_at(StationRef::new(a, 0), origin(), None));
    let junction = at(&p, a, 2);
    let b = branch(&mut p, junction, run(0.0, 2, 30.0));
    let fix_gps = gps_at(20.0, 100.0);
    let set = ok(p.set_datum_at(StationRef::new(b, 0), fix_gps, None)).value;
    assert_eq!(set.role, Some(DatumRole::GpsFix));
    assert_eq!(p.datums[set.index].via, Some(b));
    let before = xy_all(&p);
    let b_rest: Vec<_> = p.line_stations(b).unwrap()[1..].to_vec();

    let out = ok(p.detach_line(junction, b));
    assert_valid(&p);
    assert_eq!(out.value.moved_datums, vec![set.index]);
    assert_eq!(
        datum_roles(&p),
        vec![Some(DatumRole::Placement), Some(DatumRole::Placement)]
    );
    near(
        xy(&p, out.value.new_station),
        gps_local(&p, &fix_gps),
        1e-9,
        "on its GPS",
    );
    assert_shifted(&before, &xy_all(&p), 1e-9, |s| {
        if b_rest.contains(&s) {
            [20.0, 0.0]
        } else {
            [0.0, 0.0]
        }
    });
}

#[test]
fn removing_the_frame_keeps_every_piece_where_it_was_despite_the_tangent_planes() {
    // Frame D1 at the origin (lat 20.5), D2 2 km east, D3 2 km north of D2,
    // three separate pieces. Removing D1 makes D2 the frame. Re-deriving D3
    // from GPS against D2 would move it by about 0.2 m (the tangent planes
    // at D1 and D2 are not parallel); measuring keeps it in place.
    let mut p = project();
    let d1 = free_line(&mut p, [0.0, 0.0], run(0.0, 1, 10.0));
    let d2 = free_line(&mut p, [1.0, 0.0], run(0.0, 1, 10.0));
    let d3 = free_line(&mut p, [2.0, 0.0], run(0.0, 1, 10.0));
    let g2 = gps_at(2000.0, 0.0);
    let g3 = gps_from(&g2, 0.0, 2000.0);
    ok(p.set_datum_at(StationRef::new(d1, 0), origin(), None));
    ok(p.set_datum_at(StationRef::new(d2, 0), g2, None));
    ok(p.set_datum_at(StationRef::new(d3, 0), g3, None));
    let (s2, s3) = (at(&p, d2, 0), at(&p, d3, 0));
    let before = xy_all(&p);
    let relative = [
        before[&s3][0] - before[&s2][0],
        before[&s3][1] - before[&s2][1],
    ];
    let (rx, ry) = gps_to_local(&g3, &g2);
    let rederived_error = (rx - relative[0]).hypot(ry - relative[1]);
    assert!(
        rederived_error > 0.05,
        "the scenario must exercise non-additivity ({rederived_error} m)"
    );

    let out = ok(p.remove_datum(0));
    assert_valid(&p);
    assert!(out.anchors.frame_changed);
    assert_eq!(
        xy(&p, s2),
        [0.0, 0.0],
        "the new frame station is the origin"
    );
    let shift = [-before[&s2][0], -before[&s2][1]];
    assert_shifted(&before, &xy_all(&p), 1e-6, |_| shift);
    let correction = p.datums[1].correction.expect("D3 is held by a correction");
    let magnitude = correction[0].hypot(correction[1]);
    assert!((magnitude - rederived_error).abs() < 1e-6, "{magnitude} m");
    assert_eq!(p.datums[0].correction, None, "the frame carries none");
}
