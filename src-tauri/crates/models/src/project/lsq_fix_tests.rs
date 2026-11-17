//! GPS fixes in the Closures Computed View solve (#741).
//!
//! The scenario is the plan's: two entrances joined by a 500 m traverse, the
//! first entrance holding the placement datum and the second a GPS fix whose
//! GPS is 40 m off the surveyed position, across the traverse. The traverse is
//! a chain of springs grounded at the placement datum, so the solve has a
//! closed form: the fix station moves by `gap * V / (V + accuracy^2)` per axis,
//! `V` being the summed shot variance along the chain on that axis.

use std::collections::HashSet;

use crate::config::ClosureAdjustmentConfig;
use crate::{
    gps_to_local, local_to_gps, ClosureStrategy, Coordinates, DatumRole, GpsCoordinates,
    LoopClosure, Project, LineData, LineId, ShotData, StationRef, ZeroDatum,
};

const SHOTS_PER_LEG: usize = 25;
const SHOT_HORIZONTAL_M: f64 = 10.0;
/// The fix's GPS sits this far north of where the traverse puts its station.
const GAP_NORTH_M: f64 = 40.0;

fn origin() -> GpsCoordinates {
    GpsCoordinates::new(20.5, -87.3, 0.0).unwrap()
}

/// A shot of `SHOT_HORIZONTAL_M` horizontal on `azimuth`, sloping from
/// `depth_in` to `depth_out`.
fn shot(azimuth: f64, depth_in: f64, depth_out: f64) -> ShotData {
    let dz = depth_out - depth_in;
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length: SHOT_HORIZONTAL_M.hypot(dz),
        depth_in,
        depth_out,
        ..Default::default()
    }
}

/// `SHOTS_PER_LEG` shots on `azimuth` with a depth that wanders, so a solve
/// that touched depth would show.
fn leg(azimuth: f64) -> Vec<ShotData> {
    let depth = |k: usize| 8.0 + (k % 4) as f64 * 1.5;
    (0..SHOTS_PER_LEG)
        .map(|k| shot(azimuth, depth(k), depth(k + 1)))
        .collect()
}

fn add(project: &mut Project, shots: Vec<ShotData>) -> LineId {
    project
        .add_line(LineData {
            shots,
            ..Default::default()
        })
        .id
}

fn raw(project: &Project, at: StationRef) -> Coordinates {
    project.compute_station_raw_coords(at).unwrap()
}

/// The GPS a fix needs to sit `(east, north)` from the frame origin.
fn gps_at(east: f64, north: f64) -> GpsCoordinates {
    let (lat, lon) = local_to_gps(east, north, &origin()).unwrap();
    // A depth reference the solve must ignore.
    GpsCoordinates::new(lat, lon, 33.0).unwrap()
}

struct Traverse {
    project: Project,
    /// Entrance 1, the placement datum, at local (0, 0).
    entrance_1: StationRef,
    /// Entrance 2, the GPS fix, surveyed at (500, 0).
    entrance_2: StationRef,
    /// The fix's GPS position in local metres.
    gps: (f64, f64),
}

