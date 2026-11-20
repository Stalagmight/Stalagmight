//! The post-edit anchor algorithm (plan 4.1) and the frame change (4.3),
//! driven by hand-made edits of the kinds M1/M2 implement.

use crate::{gps_to_local, GpsCoordinates, LineId, StationId};

use super::test_helpers::{
    add, assert_xy_near, at, flat, gps, project, shot, three_line_junction, xy_of,
};
use super::{
    AnchorReport, AnchorSnapshot, EditContext, Placement, Project, RawPositions, StartAt, ZeroDatum,
};

const TOL: f64 = 1e-9;

/// Snapshot, run `edit`, settle with `ctx`; the result must validate.
fn edit_and_settle(
    p: &mut Project,
    ctx: &EditContext,
    edit: impl FnOnce(&mut Project),
) -> (AnchorSnapshot, AnchorReport) {
    let before = p.anchor_snapshot();
    edit(p);
    let report = p.settle_anchors(&before, ctx);
    assert_eq!(p.validate(), Vec::new(), "settled model must be valid");
    (before, report)
}

/// Every station of `after` sits at its `before` position plus `t`, except
/// `except`.
fn assert_moved_by(before: &RawPositions, after: &RawPositions, t: [f64; 2], except: &[StationId]) {
    for (station, [x, y, _]) in after.iter() {
        if except.contains(&station) {
            continue;
        }
        let Some([bx, by]) = before.xy(station) else {
            continue;
        };
        assert_xy_near(
            [x, y],
            [bx + t[0], by + t[1]],
            TOL,
            &format!("station {station}"),
        );
    }
}

/// Replace station `old` by `new` everywhere (what a merge or detach does to
/// references), and drop `old` when it is on no line any more.
fn replace_station(p: &mut Project, old: StationId, new: StationId, only_line: Option<LineId>) {
    for line in p.lines_mut() {
        if only_line.is_some_and(|l| l != line.id) {
            continue;
        }
        if line.start == old {
            line.start = new;
        }
        for shot in &mut line.shots {
            if shot.to == old {
                shot.to = new;
            }
        }
        if let Some(hand) = line.hand_moves.remove(&old) {
            line.hand_moves.insert(new, hand);
        }
    }
    if p.slots_of(old).is_empty() {
        p.stations.remove(&old);
    }
}

fn local(g: GpsCoordinates, frame: GpsCoordinates) -> [f64; 2] {
    let (x, y) = gps_to_local(&g, &frame);
    [x, y]
}

// ----- free pieces: which station stays ------------------------------------

#[test]
fn a_shot_edit_keeps_the_edited_lines_first_station_and_the_placement_station() {
    let (mut p, [_, l1, l2]) = three_line_junction();
    let (before, report) = edit_and_settle(&mut p, &EditContext::geometry(l1), |p| {
        p.line_mut(l1).unwrap().shots[1].length = 8.0;
    });
    assert_eq!(report, AnchorReport::default());
    let after = p.raw_positions();
    let moved = at(&p, l1, 2);
    assert_moved_by(before.positions(), &after, [0.0, 0.0], &[moved]);
    assert_eq!(p.placements.len(), 1);
    assert_eq!(p.placements[0].station, 0);
    assert_xy_near(
        [p.placements[0].x, p.placements[0].y],
        [100.0, 200.0],
        TOL,
        "gauge",
    );

    // L2 starts at its own station x0 and ends on L0's s3: lengthening it
    // keeps x0 and moves everything else 2 m west; the placement keeps its
    // station and records the new coordinates.
    let x0 = at(&p, l2, 0);
    let (before, _) = edit_and_settle(&mut p, &EditContext::geometry(l2), |p| {
        p.line_mut(l2).unwrap().shots[0].length = 9.0;
    });
    let after = p.raw_positions();
    assert_moved_by(before.positions(), &after, [-2.0, 0.0], &[x0]);
    assert_xy_near(
        xy_of(&after, x0),
        before.position(x0).unwrap(),
        TOL,
        "x0 stays",
    );
    assert_eq!(p.placements[0].station, 0);
    assert_xy_near(
        [p.placements[0].x, p.placements[0].y],
        [98.0, 200.0],
        TOL,
        "gauge",
    );
}

