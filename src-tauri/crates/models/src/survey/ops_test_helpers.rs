//! Synthetic survey builders and assertions shared by the M2 operation
//! tests. Fully synthetic data.

use std::collections::BTreeMap;

use crate::{local_to_gps, GpsCoordinates, LineId, ShotData, StationId};

use super::test_helpers::{add, at, flat};
use super::{OpError, OpOutcome, Project, StartAt};

/// The frame GPS of every survey here (a tropical latitude, as in today's
/// tests).
pub(super) const ORIGIN_LAT: f64 = 20.5;
pub(super) const ORIGIN_LON: f64 = -87.25;

pub(super) fn origin() -> GpsCoordinates {
    GpsCoordinates {
        latitude: ORIGIN_LAT,
        longitude: ORIGIN_LON,
        depth_reference: 0.0,
    }
}

/// The GPS position `east` / `north` metres from [`origin`] (ENU there).
pub(super) fn gps_at(east: f64, north: f64) -> GpsCoordinates {
    gps_from(&origin(), east, north)
}

/// The GPS position `east` / `north` metres from `base` (ENU at `base`).
pub(super) fn gps_from(base: &GpsCoordinates, east: f64, north: f64) -> GpsCoordinates {
    match local_to_gps(east, north, base) {
        Some((latitude, longitude)) => GpsCoordinates {
            latitude,
            longitude,
            depth_reference: 0.0,
        },
        None => panic!("no GPS {east} m east, {north} m north of {base:?}"),
    }
}

/// `count` flat shots of `length` metres on `azimuth`.
pub(super) fn run(azimuth: f64, count: usize, length: f64) -> Vec<ShotData> {
    (0..count).map(|_| flat(azimuth, length)).collect()
}

/// A new free line (its own piece) of `shots`, its start placed at `at`.
pub(super) fn free_line(p: &mut Project, at_xy: [f64; 2], shots: Vec<ShotData>) -> LineId {
    let line = add(p, StartAt::New, shots);
    let start = at(p, line, 0);
    if let Err(error) = p.set_placement(start, at_xy[0], at_xy[1]) {
        panic!("placement refused: {error}");
    }
    line
}

/// A new line starting at an existing station (a junction there).
pub(super) fn branch(p: &mut Project, from: StationId, shots: Vec<ShotData>) -> LineId {
    add(p, StartAt::Station(from), shots)
}

/// Every station's horizontal position.
pub(super) fn xy_all(p: &Project) -> BTreeMap<StationId, [f64; 2]> {
    p.raw_positions()
        .iter()
        .map(|(s, [x, y, _])| (s, [x, y]))
        .collect()
}

/// Every station's depth.
pub(super) fn z_all(p: &Project) -> BTreeMap<StationId, f64> {
    p.raw_positions()
        .iter()
        .map(|(s, [_, _, z])| (s, z))
        .collect()
}

/// A station's horizontal position.
pub(super) fn xy(p: &Project, station: StationId) -> [f64; 2] {
    match p.raw_positions().xy(station) {
        Some(xy) => xy,
        None => panic!("station {station} has no position"),
    }
}

/// Assert `actual` is within `tolerance` of `expected`.
pub(super) fn near(actual: [f64; 2], expected: [f64; 2], tolerance: f64, what: &str) {
    let gap = (actual[0] - expected[0]).hypot(actual[1] - expected[1]);
    assert!(
        gap <= tolerance,
        "{what}: {actual:?} is {gap} m from {expected:?} (tolerance {tolerance})"
    );
}

/// Every station of `before` still present in `after` moved by `shift(s)`.
pub(super) fn assert_shifted(
    before: &BTreeMap<StationId, [f64; 2]>,
    after: &BTreeMap<StationId, [f64; 2]>,
    tolerance: f64,
    shift: impl Fn(StationId) -> [f64; 2],
) {
    for (&station, &[bx, by]) in before {
        let Some(&now) = after.get(&station) else {
            continue;
        };
        let [dx, dy] = shift(station);
        near(
            now,
            [bx + dx, by + dy],
            tolerance,
            &format!("station {station}"),
        );
    }
}

/// Nothing moved (every station present in both).
pub(super) fn assert_still(
    before: &BTreeMap<StationId, [f64; 2]>,
    after: &BTreeMap<StationId, [f64; 2]>,
    tolerance: f64,
) {
    assert_shifted(before, after, tolerance, |_| [0.0, 0.0]);
}

/// The value of a successful command; panics with the refusal otherwise.
pub(super) fn ok<T>(result: Result<OpOutcome<T>, OpError>) -> OpOutcome<T> {
    match result {
        Ok(outcome) => outcome,
        Err(error) => panic!("command refused: {error} ({error:?})"),
    }
}

/// The model must be valid.
pub(super) fn assert_valid(p: &Project) {
    assert_eq!(p.validate(), Vec::new(), "model must be valid");
}

/// Where a datum's GPS puts it in the current frame.
pub(super) fn gps_local(p: &Project, gps: &GpsCoordinates) -> [f64; 2] {
    let frame = match p.frame_datum() {
        Some(frame) => p.datums[frame].gps,
        None => panic!("no frame"),
    };
    let (x, y) = crate::gps_to_local(gps, &frame);
    [x, y]
}
