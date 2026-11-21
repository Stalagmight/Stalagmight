//! The conversion notice's words for each kind of difference the raw
//! comparison reports (review finding D-migration-upgrade-6): a move in plan,
//! a depth change (expected or not), a slot the conversion could not place;
//! and the banner's view of them (demo issue I-5): one short clause each with
//! a count in the right number, renamed stations as "Line / name", a long
//! list behind Details, and the file's state for the sentence on saving.
//! Synthetic verifications only.

use super::conversion_notice::{ConversionFile, ConversionNotice, NAMES_INLINE};
use crate::persistence::migrate::{
    Axis, ClassStats, ExpectedClass, Mismatch, NameChange, Verification,
};

fn mismatch(station_index: u32, axis: Axis, displacement_m: f64) -> Mismatch {
    Mismatch {
        line_id: 0,
        station_index,
        axis,
        displacement_m,
        tolerance_m: 1e-9,
    }
}

/// The clauses of a local file's notice before its first save.
fn changes(v: &Verification) -> Option<Vec<String>> {
    ConversionNotice::of(v).map(|n| n.view(false).changes)
}

fn moved(slots: usize, max_displacement_m: f64) -> Verification {
    let mut v = Verification::default();
    v.expected.insert(
        ExpectedClass::TornMergeCollapsed,
        ClassStats {
            slots,
            max_displacement_m,
        },
    );
    v
}

fn rename(line: &str, from: &str, to: &str) -> NameChange {
    NameChange {
        line: line.to_owned(),
        from: from.to_owned(),
        to: to.to_owned(),
    }
}

/// A depth-only difference is a depth change, never "0 stations moved by up
/// to 0.00 m", and one station reads in the singular. Injection: drop
/// `depth_changed` from `ConversionNotice::of` and the notice reads "0
/// stations moved".
#[test]
fn a_depth_change_is_reported_as_one() {
    let v = Verification {
        mismatches: vec![mismatch(1, Axis::Depth, 3.5)],
        ..Verification::default()
    };
    assert_eq!(
        changes(&v).expect("a notice"),
        ["1 station changed depth by 3.50 m"]
    );
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
    assert_eq!(
        changes(&v).expect("a notice"),
        ["4 stations changed depth by up to 0.75 m"]
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
    assert_eq!(
        changes(&v).expect("a notice"),
        ["2 stations could not be placed"]
    );
}

/// Moves, depth changes and unplaced slots are each said, in that order, each
/// in its own number; a torn merge collapsed counts as a move. Injection:
/// word a count as before ("{n} stations ...") and "1 stations" comes back.
#[test]
fn every_kind_of_difference_is_said_in_the_right_number() {
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
        changes(&v).expect("a notice"),
        [
            "3 stations moved by up to 5.00 m",
            "1 station changed depth by 0.50 m",
            "1 station could not be placed",
        ]
    );
    assert_eq!(
        changes(&moved(1, 0.4)).expect("a notice"),
        ["1 station moved by 0.40 m"]
    );
}

/// A clean conversion, and one whose only differences are in the Closures
/// Computed View (C4) or a non-finite legacy position, raise none.
#[test]
fn nothing_visible_raises_nothing() {
    assert_eq!(changes(&Verification::default()), None);
    let mut v = Verification::default();
    v.expected.insert(
        ExpectedClass::LegacyNotFinite,
        ClassStats {
            slots: 1,
            max_displacement_m: 0.0,
        },
    );
    assert_eq!(changes(&v), None);
}

/// A few renamed stations are said one by one, the line named first ("North
/// Passage / 0", never "North Passage 0"); more than the banner lists are
/// counted, and every one of them goes behind Details. Joined comments come
/// last. Injection: list every rename inline again and a 30-name conversion
/// is a wall of text with no Details.
#[test]
fn renamed_stations_are_named_by_line_and_a_long_list_goes_behind_details() {
    let report = crate::persistence::migrate::MigrationReport {
        comments_joined: 1,
        ..Default::default()
    };
    let few = vec![
        rename("North Passage", "0", "3"),
        rename("East Loop", "0", "2"),
    ];
    let view = ConversionNotice::of_migration(&report, &few)
        .expect("a notice")
        .view(false);
    assert_eq!(
        view.changes,
        [
            "North Passage / 0 is now shown as 3",
            "East Loop / 0 is now shown as 2",
            "a merged station's two comments were joined",
        ]
    );
    assert!(view.details.is_empty());

    let many: Vec<NameChange> = (1..=NAMES_INLINE + 2)
        .map(|i| rename(&format!("Branch {i}"), "0", &format!("{}", i * 19)))
        .collect();
    let report = crate::persistence::migrate::MigrationReport {
        comments_joined: 3,
        ..Default::default()
    };
    let view = ConversionNotice::of_migration(&report, &many)
        .expect("a notice")
        .view(false);
    assert_eq!(
        view.changes,
        [
            format!(
                "{} stations are now shown under another name",
                NAMES_INLINE + 2
            ),
            "the comments of 3 merged stations were joined".to_owned(),
        ]
    );
    assert_eq!(view.details.len(), NAMES_INLINE + 2);
    assert_eq!(view.details[0], "Branch 1 / 0 is now shown as 19");
}

/// A legacy SpeleoDB revision converted on download keeps no local original:
/// the cache is version 2 at once. Its notice says so (`Downloaded`), never
/// the local backup and next save a local file's notice promises; a local
/// file's notice says whether the new format is on disk yet. Injection: drop
/// `.downloaded()` from the pull's install (`prepare_download`) and the
/// pull's notice promises a Restore that does not exist
/// (`a_pulled_legacy_revision_that_moved_stations_raises_the_conversion_
/// notice` covers that path end to end).
#[test]
fn the_file_state_says_where_the_original_is() {
    let local = ConversionNotice::of(&moved(1, 2.0)).expect("a notice");
    let downloaded = local.clone().downloaded();
    assert_eq!(local.view(false).file, ConversionFile::NotYetSaved);
    assert_eq!(local.view(true).file, ConversionFile::Saved);
    assert_eq!(downloaded.view(false).file, ConversionFile::Downloaded);
    assert_eq!(downloaded.view(true).file, ConversionFile::Downloaded);
    // The differences are said the same way: only the file state differs.
    assert_eq!(local.view(false).changes, downloaded.view(false).changes);
}

/// Station and line names are file text: shown without bidi controls, like
/// every receipt string.
#[test]
fn a_rename_shows_no_bidi_control() {
    let report = crate::persistence::migrate::MigrationReport::default();
    let view = ConversionNotice::of_migration(&report, &[rename("Side\u{202e}", "0", "3")])
        .expect("a notice")
        .view(false);
    assert_eq!(view.changes, ["Side / 0 is now shown as 3"]);
}
