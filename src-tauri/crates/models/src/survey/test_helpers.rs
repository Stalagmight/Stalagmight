//! Synthetic survey builders shared by the survey tests. Fully synthetic data;
//! every uuid is derived from the line id and shot index, so building the same
//! survey twice gives the same bytes.

use chrono::{TimeZone, Utc};

use crate::{GpsCoordinates, LineId, ShotData, StationId};

use super::{BuildError, LineMeta, Project, StartAt};

/// A shot with one azimuth reading pair (both `azimuth`), a slope length and
/// a depth pair.
pub(super) fn shot(azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in,
        depth_out,
        ..ShotData::default()
    }
}

/// A flat shot (no depth change) at depth 0.
pub(super) fn flat(azimuth: f64, length: f64) -> ShotData {
    shot(azimuth, length, 0.0, 0.0)
}

/// An empty project with a fixed creation instant.
pub(super) fn project() -> Project {
    Project::new_at(
        "Synthetic",
        None,
        Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
            .single()
            .unwrap_or_default(),
    )
}

/// Line metadata with a name.
pub(super) fn meta(name: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        color: "#4ecdc4".to_string(),
        ..LineMeta::default()
    }
}

/// Give each shot a uuid derived from the line id the next line will get.
fn with_uuids(p: &Project, shots: Vec<ShotData>) -> Vec<ShotData> {
    let line = p.next_line_id();
    shots
        .into_iter()
        .enumerate()
        .map(|(i, mut data)| {
            data.uuid.get_or_insert_with(|| format!("shot-{line}-{i}"));
            data
        })
        .collect()
}

/// Add a line from `start` with deterministic uuids.
pub(super) fn try_add(
    p: &mut Project,
    start: StartAt,
    shots: Vec<ShotData>,
) -> Result<LineId, BuildError> {
    let shots = with_uuids(p, shots);
    let name = format!("L{}", p.next_line_id());
    p.add_line(meta(&name), start, shots)
}

/// Add a line from `start` with deterministic uuids; panics on refusal.
pub(super) fn add(p: &mut Project, start: StartAt, shots: Vec<ShotData>) -> LineId {
    match try_add(p, start, shots) {
        Ok(line) => line,
        Err(error) => panic!("add_line refused: {error}"),
    }
}

/// Add a line over existing stations with deterministic uuids; panics on
/// refusal.
pub(super) fn add_on(p: &mut Project, chain: &[StationId], shots: Vec<ShotData>) -> LineId {
    let shots = with_uuids(p, shots);
    let name = format!("L{}", p.next_line_id());
    match p.add_line_on_stations(meta(&name), chain, shots) {
        Ok(line) => line,
        Err(error) => panic!("add_line_on_stations refused: {error}"),
    }
}

/// The station at chain index `index` of `line`.
pub(super) fn at(p: &Project, line: LineId, index: usize) -> StationId {
    match p.line_stations(line).and_then(|chain| chain.get(index)) {
        Some(&station) => station,
        None => panic!("line {line} has no station {index}"),
    }
}

/// A GPS position (depth reference 0).
pub(super) fn gps(latitude: f64, longitude: f64) -> GpsCoordinates {
    GpsCoordinates {
        latitude,
        longitude,
        depth_reference: 0.0,
    }
}

/// Assert two horizontal positions agree within `tolerance` metres.
pub(super) fn assert_xy_near(actual: [f64; 2], expected: [f64; 2], tolerance: f64, what: &str) {
    let gap = ((actual[0] - expected[0]).powi(2) + (actual[1] - expected[1]).powi(2)).sqrt();
    assert!(
        gap <= tolerance,
        "{what}: {actual:?} is {gap} m from {expected:?} (tolerance {tolerance})"
    );
}

/// A position that must exist.
pub(super) fn xy_of(positions: &super::RawPositions, station: StationId) -> [f64; 2] {
    match positions.xy(station) {
        Some(xy) => xy,
        None => panic!("station {station} has no position"),
    }
}

/// A depth that must exist.
pub(super) fn z_of(positions: &super::RawPositions, station: StationId) -> f64 {
    match positions.z(station) {
        Some(z) => z,
        None => panic!("station {station} has no depth"),
    }
}

/// A three-line junction survey, free (one placement), valid:
///
/// ```text
/// L0: s0 -> s1 -> s2 -> s3        (east, 10 m each)
/// L1: s1 -> a -> b                (north, 5 m each)  junction at s1
/// L2: s4 -> s3                    (west 7 m, ending on L0's last station)
/// ```
pub(super) fn three_line_junction() -> (Project, [LineId; 3]) {
    let mut p = project();
    let l0 = add(
        &mut p,
        StartAt::New,
        vec![
            flat(90.0, 10.0),
            shot(90.0, 10.0, 0.0, 2.0),
            flat(90.0, 10.0),
        ],
    );
    let s1 = at(&p, l0, 1);
    let l1 = add(
        &mut p,
        StartAt::Station(s1),
        vec![shot(0.0, 5.0, 0.0, 1.0), flat(0.0, 5.0)],
    );
    let s3 = at(&p, l0, 3);
    let s4 = p.mint_station("x0").unwrap_or_default();
    let l2 = add_on(&mut p, &[s4, s3], vec![flat(270.0, 7.0)]);
    let s0 = at(&p, l0, 0);
    if let Err(error) = p.set_placement(s0, 100.0, 200.0) {
        panic!("placement refused: {error}");
    }
    (p, [l0, l1, l2])
}