#[test]
fn a_shot_edit_in_a_datum_piece_keeps_the_datum_station() {
    let (mut p, [_, _, l2]) = three_line_junction();
    p.placements.clear();
    p.push_datum(ZeroDatum::new(0, gps(20.0, -87.0)));
    p.datums[0].correction = Some([4.0, 5.0]);
    let x0 = at(&p, l2, 0);
    let (before, report) = edit_and_settle(&mut p, &EditContext::geometry(l2), |p| {
        p.line_mut(l2).unwrap().shots[0].length = 9.0;
    });
    assert_eq!(report, AnchorReport::default());
    assert_eq!(p.datums[0].correction, Some([4.0, 5.0]));
    let after = p.raw_positions();
    assert_moved_by(before.positions(), &after, [0.0, 0.0], &[x0]);
    let [bx, by] = before.position(x0).unwrap();
    assert_xy_near(xy_of(&after, x0), [bx + 2.0, by], TOL, "x0 moves east");
}

#[test]
fn an_explicit_placement_move_is_kept() {
    let (mut p, _) = three_line_junction();
    let (before, _) = edit_and_settle(&mut p, &EditContext::none(), |p| {
        p.placements[0].x += 3.0;
        p.placements[0].y += 4.0;
    });
    assert_moved_by(before.positions(), &p.raw_positions(), [3.0, 4.0], &[]);
    assert_eq!(p.placements[0].station, 0);
}

#[test]
fn a_cut_off_piece_stays_where_it_was() {
    let mut p = project();
    let l0 = add(
        &mut p,
        StartAt::New,
        vec![flat(90.0, 10.0), flat(90.0, 10.0)],
    );
    let s2 = at(&p, l0, 2);
    add(
        &mut p,
        StartAt::Station(s2),
        vec![flat(0.0, 4.0), flat(0.0, 4.0)],
    );
    p.set_placement(at(&p, l0, 0), 7.0, 8.0).unwrap();
    // Delete L0's last shot: its end station is a junction, so it stays on L1.
    let (before, report) = edit_and_settle(&mut p, &EditContext::geometry(l0), |p| {
        p.line_mut(l0).unwrap().shots.pop();
    });
    assert_eq!(p.topology().components().len(), 2);
    assert_moved_by(before.positions(), &p.raw_positions(), [0.0, 0.0], &[]);
    assert_eq!(
        p.placements,
        vec![
            Placement {
                station: at(&p, l0, 0),
                x: 7.0,
                y: 8.0
            },
            Placement {
                station: s2,
                x: before.position(s2).unwrap()[0],
                y: before.position(s2).unwrap()[1],
            },
        ]
    );
    assert!(report.placements_dropped.is_empty());
}

#[test]
fn a_cut_off_piece_holding_a_fix_goes_onto_its_gps() {
    let mut p = project();
    let l0 = add(
        &mut p,
        StartAt::New,
        vec![flat(90.0, 10.0), flat(90.0, 10.0)],
    );
    let s2 = at(&p, l0, 2);
    let l1 = add(
        &mut p,
        StartAt::Station(s2),
        vec![flat(0.0, 4.0), flat(0.0, 4.0)],
    );
    let (g0, gf) = (gps(20.0, -87.0), gps(20.0003, -86.9997));
    let t2 = at(&p, l1, 2);
    p.push_datum(ZeroDatum::new(at(&p, l0, 0), g0));
    p.push_datum(ZeroDatum::new(t2, gf));
    let (before, report) = edit_and_settle(&mut p, &EditContext::geometry(l0), |p| {
        p.line_mut(l0).unwrap().shots.pop();
    });
    assert!(!report.frame_changed);
    assert_eq!(report.placed_on_gps, vec![1]);
    let after = p.raw_positions();
    assert_eq!(xy_of(&after, t2), local(gf, g0));
    // The datum piece did not move.
    assert_xy_near(
        xy_of(&after, at(&p, l0, 1)),
        before.position(at(&p, l0, 1)).unwrap(),
        TOL,
        "s1",
    );
    assert!(p.placements.is_empty());
}

// ----- merges and detaches -------------------------------------------------

#[test]
fn merging_two_free_pieces_keeps_the_stationary_placement_station() {
    let (mut p, [l0, _, _]) = three_line_junction();
    let q = add(&mut p, StartAt::New, vec![flat(0.0, 3.0), flat(0.0, 3.0)]);
    let (q0, q1) = (at(&p, q, 0), at(&p, q, 1));
    p.set_placement(q0, 500.0, 500.0).unwrap();
    let s3 = at(&p, l0, 3);
    // q1 (moving piece) merges onto s3 (stationary piece).
    let ctx = EditContext::merge(s3, q1).with_renamed(q1, s3);
    let (before, report) = edit_and_settle(&mut p, &ctx, |p| {
        replace_station(p, q1, s3, None);
    });
    assert_eq!(report.placements_dropped, vec![q0]);
    assert_eq!(p.placements.len(), 1);
    assert_eq!(p.placements[0].station, 0);
    let after = p.raw_positions();
    assert_moved_by(before.positions(), &after, [0.0, 0.0], &[q0, at(&p, q, 2)]);
    let [sx, sy] = xy_of(&after, s3);
    assert_xy_near(xy_of(&after, q0), [sx, sy - 3.0], TOL, "q0 hangs off s3");
}

