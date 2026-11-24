//! The conversion notice's words for each kind of difference the raw
//! comparison reports (review finding D-migration-upgrade-6): a move in plan,
//! a depth change (expected or not), a slot the conversion could not place;
//! and the banner's view of them (demo issue I-5): one short clause each with
//! a count in the right number, renamed stations as "Line / name", a long
//! list behind Details, and the file's state for the sentence on saving.
//! Synthetic verifications only.

use super::conversion_notice::{ConversionFile, ConversionNotice, ConversionSource, NAMES_INLINE};
use crate::models::prose;
use crate::models::{ProsePart, ProseStation};
use crate::persistence::migrate::{
    Axis, ClassStats, ClosureKeepsBoth, CommentHidden, ExpectedClass, IdentityChanges, Mismatch,
    NameChange, Verification,
};

/// RS-PROSE: the parts lists are index-aligned with the clauses, each says
/// its clause's words, and a clause that names nothing has none.
fn assert_parts_align(view: &super::ConversionNoticeView) {
    for (clauses, parts) in [
        (&view.changes, &view.change_parts),
        (&view.details, &view.detail_parts),
    ] {
        assert_eq!(clauses.len(), parts.len(), "{view:?}");
        for (clause, parts) in clauses.iter().zip(parts) {
            if !parts.is_empty() {
                assert_eq!(&prose::plain(parts), clause);
                assert!(parts.iter().any(ProsePart::is_name), "{parts:?}");
            }
            assert!(!clause.contains(prose::MARK_OPEN), "{clause:?}");
        }
    }
}

/// The names a clause's parts draw, by words.
fn named(parts: &[ProsePart]) -> Vec<&str> {
    parts
        .iter()
        .filter(|p| p.is_name())
        .map(ProsePart::text)
        .collect()
}

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

/// A slot now showing another station, `kept_line`'s `to`.
fn shown_as(line: &str, from: &str, kept_line: &str, to: &str) -> NameChange {
    NameChange {
        line: line.to_owned(),
        from: from.to_owned(),
        to: to.to_owned(),
        lettered: false,
        kept_line: Some(kept_line.to_owned()),
        ..NameChange::default()
    }
}

/// A slot whose own station now has a letter suffix.
fn lettered(line: &str, from: &str, to: &str) -> NameChange {
    NameChange {
        line: line.to_owned(),
        from: from.to_owned(),
        to: to.to_owned(),
        lettered: true,
        kept_line: None,
        ..NameChange::default()
    }
}

