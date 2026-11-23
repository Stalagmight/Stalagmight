//! Package RS-FX5 (plan 2026-11-23, kept and archived stations): the user's
//! decision of 2026-11-23, "dotted slots don't collide". A slot that shows
//! another station's identity counts for no name uniqueness on its line: a
//! foreign slot (its line is named by a record, so it shows the kept station
//! with the kept station's home-line dot) and a loop end (a closure archives
//! its node, so it shows the kept end with the loop mark). Only native slots
//! must have distinct names (N8). Fully synthetic data.

use crate::StationRef;

use super::archive_tests::shown;
use super::ops_test_helpers::{assert_valid, free_line, ok, run};
use super::test_helpers::{at, project};
use super::{ClosureStrategy, LineId, NameConflict, Project, Violation};

const BATCH: ClosureStrategy = ClosureStrategy::Batch;

/// Give each station of `line` the name of its index.
fn number_line(p: &mut Project, line: LineId) {
    let chain = p.line_stations(line).unwrap().to_vec();
    for (i, s) in chain.into_iter().enumerate() {
        p.station_mut(s).unwrap().name = i.to_string();
    }
}

/// The user's example: Main (stations 0 to 14) and Branch (stations 0 to
/// 12, its own numbering), Branch's start merged onto Main's 12, which is
/// kept. Returns the project before the merge, Main, Branch.
fn main_and_branch() -> (Project, LineId, LineId) {
    let mut p = project();
    let main = free_line(&mut p, [0.0, 0.0], run(90.0, 14, 10.0));
    let branch_line = free_line(&mut p, [400.0, 300.0], run(0.0, 12, 5.0));
    number_line(&mut p, main);
    number_line(&mut p, branch_line);
    assert_valid(&p);
    (p, main, branch_line)
}

/// The decision's own example. Main / 12 is kept; Branch's start shows "12"
/// with Main's dot, and Branch's own 13th station keeps its surveyed name
/// "12". No letter is given, the model is valid, and both "12"s are shown
/// on Branch (one foreign, one native). Failing first: before RS-FX5 the
/// merge with the dialog's empty answers refused (`NameTaken` on Branch)
/// and rule L lettered Branch's own 12 as "12a".
#[test]
fn the_users_example_letters_nothing_and_is_valid() {
    let (mut p, main, br) = main_and_branch();
    let (m12, b0, b12) = (at(&p, main, 12), at(&p, br, 0), at(&p, br, 12));
    assert_eq!(p.stations[&b12].name, "12");
    let preview = p.merge_preview(b0, m12, m12).unwrap();
    assert!(preview.collisions.is_empty(), "{:?}", preview.collisions);
    let out = ok(p.merge_stations(b0, m12, m12, &[]));
    assert!(out.identities.lettered.is_empty());
    assert_valid(&p);
    let names = shown(&p, br);
    assert_eq!((names[0].as_str(), names[12].as_str()), ("12", "12"));
    assert_eq!(
        p.stations[&b12].name, "12",
        "Branch's own 12 keeps its name"
    );
    // One foreign (Main's dot), one native.
    let start = StationRef::new(br, 0);
    let own = StationRef::new(br, 12);
    assert!(p.is_foreign_slot(start) && !p.is_native_slot(start));
    assert!(p.is_native_slot(own));
    assert_eq!(p.home_line(m12), Some(main));
    assert_eq!(p.station_at(start), Some(m12));
    assert_eq!(p.station_at(own), Some(b12));
    assert!(p.name_collisions().is_empty());

    // Without a dialog, nothing is lettered either.
    let (mut auto, ..) = main_and_branch();
    let out = ok(auto.merge_stations_auto(b0, m12, m12));
    assert!(out.identities.lettered.is_empty());
    assert_eq!(auto, p);
}