#[test]
fn merging_two_placed_pieces_demotes_the_later_datum_and_clears_its_correction() {
    let mut p = project();
    let a = add(
        &mut p,
        StartAt::New,
        vec![flat(90.0, 10.0), flat(90.0, 10.0)],
    );
    let b = add(&mut p, StartAt::New, vec![flat(0.0, 5.0)]);
    let (a2, b0, b1) = (at(&p, a, 2), at(&p, b, 0), at(&p, b, 1));
    p.push_datum(ZeroDatum::new(at(&p, a, 0), gps(20.0, -87.0)));
    p.push_datum(ZeroDatum::new(b0, gps(20.01, -87.0)));
    p.datums[1].correction = Some([1.0, 1.0]);
    let ctx = EditContext::merge(a2, b0).with_renamed(b0, a2);
    let (before, report) = edit_and_settle(&mut p, &ctx, |p| {
        replace_station(p, b0, a2, None);
        p.datums[1].station = a2;
    });
    assert_eq!(report.demoted, vec![1]);
    assert_eq!(p.datums[1].correction, None);
    let after = p.raw_positions();
    assert_moved_by(before.positions(), &after, [0.0, 0.0], &[b1]);
    let [x, y] = xy_of(&after, a2);
    assert_xy_near(xy_of(&after, b1), [x, y + 5.0], TOL, "b1 follows the merge");
}

#[test]
fn detaching_a_line_keeps_every_position() {
    let (mut p, [_, l1, _]) = three_line_junction();
    let s1 = at(&p, l1, 0);
    let (before, _) = edit_and_settle(&mut p, &EditContext::line(l1).with_renamed(s1, 7), |p| {
        let fresh = p.mint_station("1").unwrap();
        assert_eq!(fresh, 7);
        replace_station(p, s1, fresh, Some(l1));
    });
    assert_eq!(p.topology().components().len(), 2);
    let after = p.raw_positions();
    assert_moved_by(before.positions(), &after, [0.0, 0.0], &[]);
    assert_xy_near(
        xy_of(&after, 7),
        before.position(s1).unwrap(),
        TOL,
        "s1' on s1",
    );
    // The detached piece's placement is its lowest station.
    assert_eq!(p.placements[1].station, at(&p, l1, 1));
}

#[test]
fn a_fix_attached_via_the_detached_line_places_that_line_on_its_gps() {
    let (mut p, [_, l1, _]) = three_line_junction();
    p.placements.clear();
    let (g0, gf) = (gps(20.0, -87.0), gps(20.0002, -86.9998));
    p.push_datum(ZeroDatum::new(0, g0));
    let s1 = at(&p, l1, 0);
    let mut fix = ZeroDatum::new(s1, gf);
    fix.via = Some(l1);
    p.push_datum(fix);
    assert_eq!(p.validate(), Vec::new());
    let (before, report) =
        edit_and_settle(&mut p, &EditContext::line(l1).with_renamed(s1, 7), |p| {
            let fresh = p.mint_station("1").unwrap();
            replace_station(p, s1, fresh, Some(l1));
            p.datums[1].station = fresh;
        });
    assert_eq!(report.placed_on_gps, vec![1]);
    let after = p.raw_positions();
    assert_eq!(xy_of(&after, 7), local(gf, g0));
    assert_xy_near(
        xy_of(&after, s1),
        before.position(s1).unwrap(),
        TOL,
        "s1 stays",
    );
}

// ----- datums and the frame ------------------------------------------------

