//! The conversion notice's words for each kind of difference the raw
//! comparison reports (review finding D-migration-upgrade-6): a move in plan,
//! a depth change (expected or not), a slot the conversion could not place.
//! Synthetic verifications only.

use super::open_file::ConversionNotice;
use crate::persistence::migrate::{Axis, ClassStats, ExpectedClass, Mismatch, Verification};

fn mismatch(station_index: u32, axis: Axis, displacement_m: f64) -> Mismatch {
    Mismatch {
        line_id: 0,
        station_index,
        axis,
        displacement_m,
        tolerance_m: 1e-9,
    }
}

fn message(v: &Verification) -> Option<String> {
    ConversionNotice::of(v).map(|n| n.message())
}

/// A depth-only difference is a depth change, never "0 stations moved by up
/// to 0.00 m". Injection: drop `depth_changed` from `ConversionNotice::of`
/// and the notice reads "0 stations moved".
#[test]
fn a_depth_change_is_reported_as_one() {
    let v = Verification {
        mismatches: vec![mismatch(1, Axis::Depth, 3.5)],
        ..Verification::default()
    };
    let text = message(&v).expect("a notice");
    assert!(
        text.contains("1 stations changed depth by up to 3.50 m"),
        "{text}"
    );
    assert!(!text.contains("stations moved"), "{text}");
}

/// A snap whose derived alignment changed a depth tie is an expected
/// difference, and still a visible one: it raises the notice (0.6.0 showed
/// those stations at another depth). Injection: leave `SnapAlignmentChanged`
/// out of `Verification::depth_changed` and no notice is raised.
#[test]
fn an_expected_depth_change_raises_the_notice() {
    let mut v = Verification::default();
    v.expected.insert(
        ExpectedClass::SnapAlignmentChanged,
        ClassStats {
            slots: 4,
            max_displacement_m: 0.75,
        },
    );
    let text = message(&v).expect("a notice");
    assert!(
        text.contains("4 stations changed depth by up to 0.75 m"),
        "{text}"
    );
}

/// A slot with no migrated position is "could not be placed", counted once
/// however many axes it fails, and never a move "by up to inf m".
#[test]
fn a_missing_slot_could_not_be_placed() {
    let v = Verification {
        mismatches: vec![
            mismatch(2, Axis::Missing, f64::INFINITY),
            mismatch(3, Axis::Horizontal, f64::NAN),
            mismatch(3, Axis::Depth, f64::NAN),
        ],
        ..Verification::default()
    };
    let text = message(&v).expect("a notice");
    assert!(text.contains("2 stations could not be placed"), "{text}");
    assert!(!text.contains("inf") && !text.contains("NaN"), "{text}");
    assert!(!text.contains("stations moved"), "{text}");
}

/// Moves, depth changes and unplaced slots are each said, in that order; a
/// torn merge collapsed counts as a move.
#[test]
fn every_kind_of_difference_is_said() {
    let mut v = Verification {
        mismatches: vec![
            mismatch(1, Axis::Horizontal, 2.0),
            mismatch(1, Axis::Depth, 0.5),
            mismatch(5, Axis::Missing, f64::INFINITY),
        ],
        ..Verification::default()
    };
    v.expected.insert(
        ExpectedClass::TornMergeCollapsed,
        ClassStats {
            slots: 2,
            max_displacement_m: 5.0,
        },
    );
    assert_eq!(
        message(&v).as_deref(),
        Some(
            "Converted from the old file format; 3 stations moved by up to 5.00 m; \
             1 stations changed depth by up to 0.50 m; 1 stations could not be placed. \
             The new format is written the first time the project is saved, or already \
             was if you renamed it in Recent Projects or removed its SpeleoDB link; the \
             original file is kept: to go back to it, right-click the project in Recent \
             Projects and choose Restore an earlier copy."
        )
    );
}

/// A clean conversion, and one whose only differences are in the Closures
/// Computed View (C4) or a non-finite legacy position, raise none.
#[test]
fn nothing_visible_raises_nothing() {
    assert_eq!(message(&Verification::default()), None);
    let mut v = Verification::default();
    v.expected.insert(
        ExpectedClass::LegacyNotFinite,
        ClassStats {
            slots: 1,
            max_displacement_m: 0.0,
        },
    );
    assert_eq!(message(&v), None);
}

/// A legacy SpeleoDB revision converted on download keeps no local original:
/// the cache is version 2 at once. Its notice says where the original is (the
/// server's revision), never the local backup and next save a local file's
/// notice promises. Injection: drop `.downloaded()` from the pull's install
/// (`prepare_download`) and the pull's notice promises a Restore that does
/// not exist (`a_pulled_legacy_revision_that_moved_stations_raises_the_
/// conversion_notice` covers that path end to end).
#[test]
fn a_downloaded_revision_points_at_the_server_not_a_local_backup() {
    let v = Verification {
        mismatches: vec![mismatch(1, Axis::Horizontal, 2.0)],
        ..Verification::default()
    };
    let local = ConversionNotice::of(&v).expect("a notice");
    let downloaded = local.clone().downloaded();
    let (local, downloaded) = (local.message(), downloaded.message());
    assert!(local.contains("Restore an earlier copy"), "{local}");
    assert!(
        local.contains("the first time the project is saved"),
        "{local}"
    );
    assert!(
        !downloaded.contains("Restore an earlier copy"),
        "{downloaded}"
    );
    assert!(!downloaded.contains("project is saved"), "{downloaded}");
    assert!(
        downloaded.ends_with(
            ". This computer's copy is already converted, and the original revision is \
             unchanged on SpeleoDB: it stays in the project's Revision history."
        ),
        "{downloaded}"
    );
    // The differences are said the same way: only the second sentence differs.
    let what = |m: &str| m.split(". ").next().unwrap_or_default().to_owned();
    assert_eq!(what(&local), what(&downloaded));
    assert_eq!(
        what(&local),
        "Converted from the old file format; 1 stations moved by up to 2.00 m"
    );
}