/// The change lists, with the lines in order of first appearance.
fn identity(names: Vec<NameChange>, hidden: &[(&str, &str)]) -> IdentityChanges {
    let comments_hidden: Vec<CommentHidden> = hidden
        .iter()
        .enumerate()
        .map(|(i, (line, station))| CommentHidden {
            line: (*line).to_owned(),
            line_id: 0,
            station: (*station).to_owned(),
            identity: u32::try_from(i).unwrap_or(u32::MAX),
        })
        .collect();
    let mut lines: Vec<String> = Vec::new();
    for line in names
        .iter()
        .map(|n| &n.line)
        .chain(comments_hidden.iter().map(|c| &c.line))
    {
        if !lines.contains(line) {
            lines.push(line.clone());
        }
    }
    IdentityChanges {
        names,
        comments_hidden,
        closures_keep_both: Vec::new(),
        lines,
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

/// A few changes are said one by one, the line named first ("North Passage /
/// 0", never "North Passage 0"), a station shown as another by that
/// station's line too; the banner then says Detach Line from Station or
/// Remove Closure brings them back. More than the banner lists are counted
/// by kind, and every one goes behind Details, grouped by line. Injection:
/// list every entry inline again and a 30-junction conversion is a wall of
/// text with no Details.
#[test]
fn changes_are_named_by_line_and_a_long_list_goes_behind_details() {
    let report = crate::persistence::migrate::MigrationReport::default();
    let few = identity(
        vec![
            shown_as("North Passage", "0", "Main", "3"),
            lettered("East Loop", "2", "2a"),
        ],
        &[("North Passage", "0")],
    );
    let view = ConversionNotice::of_migration(&report, &few)
        .expect("a notice")
        .view(false);
    assert_eq!(view.source, ConversionSource::OldFormat);
    assert_eq!(
        view.changes,
        [
            "North Passage / 0 is now shown as Main / 3",
            "the comment of North Passage / 0 is hidden",
            "East Loop / 2 is now shown as 2a",
        ]
    );
    // RS-PROSE: every station the clauses name is a part.
    assert_parts_align(&view);
    assert_eq!(
        view.change_parts
            .iter()
            .map(|p| named(p))
            .collect::<Vec<_>>(),
        [
            vec!["North Passage / 0", "Main / 3"],
            vec!["North Passage / 0"],
            vec!["East Loop / 2", "2a"],
        ]
    );
    assert!(view.details.is_empty());
    assert!(view.brings_back);

    let mut names: Vec<NameChange> = (1..=NAMES_INLINE + 2)
        .map(|i| shown_as(&format!("Branch {i}"), "0", "Main", &format!("{}", i * 19)))
        .collect();
    names.push(lettered("Branch 1", "19", "19a"));
    let mut many = identity(
        names,
        &[("Branch 2", "0"), ("Branch 3", "0"), ("Branch 4", "0")],
    );
    many.closures_keep_both.push(ClosureKeepsBoth {
        a: "Side / 17".to_owned(),
        b: "Main / 3".to_owned(),
        ..ClosureKeepsBoth::default()
    });
    let view = ConversionNotice::of_migration(&report, &many)
        .expect("a notice")
        .view(false);
    assert_eq!(
        view.changes,
        [
            format!(
                "{} stations are now shown under another station's name",
                NAMES_INLINE + 2
            ),
            "1 station now has a letter suffix".to_owned(),
            "3 stations' comments are now hidden".to_owned(),
            "1 loop closure keeps both names".to_owned(),
        ]
    );
    assert_eq!(view.details.len(), NAMES_INLINE + 2 + 1 + 3 + 1);
    assert_eq!(
        view.details[..3],
        [
            "Branch 1 / 0 is now shown as Main / 19",
            "Branch 1 / 19 is now shown as 19a",
            "Branch 2 / 0 is now shown as Main / 38",
        ],
        "grouped by line"
    );
    assert_eq!(
        view.details.last().map(String::as_str),
        Some("the loop closure between Side / 17 and Main / 3 keeps both names")
    );
    // The counts name nothing; a closure end without parts stays words.
    assert_parts_align(&view);
    assert!(view.change_parts.iter().all(Vec::is_empty));
    assert!(view.detail_parts.last().is_some_and(Vec::is_empty));
}

/// A closure end with its parts is drawn by them (RS-PROSE); words a part
/// does not say stay words.
#[test]
fn a_closure_that_keeps_both_names_draws_its_ends() {
    let side = ProsePart::Station(ProseStation {
        text: "Side / 17".to_owned(),
        line_id: Some(2),
        context: Some("Side".to_owned()),
        name: "17".to_owned(),
        home_line: None,
        loop_end: None,
        station_id: Some(17),
        slot: None,
    });
    let mut changes = IdentityChanges::default();
    changes.closures_keep_both.push(ClosureKeepsBoth {
        a: "Side / 17".to_owned(),
        b: "Main / 3".to_owned(),
        a_parts: vec![side.clone()],
        // Parts that say other words are not used.
        b_parts: vec![side],
    });
    let report = crate::persistence::migrate::MigrationReport::default();
    let view = ConversionNotice::of_migration(&report, &changes)
        .expect("a notice")
        .view(false);
    assert_eq!(
        view.changes,
        ["the loop closure between Side / 17 and Main / 3 keeps both names"]
    );
    assert_parts_align(&view);
    assert_eq!(named(&view.change_parts[0]), ["Side / 17"]);
}

/// Letters alone bring nothing back: the banner says nothing of Detach Line
/// from Station or Remove Closure.
#[test]
fn letters_alone_do_not_say_what_brings_them_back() {
    let report = crate::persistence::migrate::MigrationReport::default();
    let only_letters = identity(vec![lettered("Side", "2", "2a")], &[]);
    let view = ConversionNotice::of_migration(&report, &only_letters)
        .expect("a notice")
        .view(false);
    assert!(!view.brings_back);
}

/// An earlier version 2 file's upgrade raises the notice with its own lead,
/// and only when a line shows something differently.
#[test]
fn an_earlier_version_2_upgrade_has_its_own_lead() {
    let mut report = crate::persistence::stalx_v2::UpgradeReport::default();
    assert!(ConversionNotice::of_upgrade(&report).is_none());
    report.changes = identity(vec![shown_as("Main", "M2", "Side", "S0")], &[]);
    let view = ConversionNotice::of_upgrade(&report)
        .expect("a notice")
        .view(false);
    assert_eq!(view.source, ConversionSource::EarlierVersion2);
    assert_eq!(view.changes, ["Main / M2 is now shown as Side / S0"]);
    assert_eq!(view.file, ConversionFile::NotYetSaved);
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
    let changes = identity(vec![lettered("Side\u{202e}", "0", "0a")], &[]);
    let view = ConversionNotice::of_migration(&report, &changes)
        .expect("a notice")
        .view(false);
    assert_eq!(view.changes, ["Side / 0 is now shown as 0a"]);
}

/// The banner's lead is picked by the source's serde string, and the
/// frontend's map is keyed by the generated TS spelling: the two must be the
/// same string (RS-FX6, P2-3: serde's `snake_case` wrote
/// `earlier_version2`, ts-rs `earlier_version_2`, so the earlier version 2
/// banner had no lead). Injection: drop the variant's `serde(rename)`.
#[test]
fn the_source_serialises_as_its_typescript_literal() {
    use ts_rs::TS;
    let decl = ConversionSource::decl();
    for (source, literal) in [
        (ConversionSource::OldFormat, "old_format"),
        (ConversionSource::EarlierVersion2, "earlier_version_2"),
    ] {
        assert_eq!(
            serde_json::to_value(source).ok(),
            Some(serde_json::Value::String(literal.to_owned()))
        );
        assert!(
            decl.contains(&format!("\"{literal}\"")),
            "{literal} is not in {decl}"
        );
    }
}

/// One station on two lines hides one comment (RS-FX6, P3-9): the earlier
/// file's M4 on Main and Annex was counted and listed once per line. It is
/// counted once and listed once, with its lines. Injection: count
/// `comments_hidden.len()` again and the banner says "2 stations' comments".
#[test]
fn one_station_on_two_lines_hides_one_comment() {
    let report = crate::persistence::stalx_v2::UpgradeReport {
        changes: IdentityChanges {
            names: vec![
                shown_as("Main", "M2", "Side", "S0"),
                shown_as("Main", "M4", "Side", "S4"),
                shown_as("Annex", "M4", "Side", "S4"),
                lettered("Annex", "A1", "A1a"),
            ],
            comments_hidden: vec![
                CommentHidden {
                    line: "Main".to_owned(),
                    line_id: 1,
                    station: "M4".to_owned(),
                    identity: 4,
                },
                CommentHidden {
                    line: "Annex".to_owned(),
                    line_id: 2,
                    station: "M4".to_owned(),
                    identity: 4,
                },
            ],
            closures_keep_both: Vec::new(),
            lines: vec!["Main".to_owned(), "Annex".to_owned()],
        },
        ..Default::default()
    };
    let view = ConversionNotice::of_upgrade(&report)
        .expect("a notice")
        .view(false);
    assert!(
        view.changes
            .contains(&"1 station's comment is now hidden".to_owned()),
        "{:?}",
        view.changes
    );
    let hidden: Vec<&String> = view
        .details
        .iter()
        .filter(|d| d.starts_with("the comment of"))
        .collect();
    assert_eq!(hidden, ["the comment of M4 on Main and Annex is hidden"]);
    // RS-PROSE: the station and both lines are parts of that clause.
    let at = view
        .details
        .iter()
        .position(|d| d.starts_with("the comment of"))
        .expect("listed");
    let names: Vec<(&str, &str)> = view.detail_parts[at]
        .iter()
        .filter_map(|p| match p {
            ProsePart::Station(s) => Some(("station", s.text.as_str())),
            ProsePart::Line(l) => Some(("line", l.text.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        names,
        [("station", "M4"), ("line", "Main"), ("line", "Annex")]
    );
    assert_parts_align(&view);
    // Listed under the first line it is on, after that line's names.
    assert_eq!(
        view.details[..3],
        [
            "Main / M2 is now shown as Side / S0",
            "Main / M4 is now shown as Side / S4",
            "the comment of M4 on Main and Annex is hidden",
        ]
    );
}
