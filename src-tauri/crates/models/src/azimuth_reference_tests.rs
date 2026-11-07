//! Model-layer behaviour of the section north reference (#438).
//!
//! [`crate::azimuth_reference`] unit-tests the type in isolation. These are the
//! half of the contract that lives in this crate: the coordinate walk, the
//! section-vs-project resolution, the reverse's offset compensation and the
//! undo identity. The half that spans crates - a station move, a split, save /
//! load, and the Ariane export flag - is in
//! `stalagmight-core::core::state::azimuth_reference_tests`, the lowest crate
//! that can see all of them.
//!
//! What each of these is trying to catch is a MISSING or MISDIRECTED correction
//! - a walk that forgot to add it, a write-back that forgot to remove it, a
//! persistence hop that dropped it - so every assertion below is written against
//! a section whose correction is large enough (10 degrees, ~1.7 m over 10 m)
//! that no tolerance can hide it.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test module: a failed unwrap is the failure report"
)]

use crate::test_fixtures::straight_shot;
use crate::{
    AzimuthReference, CorrectionSource, NorthReference, Project, Section, SectionData, SectionId,
    ShotData, StationRef,
};

/// 10 degrees east: big enough that a dropped correction cannot pass as noise.
const TEST_CORRECTION: f64 = 10.0;

fn project_with(shots: Vec<ShotData>, reference: Option<AzimuthReference>) -> (Project, SectionId) {
    let mut project = Project::new("declination".to_string(), None);
    let id = project
        .add_section(SectionData {
            name: Some("S".to_string()),
            surveyor: None,
            date: None,
            color: "#ffffff".to_string(),
            direction: None,
            quality: 1.0,
            shots,
        })
        .id;
    project.get_section_mut(id).unwrap().azimuth_reference = reference;
    project.ensure_shot_uuids();
    (project, id)
}

fn station_xy(project: &Project, section_id: SectionId, index: u32) -> (f64, f64) {
    let coords = project
        .compute_station_raw_coords(StationRef::new(section_id, index))
        .expect("station coords");
    (coords.x, coords.y)
}

/// The whole point of the feature: the shot keeps the measured number and the
/// GEOMETRY moves. Both halves are asserted, because a change that rotated the
/// stored reading instead would satisfy the coordinate half alone.
#[test]
fn the_correction_rotates_the_geometry_and_never_the_reading() {
    let reference = AzimuthReference::magnetic(TEST_CORRECTION, CorrectionSource::FileDeclared);
    let (project, id) = project_with(vec![straight_shot(0.0, 10.0)], Some(reference));

    let stored = &project.get_section(id).unwrap().shots[0];
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
fn a_section_with_no_reference_walks_exactly_where_it_always_did() {
    let shots = vec![
        straight_shot(37.5, 12.0),
        straight_shot(191.25, 7.5),
        straight_shot(400.0, 3.0),
    ];
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

/// A section that declares its own reference must not read the project default,
/// and one that declares none must.
#[test]
fn the_section_reference_wins_over_the_project_default() {
    let (mut project, id) = project_with(
        vec![straight_shot(0.0, 10.0)],
        Some(AzimuthReference::magnetic(
            TEST_CORRECTION,
            CorrectionSource::Manual,
        )),
    );
    project.default_azimuth_reference =
        Some(AzimuthReference::magnetic(-45.0, CorrectionSource::Manual));

    let section = project.get_section(id).unwrap();
    assert_eq!(
        project.azimuth_reference_for(section).applied_correction(),
        TEST_CORRECTION
    );

    project.get_section_mut(id).unwrap().azimuth_reference = None;
    let section = project.get_section(id).unwrap();
    assert_eq!(
        project.azimuth_reference_for(section).applied_correction(),
        -45.0,
        "a section with no reference of its own inherits the project's"
    );
}

/// Reversing a section must not move the cave: station 0 becomes the far end
/// and the section offset gains the span it used to cover. That span is measured
/// by `shot_run_displacement`, which has to work in the SAME rotated frame as
/// the walk - otherwise a section carrying a declination slides by the
/// difference between the two frames, which for a 100 m passage at 10 degrees is
/// most of 17 m.
#[test]
fn reversing_a_corrected_section_keeps_every_station_where_it_was() {
    let reference = AzimuthReference::magnetic(TEST_CORRECTION, CorrectionSource::FileDeclared);
    let (mut project, id) = project_with(
        vec![
            straight_shot(0.0, 40.0),
            straight_shot(75.0, 30.0),
            straight_shot(310.0, 30.0),
        ],
        Some(reference),
    );
    let before: Vec<(f64, f64)> = (0..4).map(|i| station_xy(&project, id, i)).collect();

    assert!(project.reverse_section(id, false), "reverse should succeed");

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

/// `SectionData` cannot carry the reference, so an undo that re-creates a
/// section restores it from [`crate::SectionIdentity`]. Losing it
/// rotates that section by the declination on the next redraw.
#[test]
fn restoring_a_section_from_its_identity_keeps_the_reference() {
    let reference = AzimuthReference::grid(-3.25, CorrectionSource::IgrfAuto { epoch_year: 2019 });
    let (project, id) = project_with(vec![straight_shot(0.0, 10.0)], Some(reference));
    let section = project.get_section(id).unwrap();

    let identity = crate::SectionIdentity::of(section);
    let restored = Section::restore_with_identity(id, section.to_section_data(), &identity);

    assert_eq!(
        restored.azimuth_reference,
        Some(reference),
        "the restored section lost its north reference"
    );
    // A legacy entry (no recorded reference) must still restore, inheriting the
    // project default - the pre-#438 behaviour, not a panic or a zero override.
    let legacy = crate::SectionIdentity::default();
    let restored = Section::restore_with_identity(id, section.to_section_data(), &legacy);
    assert_eq!(restored.azimuth_reference, None);
}

/// Guards the enum's meaning rather than its spelling: `north` describes the
/// STORED numbers, so a magnetic section with a correction is still magnetic.
#[test]
fn a_corrected_magnetic_section_is_still_magnetic() {
    let reference = AzimuthReference::magnetic(TEST_CORRECTION, CorrectionSource::FileDeclared);
    assert_eq!(reference.north, NorthReference::Magnetic);
    assert!(!reference.is_identity());
}