/// Two foreign slots with the same name on one line are valid: X's start is
/// merged onto Main's 12 and X's end onto Side's 12, both kept, so X shows
/// two dotted "12"s (Main's dot and Side's dot).
#[test]
fn two_foreign_slots_with_one_name_on_a_line_are_valid() {
    let mut p = project();
    let main = free_line(&mut p, [0.0, 0.0], run(90.0, 14, 10.0));
    let side = free_line(&mut p, [0.0, 500.0], run(90.0, 14, 10.0));
    let x = free_line(&mut p, [700.0, -300.0], run(0.0, 3, 5.0));
    number_line(&mut p, main);
    number_line(&mut p, side);
    number_line(&mut p, x);
    let (m12, s12) = (at(&p, main, 12), at(&p, side, 12));
    let (x0, x3) = (at(&p, x, 0), at(&p, x, 3));
    ok(p.merge_stations(x0, m12, m12, &[]));
    let out = ok(p.merge_stations(x3, s12, s12, &[]));
    assert!(out.identities.lettered.is_empty());
    assert_valid(&p);
    assert_eq!(shown(&p, x), ["12", "1", "2", "12"]);
    assert!(p.is_foreign_slot(StationRef::new(x, 0)));
    assert!(p.is_foreign_slot(StationRef::new(x, 3)));
    // And a native third "12" on X stays valid beside both.
    let x1 = at(&p, x, 1);
    p.station_mut(x1).unwrap().name = "12".to_string();
    assert_valid(&p);
    // A second native "12" on X is a collision (N8 on native slots).
    let x2 = at(&p, x, 2);
    p.station_mut(x2).unwrap().name = "12".to_string();
    assert_eq!(
        p.validate(),
        vec![Violation::DuplicateStationName {
            line: x,
            name: "12".to_string(),
            first: x1,
            second: x2,
        }]
    );
}

/// A foreign slot and a loop end with one name on a line are valid, and so
/// is a loop end beside a native station of its name: Branch shows Main's
/// 12 at its start (dotted), its end is archived behind Main's 12 by a loop
/// closure (the loop mark), and its own 12 keeps its name.
#[test]
fn a_foreign_slot_a_loop_end_and_a_native_station_share_a_name() {
    let (mut p, main, br) = main_and_branch();
    let (m12, m13, b0) = (at(&p, main, 12), at(&p, main, 13), at(&p, br, 0));
    ok(p.merge_stations(b0, m12, m12, &[]));
    let b_end = at(&p, br, 12);
    // Archive Branch's 6 behind Main's 12 (a same-line closure on Branch,
    // whose start is Main's 12).
    let b6 = at(&p, br, 6);
    let naming = p
        .closure_naming((b6, Some(br)), (m12, Some(main)), BATCH, Some(m12))
        .unwrap();
    assert!(naming.collisions.is_empty());
    let out = ok(p.create_closure(b6, Some(br), m12, Some(main), BATCH, Some(m12), &[]));
    assert!(out.identities.lettered.is_empty());
    assert_valid(&p);
    let names = shown(&p, br);
    assert_eq!(
        (names[0].as_str(), names[6].as_str(), names[12].as_str()),
        ("12", "12", "12")
    );
    assert!(p.is_foreign_slot(StationRef::new(br, 0)));
    assert!(p.is_closure_archived(b6), "a loop end");
    assert!(p.is_native_slot(StationRef::new(br, 12)));
    assert_eq!(p.station_at(StationRef::new(br, 12)), Some(b_end));
    // Main's 13 is untouched.
    assert_eq!(p.stations[&m13].name, "13");
}

/// Point 3 of RS-FX5: a rename of the kept station, or one through a
/// foreign slot (which addresses the kept station), is checked only on the
/// lines where that identity is native. Renaming Main's 12 to "Sump" is
/// free although Branch shows "Sump" natively (Branch shows Main's 12 only
/// through its dotted start); "11" is taken on Main.
#[test]
fn a_rename_through_a_foreign_slot_is_checked_on_the_kept_stations_lines() {
    let (mut p, main, br) = main_and_branch();
    let (m11, m12, b0) = (at(&p, main, 11), at(&p, main, 12), at(&p, br, 0));
    ok(p.merge_stations(b0, m12, m12, &[]));
    let start = StationRef::new(br, 0);
    let through_branch = p.station_at(start).unwrap();
    assert_eq!(through_branch, m12);
    assert_eq!(p.native_lines_of_identity(m12), vec![main]);
    let b4 = at(&p, br, 4);
    p.station_mut(b4).unwrap().name = "Sump".to_string();
    assert!(p.conflicts_with_name(through_branch, "Sump").is_empty());
    assert!(p.conflicts_with_name(m12, "Sump").is_empty());
    assert_eq!(
        p.conflicts_with_name(through_branch, "11"),
        vec![NameConflict {
            line: main,
            other: m11
        }]
    );
    // A rename of a Branch station to "12" is free too: Branch's 12 that
    // matters is its own, at its end, so its 3 must not become 12, but its
    // dotted start does not count.
    let b3 = at(&p, br, 3);
    let b12 = at(&p, br, 12);
    assert_eq!(
        p.conflicts_with_name(b3, "12"),
        vec![NameConflict {
            line: br,
            other: b12
        }]
    );
    p.station_mut(b12).unwrap().name = "end".to_string();
    assert!(p.conflicts_with_name(b3, "12").is_empty());
}