#[test]
fn a_first_datum_moves_the_survey_onto_it_as_one_body() {
    let (mut p, [l0, _, _]) = three_line_junction();
    let q = add(&mut p, StartAt::New, vec![flat(0.0, 3.0)]);
    let q0 = at(&p, q, 0);
    p.set_placement(q0, 500.0, 500.0).unwrap();
    let s2 = at(&p, l0, 2);
    let (before, report) = edit_and_settle(&mut p, &EditContext::none(), |p| {
        p.push_datum(ZeroDatum::new(s2, gps(20.0, -87.0)));
    });
    let t = before.position(s2).map(|[x, y]| [-x, -y]).unwrap();
    assert!(report.frame_changed);
    assert_eq!(report.translation, t);
    assert_eq!(report.placed_on_gps, vec![0]);
    assert_eq!(report.placements_dropped, vec![0]);
    assert_eq!(p.datums[0].correction, None);
    let after = p.raw_positions();
    assert_eq!(xy_of(&after, s2), [0.0, 0.0]);
    assert_moved_by(before.positions(), &after, t, &[]);
    assert_eq!(
        p.placements,
        vec![Placement {
            station: q0,
            x: 500.0 + t[0],
            y: 500.0 + t[1]
        }]
    );
}

#[test]
fn a_later_datum_on_a_free_piece_places_it_on_its_gps() {
    let (mut p, _) = three_line_junction();
    let q = add(&mut p, StartAt::New, vec![flat(0.0, 3.0)]);
    let (q0, q1) = (at(&p, q, 0), at(&p, q, 1));
    p.set_placement(q0, 500.0, 500.0).unwrap();
    let (g0, g1) = (gps(20.0, -87.0), gps(20.001, -87.002));
    edit_and_settle(&mut p, &EditContext::none(), |p| {
        p.push_datum(ZeroDatum::new(0, g0));
    });
    let (before, report) = edit_and_settle(&mut p, &EditContext::none(), |p| {
        p.push_datum(ZeroDatum::new(q1, g1));
    });
    assert!(!report.frame_changed);
    assert_eq!(report.placed_on_gps, vec![1]);
    assert_eq!(report.placements_dropped, vec![q0]);
    let after = p.raw_positions();
    assert_eq!(xy_of(&after, q1), local(g1, g0));
    assert_moved_by(before.positions(), &after, [0.0, 0.0], &[q0, q1]);
}

/// Three datum pieces: A (frame), B, C (C off its GPS by a legacy correction).
fn three_datum_pieces() -> (Project, [LineId; 3], [GpsCoordinates; 3]) {
    let mut p = project();
    let a = add(
        &mut p,
        StartAt::New,
        vec![flat(90.0, 10.0), flat(0.0, 10.0)],
    );
    let b = add(&mut p, StartAt::New, vec![flat(45.0, 10.0)]);
    let c = add(&mut p, StartAt::New, vec![shot(180.0, 12.0, 1.0, 3.0)]);
    let g = [gps(20.0, -87.0), gps(20.02, -86.98), gps(19.99, -87.03)];
    p.push_datum(ZeroDatum::new(at(&p, a, 0), g[0]));
    p.push_datum(ZeroDatum::new(at(&p, b, 0), g[1]));
    p.push_datum(ZeroDatum::new(at(&p, c, 1), g[2]));
    p.datums[2].correction = Some([5.0, 5.0]);
    assert_eq!(p.validate(), Vec::new());
    (p, [a, b, c], g)
}

#[test]
fn removing_the_frame_datum_translates_everything_and_moves_nothing_relative() {
    let (mut p, [a, b, _], _) = three_datum_pieces();
    let b0 = at(&p, b, 0);
    let (before, report) = edit_and_settle(&mut p, &EditContext::none(), |p| {
        p.datums.remove(0);
    });
    let t = before.position(b0).map(|[x, y]| [-x, -y]).unwrap();
    assert!(report.frame_changed);
    assert_eq!(report.translation, t);
    assert!(report.placed_on_gps.is_empty());
    let after = p.raw_positions();
    assert_eq!(xy_of(&after, b0), [0.0, 0.0]);
    assert_eq!(p.datums[0].correction, None);
    assert!(p.datums[1].correction.is_some());
    assert_moved_by(before.positions(), &after, t, &[]);
    // A is a free piece now, held where it was.
    assert_eq!(p.placements.len(), 1);
    assert_eq!(p.placements[0].station, at(&p, a, 0));
}

#[test]
fn removing_the_frame_datum_promotes_a_fix_in_its_piece_onto_its_gps() {
    let (mut p, [a, b, _], g) = three_datum_pieces();
    let a2 = at(&p, a, 2);
    let gf = gps(20.0001, -86.9999);
    // A fix at A's last station, some 15 m from where that station sits.
    p.datums.insert(1, ZeroDatum::new(a2, gf));
    let (before, report) = edit_and_settle(&mut p, &EditContext::none(), |p| {
        p.datums.remove(0);
    });
    let x = local(gf, g[0]);
    let t = [-x[0], -x[1]];
    assert!(report.frame_changed);
    assert_eq!(report.placed_on_gps, vec![0]);
    assert_eq!(report.translation, t);
    let after = p.raw_positions();
    assert_eq!(xy_of(&after, a2), [0.0, 0.0]);
    // The other pieces keep their place relative to each other and to where
    // A's fix said A should be.
    assert_moved_by(before.positions(), &after, t, &at_all(&p, a));
    let b0 = at(&p, b, 0);
    assert_xy_near(
        xy_of(&after, b0),
        [
            before.position(b0).unwrap()[0] + t[0],
            before.position(b0).unwrap()[1] + t[1],
        ],
        TOL,
        "b0",
    );
}