/// Entrance 1 runs 250 m east; entrance 2 runs 250 m west and is merged onto
/// its end. Entrance 1 holds the placement datum, entrance 2 a fix with
/// `accuracy_m`.
fn traverse(accuracy_m: Option<f64>) -> Traverse {
    let mut project = Project::new("Two entrances".to_string(), None);
    let a = add(&mut project, leg(90.0));
    let b = add(&mut project, leg(270.0));
    let end = SHOTS_PER_LEG as u32;
    project
        .merge_stations(StationRef::new(b, end), StationRef::new(a, end))
        .unwrap();

    let entrance_1 = StationRef::new(a, 0);
    let entrance_2 = StationRef::new(b, 0);
    let surveyed = raw(&project, entrance_2);
    assert!(raw(&project, entrance_1).x.abs() < 1e-9);
    assert!((surveyed.x - 500.0).abs() < 1e-6, "{surveyed:?}");

    let fix_gps = gps_at(surveyed.x, surveyed.y + GAP_NORTH_M);
    project.zero_datums.push(ZeroDatum::new(
        project.key_for_ref(entrance_1).unwrap(),
        origin(),
    ));
    project.zero_datums.push(ZeroDatum {
        station_ref: project.key_for_ref(entrance_2).unwrap(),
        gps: fix_gps,
        accuracy_m,
        entrance: None,
    });
    assert_eq!(
        project.datum_roles(),
        vec![Some(DatumRole::Placement), Some(DatumRole::GpsFix)]
    );
    Traverse {
        project,
        entrance_1,
        entrance_2,
        gps: gps_to_local(&fix_gps, &origin()),
    }
}

/// Solve `project`'s component around `anchor` with `closures`, as the
/// Computed View does on its clone.
fn solve(project: &mut Project, anchor: StationRef, closures: &[LoopClosure]) {
    let component = project.get_connected_line_ids(anchor.line_id);
    project
        .adjust_least_squares(
            &component,
            closures,
            anchor,
            &ClosureAdjustmentConfig::default(),
        )
        .unwrap();
}

/// Every station's raw position, in a fixed order.
fn all_positions(project: &Project) -> Vec<(StationRef, Coordinates)> {
    project.compute_station_positions()
}

fn assert_depths_unchanged(
    before: &[(StationRef, Coordinates)],
    after: &[(StationRef, Coordinates)],
) {
    assert_eq!(before.len(), after.len());
    for ((ra, a), (rb, b)) in before.iter().zip(after) {
        assert_eq!(ra, rb);
        assert!((a.z - b.z).abs() < 1e-9, "{ra:?} depth {} -> {}", a.z, b.z);
    }
}

fn assert_at(project: &Project, at: StationRef, x: f64, y: f64) {
    let c = raw(project, at);
    assert!(
        (c.x - x).abs() < 1e-6 && (c.y - y).abs() < 1e-6,
        "{at:?} at ({}, {}), expected ({x}, {y})",
        c.x,
        c.y
    );
}

/// Summed per-axis shot variance along every shot of `project`, by the
/// solve's own model at the default config. The traverse is one chain, so
/// that is its series compliance.
fn chain_variance(project: &Project) -> (f64, f64) {
    let config = ClosureAdjustmentConfig::default();
    let sigma_len = config.base_sigma_length_m;
    let sigma_az = config.base_sigma_azimuth_deg.to_radians();
    let mut v = (0.0, 0.0);
    for line in &project.lines {
        for s in &line.shots {
            let az = s.azimuth_in.to_radians();
            v.0 += (az.sin() * sigma_len).powi(2) + (s.length * az.cos() * sigma_az).powi(2);
            v.1 += (az.cos() * sigma_len).powi(2) + (s.length * az.sin() * sigma_az).powi(2);
        }
    }
    v
}

#[test]
fn hard_fix_puts_its_station_on_its_gps() {
    let Traverse {
        mut project,
        entrance_1,
        entrance_2,
        gps,
    } = traverse(Some(0.0));
    let before = all_positions(&project);

    solve(&mut project, entrance_1, &[]);

    assert_at(&project, entrance_2, gps.0, gps.1);
    assert_at(&project, entrance_1, 0.0, 0.0);
    assert_depths_unchanged(&before, &all_positions(&project));
}