/// Native collisions still letter: Detach after the merge gives Branch its
/// own station 0 back natively, and Branch's station 5 renamed "0" since
/// repeats it. The preview lists the collision (rule L letters the station
/// new on the line, the restored 0, being a tie on lines), the dialog's
/// empty answer is refused, and the form without a dialog letters it.
#[test]
fn a_native_collision_a_detach_makes_still_letters() {
    let (mut p, main, br) = main_and_branch();
    let (m12, b0, b5) = (at(&p, main, 12), at(&p, br, 0), at(&p, br, 5));
    ok(p.merge_stations(b0, m12, m12, &[]));
    p.station_mut(b5).unwrap().name = "0".to_string();
    assert_valid(&p);
    let naming = p.detach_naming(m12, br, None).unwrap();
    assert_eq!(naming.collisions.len(), 1, "{naming:?}");
    let letter = &naming.collisions[0];
    assert_eq!((letter.station, letter.other), (b0, b5));
    assert_eq!((letter.from.as_str(), letter.to.as_str()), ("0", "0a"));
    assert!(p.clone().detach_line(m12, br, &[], None).is_err());
    let out = ok(p.detach_line_auto(m12, br, None));
    assert_eq!(out.identities.lettered.len(), 1);
    assert_valid(&p);
    assert_eq!(shown(&p, br)[0], "0a");
    assert_eq!(shown(&p, br)[5], "0");
}

/// Rule L's letter is free among the native slots of the lettered station's
/// lines: a dotted "5a" on Branch does not block it, as it would not block
/// a rename to "5a" (RS-FX5 decides one rule for both).
#[test]
fn a_letter_is_free_among_native_slots_only() {
    let mut p = project();
    let main = free_line(&mut p, [0.0, 0.0], run(90.0, 6, 10.0));
    let br = free_line(&mut p, [300.0, 300.0], run(0.0, 6, 5.0));
    number_line(&mut p, main);
    number_line(&mut p, br);
    let (m5, b0) = (at(&p, main, 5), at(&p, br, 0));
    p.station_mut(m5).unwrap().name = "5a".to_string();
    ok(p.merge_stations(b0, m5, m5, &[]));
    assert_eq!(shown(&p, br)[0], "5a", "dotted");
    // Branch's 2 renamed 5 repeats Branch's own 5: both are on one line and
    // were on it before, so the lower Station ID keeps the name.
    let (b2, b5) = (at(&p, br, 2), at(&p, br, 5));
    let before = p.clone();
    p.station_mut(b2).unwrap().name = "5".to_string();
    let letters = p.letter_collisions(&before);
    assert_eq!(letters.len(), 1);
    let lettered = letters[0].station;
    assert_eq!(lettered, b2.max(b5));
    assert_eq!(
        p.stations[&lettered].name, "5a",
        "the dotted 5a does not block it"
    );
    assert_valid(&p);
    let names = shown(&p, br);
    assert_eq!(names.iter().filter(|n| *n == "5a").count(), 2);
}

/// Point 4 of RS-FX5: plain text that names a slot without the dot or the
/// loop mark qualifies a dotted slot by the kept station's home line and a
/// loop end by the line it sits on, so Branch's two "12"s read apart
/// (`slot_text.rs`; the frontend's `stationLabelOf` words them the same).
#[test]
fn plain_text_tells_the_two_12s_on_a_line_apart() {
    let (mut p, main, br) = main_and_branch();
    let (m12, b0) = (at(&p, main, 12), at(&p, br, 0));
    ok(p.merge_stations(b0, m12, m12, &[]));
    let b6 = at(&p, br, 6);
    ok(p.create_closure(b6, Some(br), m12, Some(main), BATCH, Some(m12), &[]));
    let slot = |line, i| StationRef::new(line, i);
    let (main_label, br_label) = (p.line_label_of(main), p.line_label_of(br));
    assert_eq!(p.slot_name_text(slot(br, 12)), "12", "native");
    assert_eq!(p.slot_name_text(slot(br, 0)), format!("12 ({main_label})"));
    assert_eq!(
        p.slot_name_text(slot(br, 6)),
        format!("12 (loop end on {br_label})")
    );
    assert_eq!(p.slot_name_text(slot(main, 12)), "12", "its own line");
    assert_eq!(
        p.slot_text(slot(br, 12)).as_deref(),
        Some(format!("{br_label} / 12").as_str())
    );
    assert_eq!(
        p.slot_text(slot(br, 0)).as_deref(),
        Some(format!("{br_label} / 12 ({main_label})").as_str())
    );
    assert_eq!(
        p.slot_text(slot(br, 6)).as_deref(),
        Some(format!("{main_label} / 12 (loop end on {br_label})").as_str())
    );
    assert_eq!(
        p.shot_text(br, 0),
        format!("{br_label}: 12 ({main_label}) \u{2192} 1")
    );
    assert_eq!(p.shot_text(br, 11), format!("{br_label}: 11 \u{2192} 12"));
    assert_eq!(
        p.station_text(m12, Some(br)),
        format!("{br_label} / 12 ({main_label})")
    );
    assert_eq!(
        p.station_text(m12, Some(main)),
        format!("{main_label} / 12")
    );
    assert_eq!(p.slot_name_text(slot(br, 99)), "99", "no such slot");
    assert_eq!(p.slot_text(slot(br, 99)), None);
}