fn at_all(p: &Project, line: LineId) -> Vec<StationId> {
    p.line_stations(line).unwrap().to_vec()
}

#[test]
fn removing_the_last_datum_moves_nothing() {
    let mut p = project();
    let a = add(&mut p, StartAt::New, vec![flat(90.0, 10.0)]);
    p.push_datum(ZeroDatum::new(at(&p, a, 1), gps(20.0, -87.0)));
    p.datums[0].correction = Some([2.0, 0.0]);
    let (before, report) = edit_and_settle(&mut p, &EditContext::none(), |p| {
        p.datums.clear();
    });
    assert!(!report.frame_changed);
    assert_moved_by(before.positions(), &p.raw_positions(), [0.0, 0.0], &[]);
    assert_eq!(p.placements.len(), 1);
}

#[test]
fn a_frame_gps_edit_keeps_the_frame_at_the_origin_and_rederives_the_others() {
    let (mut p, [a, b, c], g) = three_datum_pieces();
    let q = add(&mut p, StartAt::New, vec![flat(0.0, 3.0)]);
    p.set_placement(at(&p, q, 0), 50.0, 60.0).unwrap();
    p.datums[0].correction = Some([0.5, 0.5]);
    let moved = gps(20.0005, -87.0005);
    let ctx = EditContext::none().with_gps_edit(0, g[0]);
    let (_, report) = edit_and_settle(&mut p, &ctx, |p| {
        p.datums[0].gps = moved;
    });
    assert!(!report.frame_changed);
    assert_eq!(report.placed_on_gps, vec![0]);
    assert_eq!(p.datums[0].correction, None);
    assert_eq!(p.datums[2].correction, Some([5.0, 5.0]));
    let after = p.raw_positions();
    assert_eq!(xy_of(&after, at(&p, a, 0)), [0.0, 0.0]);
    assert_eq!(xy_of(&after, at(&p, b, 0)), local(g[1], moved));
    let c1 = local(g[2], moved);
    assert_eq!(xy_of(&after, at(&p, c, 1)), [c1[0] + 5.0, c1[1] + 5.0]);
    // A free piece is not georeferenced: it keeps its local place.
    assert_eq!(
        p.placements,
        vec![Placement {
            station: at(&p, q, 0),
            x: 50.0,
            y: 60.0
        }]
    );
}

#[test]
fn a_gps_edit_of_another_placement_datum_clears_its_correction() {
    let (mut p, [_, _, c], g) = three_datum_pieces();
    let moved = gps(19.991, -87.031);
    let ctx = EditContext::none().with_gps_edit(2, g[2]);
    edit_and_settle(&mut p, &ctx, |p| {
        p.datums[2].gps = moved;
    });
    assert_eq!(p.datums[2].correction, None);
    assert_eq!(xy_of(&p.raw_positions(), at(&p, c, 1)), local(moved, g[0]));
}

#[test]
fn settling_twice_changes_nothing() {
    let (mut p, _, _) = three_datum_pieces();
    edit_and_settle(&mut p, &EditContext::none(), |p| {
        p.datums.remove(0);
    });
    let settled = p.clone();
    let (_, report) = edit_and_settle(&mut p, &EditContext::none(), |_| {});
    assert_eq!(report, AnchorReport::default());
    assert_eq!(p, settled);
}

#[test]
fn correction_to_hold_is_the_frame_change_formula() {
    let (p, _, g) = three_datum_pieces();
    assert_eq!(p.correction_to_hold(0, [1.0, 2.0]), Some([1.0, 2.0]));
    assert_eq!(p.correction_to_hold(0, [0.0, 0.0]), None);
    let base = local(g[1], g[0]);
    assert_eq!(p.correction_to_hold(1, base), None);
    let held = p.correction_to_hold(1, [base[0] + 3.0, base[1]]).unwrap();
    assert!((held[0] - 3.0).abs() < 1e-9 && held[1].abs() < 1e-9);
}