#[test]
fn five_metre_fix_lands_where_the_variance_model_puts_it() {
    let Traverse {
        mut project,
        entrance_1,
        entrance_2,
        gps,
    } = traverse(Some(5.0));
    let surveyed = raw(&project, entrance_2);
    let (vx, vy) = chain_variance(&project);
    let gps_var = 5.0_f64.powi(2);
    let expect_x = surveyed.x + (gps.0 - surveyed.x) * vx / (vx + gps_var);
    let expect_y = surveyed.y + (gps.1 - surveyed.y) * vy / (vy + gps_var);
    let before = all_positions(&project);

    solve(&mut project, entrance_1, &[]);

    assert_at(&project, entrance_2, expect_x, expect_y);
    assert_at(&project, entrance_1, 0.0, 0.0);
    assert_depths_unchanged(&before, &all_positions(&project));

    // Between the survey and the GPS, and, with the default shot sigmas,
    // much nearer the survey: the 500 m traverse is trusted to ~1.3 m across
    // its length, the GPS to 5 m. (A level traverse gives 2.30 m; these
    // shots slope, and the azimuth term scales with slope length.)
    let moved = raw(&project, entrance_2).y - surveyed.y;
    assert!(moved > 0.0 && moved < GAP_NORTH_M / 2.0, "moved {moved}");
    assert!((moved - 2.4387).abs() < 1e-3, "moved {moved}");
}

#[test]
fn a_bad_stored_accuracy_pins_the_fix_like_the_default() {
    // The default is 0, a hard pin, so a bad stored accuracy must put the fix
    // station ON its GPS. Infinity is the value that tells: read as a weight it
    // is `1 / inf^2 = 0`, a fix that does not pull at all.
    for bad in [-1.0, f64::NAN, f64::INFINITY] {
        let Traverse {
            mut project,
            entrance_1,
            entrance_2,
            gps,
        } = traverse(Some(bad));
        solve(&mut project, entrance_1, &[]);
        assert_at(&project, entrance_2, gps.0, gps.1);
    }
}

#[test]
fn a_fix_and_a_closure_are_solved_together() {
    let Traverse {
        mut project,
        entrance_1,
        entrance_2,
        gps,
    } = traverse(None);
    let a = entrance_1.line_id;
    // A side loop off station 10 (100, 0) that comes back near station 12
    // (120, 0), 0.3 m short.
    let c = add(
        &mut project,
        vec![
            shot(0.0, 10.0, 10.0),
            ShotData {
                length: 20.0,
                ..shot(90.0, 10.0, 10.0)
            },
            ShotData {
                length: 10.3,
                ..shot(180.0, 10.0, 10.0)
            },
        ],
    );
    project
        .merge_stations(StationRef::new(c, 0), StationRef::new(a, 10))
        .unwrap();
    let closure = LoopClosure {
        id: 1,
        station_a: project.key_for_ref(StationRef::new(c, 3)).unwrap(),
        station_b: project.key_for_ref(StationRef::new(a, 12)).unwrap(),
        path_length: 40.0,
        misclosure_distance: 0.3,
        error_percentage: 0.75,
        horizontal_misclosure: 0.3,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };
    let surveyed = raw(&project, entrance_2);
    let before = all_positions(&project);

    solve(&mut project, entrance_1, &[closure]);

    assert_at(&project, entrance_1, 0.0, 0.0);
    let after = raw(&project, entrance_2);
    let gap = |p: Coordinates| (gps.0 - p.x).hypot(gps.1 - p.y);
    assert!(
        gap(after) < gap(surveyed) - 1.0,
        "{surveyed:?} -> {after:?}"
    );
    let (c3, a12) = (
        raw(&project, StationRef::new(c, 3)),
        raw(&project, StationRef::new(a, 12)),
    );
    assert!((c3.x - a12.x).hypot(c3.y - a12.y) < 1e-6, "loop not closed");
    assert_depths_unchanged(&before, &all_positions(&project));
}