/// RS-PROSE: each plain form has a part form with the same words, and the
/// part says what the pill draws: the slot's line (its colour), the line as
/// context when the text names it, the bare name, a foreign slot's home
/// line (the dot), a loop end (the loop mark), the Station ID it shows and
/// the slot. A marked text splits into the plain string and its parts.
#[test]
fn prose_parts_word_a_slot_as_its_plain_text_does() {
    use crate::prose::{self, ProsePart};

    let (mut p, main, br) = main_and_branch();
    let (m12, b0) = (at(&p, main, 12), at(&p, br, 0));
    ok(p.merge_stations(b0, m12, m12, &[]));
    let b6 = at(&p, br, 6);
    ok(p.create_closure(b6, Some(br), m12, Some(main), BATCH, Some(m12), &[]));
    let slot = |line, i| StationRef::new(line, i);
    let station = |part: Option<ProsePart>| match part {
        Some(ProsePart::Station(s)) => s,
        other => panic!("a station part, not {other:?}"),
    };
    for (s, with_line) in [
        (slot(br, 12), false),
        (slot(br, 0), false),
        (slot(br, 6), false),
        (slot(br, 12), true),
        (slot(br, 0), true),
        (slot(br, 6), true),
    ] {
        let part = station(p.slot_part(s, with_line));
        let words = if with_line {
            p.slot_text(s).unwrap()
        } else {
            p.slot_name_text(s)
        };
        assert_eq!(part.text, words, "{s:?} {with_line}");
        assert_eq!(part.name, "12");
        assert_eq!(part.line_id, Some(br));
        assert_eq!(part.slot, Some(s));
        assert_eq!(part.context.is_some(), with_line);
    }
    let foreign = station(p.slot_part(slot(br, 0), true));
    assert_eq!(foreign.home_line, Some(main));
    assert_eq!(foreign.station_id, Some(m12));
    assert_eq!(foreign.loop_end, None);
    let loop_end = station(p.slot_part(slot(br, 6), false));
    assert_eq!(loop_end.loop_end, Some(true));
    assert_eq!(loop_end.home_line, None);
    assert_eq!(loop_end.station_id, Some(m12), "it shows Main's 12");
    assert_eq!(p.slot_part(slot(br, 99), true), None);

    let ProsePart::Shot(shot) = p.shot_part(br, 0) else {
        panic!("a shot part");
    };
    assert_eq!(shot.text, p.shot_text(br, 0));
    assert_eq!(shot.from.home_line, Some(main));
    assert_eq!(
        (shot.from.name.as_str(), shot.to.name.as_str()),
        ("12", "1")
    );
    assert_eq!(shot.context, Some(p.line_label_of(br)));

    let marked = format!(
        "Detach line {} from {} at {}",
        p.line_marked(br, true),
        p.station_marked(m12, Some(br)),
        p.shot_marked(br, 0)
    );
    let (text, parts) = prose::split(&marked);
    assert_eq!(
        text,
        format!(
            "Detach line \"{}\" from {} at {}",
            p.line_label_of(br),
            p.station_text(m12, Some(br)),
            p.shot_text(br, 0)
        )
    );
    assert_eq!(parts.iter().filter(|part| part.is_name()).count(), 3);
    let mut texts = super::SlotTexts::new(&p);
    assert_eq!(
        texts.station_marked(m12, Some(br)),
        p.station_marked(m12, Some(br))
    );
    assert_eq!(texts.shot_marked(br, 0), p.shot_marked(br, 0));
}
