//! Model-layer behaviour of the line north reference (#438).
//!
//! [`crate::azimuth_reference`] unit-tests the type in isolation. These are the
//! half of the contract that lives in this crate, on the station-graph model:
//! the coordinate walk, the line-vs-project resolution and the reverse. The half that spans crates - a station move, a split, save /
//! load, and the Ariane export flag - is in
//! `stalagmight-core::core::state::azimuth_reference_tests`, the lowest crate
//! that can see all of them.
//!
//! What each of these is trying to catch is a MISSING or MISDIRECTED correction
//! - a walk that forgot to add it, a write-back that forgot to remove it, a
//! persistence hop that dropped it - so every assertion below is written against
//! a line whose correction is large enough (10 degrees, ~1.7 m over 10 m)
//! that no tolerance can hide it.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: a failed unwrap is the failure report"
)]

use crate::survey::{LineMeta, Project, StartAt};
use crate::{AzimuthReference, CorrectionSource, LineId, NorthReference, ShotData};

/// 10 degrees east: big enough that a dropped correction cannot pass as noise.
const TEST_CORRECTION: f64 = 10.0;

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in: 0.0,
        depth_out: 0.0,
        ..Default::default()
    }
}

fn project_with(shots: Vec<ShotData>, reference: Option<AzimuthReference>) -> (Project, LineId) {
    let mut project = Project::new("declination", None);
    let shots = shots
        .into_iter()
        .enumerate()
        .map(|(i, mut data)| {
            data.uuid = Some(format!("shot-{i}"));
            data
        })
        .collect();
    let meta = LineMeta {
        name: Some("S".to_string()),
        color: "#ffffff".to_string(),
        ..LineMeta::default()
    };
    let id = project.add_line(meta, StartAt::New, shots).unwrap();
    project.line_mut(id).unwrap().azimuth_reference = reference;
    let start = project.line_stations(id).unwrap()[0];
    project.set_placement(start, 0.0, 0.0).unwrap();
    (project, id)
}

fn station_xy(project: &Project, line_id: LineId, index: usize) -> (f64, f64) {
    let station = project.line_stations(line_id).expect("line")[index];
    let [x, y] = project.raw_positions().xy(station).expect("station coords");
    (x, y)
}

/// The whole point of the feature: the shot keeps the measured number and the
/// GEOMETRY moves. Both halves are asserted, because a change that rotated the
/// stored reading instead would satisfy the coordinate half alone.
#[test]
fn the_correction_rotates_the_geometry_and_never_the_reading() {
    let reference = AzimuthReference::magnetic(TEST_CORRECTION, CorrectionSource::FileDeclared);
    let (project, id) = project_with(vec![shot(0.0, 10.0)], Some(reference));

    let stored = &project.line(id).unwrap().shots[0];
    assert_eq!(stored.azimuth_in, 0.0, "the measured reading must not move");
    assert_eq!(stored.azimuth_out, 0.0);

    let (x, y) = station_xy(&project, id, 1);
    let expected = TEST_CORRECTION.to_radians();
    assert!(
        (x - 10.0 * expected.sin()).abs() < 1e-9 && (y - 10.0 * expected.cos()).abs() < 1e-9,
        "station 1 should sit 10 degrees east of north, got ({x}, {y})"
    );
}

/// The no-reference case has to be BIT-identical, not merely close: it is what
/// every existing project computes, and this feature is only safe to land
/// because upgrading moves nobody.
#[test]
fn a_line_with_no_reference_walks_exactly_where_it_always_did() {
    let shots = vec![shot(37.5, 12.0), shot(191.25, 7.5), shot(400.0, 3.0)];
    let (with_none, id_a) = project_with(shots.clone(), None);
    let (with_default, id_b) = project_with(shots, Some(AzimuthReference::default()));

    for index in 0..4 {
        let a = station_xy(&with_none, id_a, index);
        let b = station_xy(&with_default, id_b, index);
        assert_eq!(
            a, b,
            "station {index}: an explicit zero reference must be the same walk as none at all"
        );
    }
    // `400.0` is deliberately out of range: the identity path must not silently
    // normalise it, because the pre-#438 walk did not.
    let (x, _) = station_xy(&with_none, id_a, 3);
    assert!(x.is_finite());
}

/// A line that declares its own reference must not read the project default,
/// and one that declares none must.
#[test]
fn the_line_reference_wins_over_the_project_default() {
    let (mut project, id) = project_with(
        vec![shot(0.0, 10.0)],
        Some(AzimuthReference::magnetic(
            TEST_CORRECTION,
            CorrectionSource::Manual,
        )),
    );
    project.settings.default_azimuth_reference =
        Some(AzimuthReference::magnetic(-45.0, CorrectionSource::Manual));

    let line = project.line(id).unwrap();
    assert_eq!(
        project.azimuth_reference_for(line).applied_correction(),
        TEST_CORRECTION
    );

    project.line_mut(id).unwrap().azimuth_reference = None;
    let line = project.line(id).unwrap();
    assert_eq!(
        project.azimuth_reference_for(line).applied_correction(),
        -45.0,
        "a line with no reference of its own inherits the project's"
    );
}

/// Reversing a line must not move the cave: station 0 becomes the far end,
/// and each reciprocated shot must still walk in the SAME rotated frame -
/// otherwise a line carrying a declination slides by the difference between
/// the two frames, which for a 100 m passage at 10 degrees is most of 17 m.
#[test]
fn reversing_a_corrected_line_keeps_every_station_where_it_was() {
    let reference = AzimuthReference::magnetic(TEST_CORRECTION, CorrectionSource::FileDeclared);
    let (mut project, id) = project_with(
        vec![shot(0.0, 40.0), shot(75.0, 30.0), shot(310.0, 30.0)],
        Some(reference),
    );
    let before: Vec<(f64, f64)> = (0..4).map(|i| station_xy(&project, id, i)).collect();

    project
        .reverse_line(id, false)
        .expect("reverse should succeed");

    let after: Vec<(f64, f64)> = (0..4).map(|i| station_xy(&project, id, i)).collect();
    for (index, expected) in before.iter().enumerate() {
        // The reverse renumbers, so station i is now station N - i.
        let got = after[before.len() - 1 - index];
        assert!(
            (got.0 - expected.0).abs() < 1e-6 && (got.1 - expected.1).abs() < 1e-6,
            "station {index} moved across the reverse: {expected:?} -> {got:?}"
        );
    }
}

/// Guards the enum's meaning rather than its spelling: `north` describes the
/// STORED numbers, so a magnetic line with a correction is still magnetic.
#[test]
fn a_corrected_magnetic_line_is_still_magnetic() {
    let reference = AzimuthReference::magnetic(TEST_CORRECTION, CorrectionSource::FileDeclared);
    assert_eq!(reference.north, NorthReference::Magnetic);
    assert!(!reference.is_identity());
}