#[test]
fn a_fix_in_the_placement_datums_own_group_moves_nothing() {
    let mut project = Project::new("Fix on the datum".to_string(), None);
    let a = add(&mut project, leg(90.0));
    let d = add(&mut project, leg(0.0));
    project
        .merge_stations(StationRef::new(d, 0), StationRef::new(a, 0))
        .unwrap();
    project.zero_datums.push(ZeroDatum::new(
        project.key_for_ref(StationRef::new(a, 0)).unwrap(),
        origin(),
    ));
    project.zero_datums.push(ZeroDatum {
        station_ref: project.key_for_ref(StationRef::new(d, 0)).unwrap(),
        gps: gps_at(0.0, GAP_NORTH_M),
        accuracy_m: Some(0.0),
        entrance: None,
    });
    let before = all_positions(&project);

    solve(&mut project, StationRef::new(a, 0), &[]);

    for ((r, b), (_, c)) in before.iter().zip(all_positions(&project)) {
        assert!((b.x - c.x).hypot(b.y - c.y) < 1e-6, "{r:?} moved");
    }
}

#[test]
fn two_placement_datums_in_one_group_are_refused_not_a_panic() {
    let mut project = Project::new("Two placements".to_string(), None);
    let a = add(&mut project, leg(90.0));
    let e = add(&mut project, leg(0.0));
    for (s, gps) in [(a, origin()), (e, gps_at(0.0, 1.0))] {
        project.zero_datums.push(ZeroDatum::new(
            project.key_for_ref(StationRef::new(s, 0)).unwrap(),
            gps,
        ));
    }
    let closure = LoopClosure {
        id: 1,
        station_a: project.key_for_ref(StationRef::new(a, 0)).unwrap(),
        station_b: project.key_for_ref(StationRef::new(e, 0)).unwrap(),
        path_length: 1.0,
        misclosure_distance: 1.0,
        error_percentage: 100.0,
        horizontal_misclosure: 1.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };
    let component: HashSet<LineId> = [a, e].into_iter().collect();
    let result = project.adjust_least_squares(
        &component,
        &[closure],
        StationRef::new(a, 0),
        &ClosureAdjustmentConfig::default(),
    );
    assert!(result.is_err(), "{result:?}");
}

#[test]
fn a_hard_fix_one_shot_from_the_placement_datum_reaches_its_gps() {
    // A single shot: its start holds the placement datum and its end a hard
    // fix. Both groups are held, so nothing is left to solve, and the shot
    // between them still has to be rewritten to put the fix on its GPS.
    let mut project = Project::new("One shot".to_string(), None);
    let a = add(&mut project, vec![shot(90.0, 5.0, 7.0)]);
    let (start, end) = (StationRef::new(a, 0), StationRef::new(a, 1));
    let surveyed = raw(&project, end);
    let fix_gps = gps_at(surveyed.x, surveyed.y + GAP_NORTH_M);
    project.zero_datums.push(ZeroDatum::new(
        project.key_for_ref(start).unwrap(),
        origin(),
    ));
    project.zero_datums.push(ZeroDatum {
        station_ref: project.key_for_ref(end).unwrap(),
        gps: fix_gps,
        accuracy_m: None,
        entrance: None,
    });
    let before = all_positions(&project);

    solve(&mut project, start, &[]);

    let (gx, gy) = gps_to_local(&fix_gps, &origin());
    assert_at(&project, end, gx, gy);
    assert_at(&project, start, 0.0, 0.0);
    assert_depths_unchanged(&before, &all_positions(&project));
}

#[test]
fn a_fix_whose_gps_is_no_position_is_left_out_of_the_solve() {
    // A damaged file can hold an infinite latitude. As a hard pin it would
    // put a NaN position into the solve and turn every station to NaN; it is
    // no observation, so the computed view solves as if it were not there.
    let Traverse {
        mut project,
        entrance_1,
        ..
    } = traverse(Some(0.0));
    project.zero_datums[1].gps.latitude = f64::INFINITY;

    solve(&mut project, entrance_1, &[]);

    for (at, c) in all_positions(&project) {
        assert!(c.x.is_finite() && c.y.is_finite(), "{at:?} at {c:?}");
    }
    assert_at(&project, entrance_1, 0.0, 0.0);
}
