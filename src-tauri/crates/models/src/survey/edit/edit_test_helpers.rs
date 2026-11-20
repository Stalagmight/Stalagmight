//! Shared builders and assertions for the operation tests. Fully synthetic
//! data; every uuid is derived from the line id and shot index (or given),
//! so building the same survey twice gives the same bytes.

use crate::{LineId, ShotData, ShotId, StationId};

use super::super::test_helpers::{flat, meta, project, shot};
use super::super::{Project, RawPositions, StartAt};
use super::{EditError, Edited};

/// A flat shot with a fixed uuid.
pub(super) fn flat_u(azimuth: f64, length: f64, uuid: &str) -> ShotData {
    ShotData {
        uuid: Some(uuid.to_string()),
        ..flat(azimuth, length)
    }
}

/// A shot with depths and a fixed uuid.
pub(super) fn shot_u(
    azimuth: f64,
    length: f64,
    depth_in: f64,
    depth_out: f64,
    uuid: &str,
) -> ShotData {
    ShotData {
        uuid: Some(uuid.to_string()),
        ..shot(azimuth, length, depth_in, depth_out)
    }
}

/// `count` flat shots of `length` along `azimuth`, uuids `"{tag}-{i}"`.
pub(super) fn run(tag: &str, azimuth: f64, length: f64, count: usize) -> Vec<ShotData> {
    (0..count)
        .map(|i| flat_u(azimuth, length, &format!("{tag}-{i}")))
        .collect()
}

/// Unwrap an edit, panicking with the refusal.
pub(super) fn ok<T>(result: Result<Edited<T>, EditError>) -> T {
    match result {
        Ok(edited) => edited.value,
        Err(error) => panic!("edit refused: {error}"),
    }
}

/// The refusal text of an edit that must be refused.
pub(super) fn refused<T: std::fmt::Debug>(result: Result<T, EditError>) -> String {
    match result {
        Ok(value) => panic!("edit was not refused: {value:?}"),
        Err(EditError::Refused(text)) => text,
        Err(EditError::Invalid(violations)) => panic!("edit invalid, not refused: {violations:?}"),
    }
}

/// A line created through the operation at the origin, named `name`.
pub(super) fn line(p: &mut Project, name: &str, shots: Vec<ShotData>) -> LineId {
    ok(p.create_line(meta(name), StartAt::New, shots, None))
}

/// A line created through the operation from an existing station.
pub(super) fn line_from(
    p: &mut Project,
    name: &str,
    start: StationId,
    shots: Vec<ShotData>,
) -> LineId {
    ok(p.create_line(meta(name), StartAt::Station(start), shots, None))
}

/// A line created through the operation with its start placed at `at`.
pub(super) fn line_at(p: &mut Project, name: &str, at: [f64; 2], shots: Vec<ShotData>) -> LineId {
    ok(p.create_line(meta(name), StartAt::New, shots, Some(at)))
}

/// Put a datum on `station` through the anchor protocol (snapshot, push,
/// settle), as a Set Datum command would.
pub(super) fn add_datum(p: &mut Project, station: StationId, at: crate::GpsCoordinates) {
    let before = p.anchor_snapshot();
    p.datums.push(super::super::ZeroDatum::new(station, at));
    p.settle_anchors(&before, &super::super::EditContext::none());
}

/// Merge station `moving` into `stationary` the way plan 5.1 / 5.5 merge
/// (a test stand-in for stream M2's Merge Stations): `moving`'s piece moves
/// so `moving` lands on `stationary`, which survives (its id and name; the
/// other becomes an alias). Panics if refused or invalid.
pub(super) fn merge(p: &mut Project, moving: StationId, stationary: StationId) {
    let before = p.anchor_snapshot();
    if let Err(error) = p.fold_station(moving, stationary, super::CommentRule::Join) {
        panic!("merge refused: {error}");
    }
    let ctx = super::super::EditContext::merge(stationary, moving).with_renamed(moving, stationary);
    p.settle_anchors(&before, &ctx);
    let violations = p.validate();
    assert!(
        violations.is_empty(),
        "merge left an invalid model: {violations:?}"
    );
}

/// An empty project.
pub(super) fn empty() -> Project {
    project()
}

/// The session id of shot `index` of `line`.
pub(super) fn shot_id(p: &Project, line: LineId, index: usize) -> ShotId {
    match p.line(line).and_then(|l| l.shots.get(index)) {
        Some(shot) => shot.id,
        None => panic!("line {line} has no shot {index}"),
    }
}

/// The station at chain index `index` of `line`.
pub(super) fn st(p: &Project, line: LineId, index: usize) -> StationId {
    super::super::test_helpers::at(p, line, index)
}

/// The name of station `s`.
pub(super) fn name(p: &Project, s: StationId) -> String {
    match p.station(s) {
        Some(station) => station.name.clone(),
        None => panic!("station {s} does not exist"),
    }
}

/// The names along `line`'s chain.
pub(super) fn names(p: &Project, line: LineId) -> Vec<String> {
    p.line_stations(line)
        .unwrap_or(&[])
        .iter()
        .map(|s| name(p, *s))
        .collect()
}

/// Horizontal position of `s` (must exist).
pub(super) fn xy(positions: &RawPositions, s: StationId) -> [f64; 2] {
    super::super::test_helpers::xy_of(positions, s)
}

/// Assert `a` and `b` agree within `tol` metres.
pub(super) fn near(a: [f64; 2], b: [f64; 2], tol: f64, what: &str) {
    super::super::test_helpers::assert_xy_near(a, b, tol, what);
}

/// Every station present in both states sits where it was (within `tol`),
/// except `except`.
pub(super) fn assert_still(
    before: &RawPositions,
    after: &RawPositions,
    tol: f64,
    except: &[StationId],
) {
    for (s, [x, y, _]) in after.iter() {
        if except.contains(&s) {
            continue;
        }
        if let Some(b) = before.xy(s) {
            near([x, y], b, tol, &format!("station {s}"));
        }
    }
}

/// The project must be valid.
pub(super) fn assert_valid(p: &Project) {
    let violations = p.validate();
    assert!(violations.is_empty(), "invalid model: {violations:?}");
}
