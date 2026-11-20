//! Forward compatibility: what a file a newer Stalagmight wrote does here
//! (plan 6.4, `docs/file-format.md` sections 3, 7 and 11). Each case is a
//! review finding of the format, pinned.

use std::fmt::Write as _;

use crate::models::survey::{LineMeta, Project, StartAt};
use crate::models::{Section, ShotData};

use super::format_v2_doc_tests::example;
use super::test_helpers::text_of;
use super::{
    decode, encode, rewrite_local, CountLimits, DecodeNote, DecodeOptions, Decoded, Extras, Mode,
    RawJson,
};

fn open(text: &str) -> Decoded {
    decode(text.as_bytes(), &DecodeOptions::default()).unwrap()
}

/// One line of `shots` one-metre shots, and `sections` unused Sections
/// coloured `#123456`.
fn survey(shots: usize, sections: u32) -> Project {
    let mut p = Project::new_at(
        "Forward",
        None,
        chrono::DateTime::from_timestamp(0, 0).unwrap(),
    );
    let data = (0..shots)
        .map(|i| ShotData {
            uuid: Some(format!("u{i}")),
            length: 1.0,
            ..ShotData::default()
        })
        .collect();
    let line = p.add_line(LineMeta::default(), StartAt::New, data).unwrap();
    let start = p.line(line).unwrap().start;
    p.set_placement(start, 0.0, 0.0).unwrap();
    p.sections = (0..sections)
        .map(|id| Section {
            id,
            name: format!("S{id}"),
            color: "#123456".to_string(),
        })
        .collect();
    p.counters.next_section_id = sections;
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    p
}

/// `text` with `extra` as the extras object of every shot row (rows that
/// have none).
fn with_row_extras(text: &str, extra: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for line in text.lines() {
        if line.starts_with("[\"") {
            let (row, comma) = line.strip_suffix(',').map_or((line, ""), |row| (row, ","));
            let cells = row.strip_suffix(']').unwrap();
            let _ = write!(out, "{cells},{extra}]{comma}");
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Tier-1 unknown keys are never held, so they never exhaust the
/// preserved-content cap: a newer writer that adds one key to every shot, or
/// to every Section, gives a file that opens read-only however large it is,
/// never one refused as too complex (review finding C-file-format-1).
/// Injection: counting `skip_unknown` or `Fields::unknown_to` against
/// `Limit::Preserved` again refuses both files with `TooComplex`.
#[test]
fn a_new_key_on_every_record_opens_read_only_at_any_size() {
    let over = CountLimits::default().preserved + 1;
    let base =
        text_of(&encode(&survey(over, 3), &Extras::default(), Mode::Normal).unwrap()).to_string();
    let shots = with_row_extras(&base, r#"{"zz_future":1}"#);
    let decoded = open(&shots);
    let reason = decoded.read_only().expect("read-only, not refused");
    assert_eq!(reason.unknown_content_total, over);
    assert_eq!(decoded.project.lines()[0].shots.len(), over);

    let base = text_of(
        &encode(
            &survey(2, u32::try_from(over).unwrap()),
            &Extras::default(),
            Mode::Normal,
        )
        .unwrap(),
    )
    .to_string();
    let sections = base.replace("\"#123456\"}", "\"#123456\",\"zz_future\":1}");
    let decoded = open(&sections);
    let reason = decoded.read_only().expect("read-only, not refused");
    assert_eq!(reason.unknown_content_total, over);
    assert_eq!(decoded.project.sections.len(), over);
}

/// Unknown keys of `local` and `local.speleodb` (a newer build's per-machine
/// and sync state) survive a local-only rewrite and a normal save, in their
/// canonical place after the known keys; a detached copy and an upload, which
/// carry no per-machine state, leave them out (review finding
/// H-frontend-wire-2). Injection: dropping an unknown key in `read_local`, or
/// not writing `preserved` in `write_local`, loses them on the toggle.
#[test]
fn unknown_local_keys_survive_every_rewrite_of_this_machine() {
    let (p, extras) = example();
    let text = text_of(&encode(&p, &extras, Mode::Normal).unwrap()).to_string();
    let newer = text
        .replacen(
            "\"local\":{",
            "\"local\":{\"zz_machine\":{\"keep\":[1,2.50]},",
            1,
        )
        .replacen("\"last_edit\":", "\"zz_sync\":true,\"last_edit\":", 1);
    assert_ne!(newer, text);
    let decoded = open(&newer);
    assert!(decoded.read_only().is_none());
    assert!(
        !decoded
            .notes
            .iter()
            .any(|n| matches!(n, DecodeNote::LocalDropped { .. })),
        "{:?}",
        decoded.notes
    );
    assert_eq!(
        decoded.extras.local.preserved,
        vec![(
            "zz_machine".to_string(),
            RawJson::parse(r#"{"keep":[1,2.50]}"#).unwrap()
        )]
    );
    assert_eq!(
        decoded.extras.local.speleodb.preserved,
        vec![("zz_sync".to_string(), RawJson::parse("true").unwrap())]
    );

    // A display toggle: a local-only rewrite.
    let mut toggled = decoded.extras.clone();
    toggled
        .local
        .display
        .insert("grid".to_string(), "false".to_string());
    let rewritten = text_of(&rewrite_local(newer.as_bytes(), &toggled).unwrap()).to_string();
    assert!(
        rewritten.contains(
            "\"local\":{\"display\":{\"grid\":\"false\"},\"speleodb\":{\"base\":\
             \"3a7bd3e2360a3d29eea436fcfb7e44c735d117c4\",\"unpushed\":true,\
             \"last_edit\":\"2025-11-20T13:00:00.000000000Z\",\"zz_sync\":true},\
             \"zz_machine\":{\"keep\":[1,2.50]}},\n"
        ),
        "{rewritten}"
    );

    // A normal save keeps them, and is a fixpoint.
    let saved = encode(&decoded.project, &decoded.extras, Mode::Normal).unwrap();
    let again = decode(&saved, &DecodeOptions::default()).unwrap();
    assert_eq!(again.extras.local, decoded.extras.local);
    assert_eq!(
        encode(&again.project, &again.extras, Mode::Normal).unwrap(),
        saved
    );

    // No per-machine state leaves the machine.
    for mode in [
        Mode::DetachedCopy,
        Mode::ForPush(super::PushOptions::default()),
    ] {
        let out = encode(&decoded.project, &decoded.extras, mode).unwrap();
        assert!(!text_of(&out).contains("zz_"), "{mode:?}");
    }
}

/// The documented example with a station on no line (an invariant this
/// version holds, which a future feature could relax).
fn example_with_an_off_line_station() -> String {
    let (p, extras) = example();
    let text = text_of(&encode(&p, &extras, Mode::Normal).unwrap()).to_string();
    let edited = text
        .replacen("\"station\":8,", "\"station\":10,", 1)
        .replacen("[6,\"C1\"]\n", "[6,\"C1\"],\n[9,\"Z9\"]\n", 1);
    assert_eq!(edited.matches("Z9").count(), 1);
    edited
}

/// A file that requires a feature this build does not know is read as far as
/// this build can; when its survey then breaks an invariant, the refusal says
/// "made with a newer Stalagmight", never that the file is damaged: the
/// feature may be what relaxes the rule (review finding C-file-format-2).
/// Unknown survey keys alone do not change the verdict, since tier 1 never
/// changes meaning. Injection: returning `DecodeError::Invalid` whatever the
/// `requires` list says fails the first assertion.
#[test]
fn a_rule_broken_under_an_unknown_feature_is_newer_not_damaged() {
    let broken = example_with_an_off_line_station();
    let newer = broken.replacen("\"requires\":[]", "\"requires\":[\"zz-future\"]", 1);
    match decode(newer.as_bytes(), &DecodeOptions::default()) {
        Err(super::DecodeError::NewerFeatures {
            features,
            violations,
        }) => {
            assert_eq!(features, vec!["zz-future".to_string()]);
            assert!(!violations.is_empty());
        }
        other => panic!("not refused as newer: {other:?}"),
    }
    let message = decode(newer.as_bytes(), &DecodeOptions::default())
        .unwrap_err()
        .to_string();
    assert!(message.contains("newer Stalagmight"), "{message}");
    assert!(message.contains("zz-future"), "{message}");

    // The same survey without the unknown feature is damaged.
    assert!(matches!(
        decode(broken.as_bytes(), &DecodeOptions::default()),
        Err(super::DecodeError::Invalid { .. })
    ));
    let tier_one = broken.replacen("[9,\"Z9\"]", "[9,\"Z9\",{\"zz_future\":1}]", 1);
    assert!(matches!(
        decode(tier_one.as_bytes(), &DecodeOptions::default()),
        Err(super::DecodeError::Invalid { .. })
    ));
}

/// A summary that is not an object (a third-party writer's `null`) opens,
/// and the rewrites that edit it leave it out instead of failing: a display
/// toggle, a sync-state change and a Recents rename all still work (review
/// finding C-file-format-5). Injection: writing such a summary through
/// `replace_keys` again fails both rewrites with "expected an object".
#[test]
fn a_summary_that_is_not_an_object_never_blocks_a_rewrite() {
    let (p, extras) = example();
    let text = text_of(&encode(&p, &extras, Mode::Normal).unwrap()).to_string();
    let at = text.find("\"summary\":").unwrap();
    let end = at + text[at..].find(",\n").unwrap();
    let nulled = format!("{}\"summary\":null{}", &text[..at], &text[end..]);
    let decoded = open(&nulled);
    assert!(decoded.read_only().is_none());

    let rewritten = rewrite_local(nulled.as_bytes(), &decoded.extras).unwrap();
    assert!(!text_of(&rewritten).contains("\"summary\""));
    let again = decode(&rewritten, &DecodeOptions::default()).unwrap();
    assert_eq!(again.project, decoded.project);
    assert_eq!(again.extras.local, decoded.extras.local);

    let renamed = super::rename_project(nulled.as_bytes(), "Renamed").unwrap();
    assert!(!text_of(&renamed).contains("\"summary\""));
    let again = decode(&renamed, &DecodeOptions::default()).unwrap();
    assert_eq!(again.project.name, "Renamed");
}

/// The closure counter is range-checked like the station and Section ones:
/// at the format's integer bound the file opens (the ids are used up), one
/// more closure breaks an invariant (so the app refuses the edit), a counter
/// or a closure id past the bound is invalid, and salvage lowers the counter
/// and drops the closure (review finding C-file-format-6). Injection: without
/// the bound, the closure minted at the bound validates and the save fails.
#[test]
fn the_closure_counter_is_range_checked() {
    use crate::models::survey::{ClosureStrategy, CounterKind, Violation, MAX_CLOSURE_ID};
    let (p, extras) = example();
    let text = text_of(&encode(&p, &extras, Mode::Normal).unwrap()).to_string();
    let at_bound = text.replacen(
        "\"closure\":2,",
        &format!("\"closure\":{MAX_CLOSURE_ID},"),
        1,
    );
    assert_ne!(at_bound, text);
    let mut decoded = open(&at_bound);
    assert!(decoded.project.is_valid());
    decoded
        .project
        .push_closure(1, None, 4, None, ClosureStrategy::Batch);
    assert!(decoded
        .project
        .validate()
        .contains(&Violation::CounterOutOfRange {
            counter: CounterKind::Closure,
            next: MAX_CLOSURE_ID + 1,
        }));

    let mut past = p.clone();
    past.counters.next_closure_id = MAX_CLOSURE_ID + 1;
    assert!(!past.is_valid());
    let mut last_id = p.clone();
    last_id.closures[0].id = MAX_CLOSURE_ID;
    assert!(last_id.validate().contains(&Violation::IdOutOfRange {
        kind: CounterKind::Closure,
        id: MAX_CLOSURE_ID,
    }));

    // A file whose only closure has the last id: salvage drops it and keeps
    // the counter writable.
    let last = at_bound.replacen(
        "{\"id\":1,\"a\":4",
        &format!("{{\"id\":{MAX_CLOSURE_ID},\"a\":4"),
        1,
    );
    assert_ne!(last, at_bound);
    let (salvaged, report) = super::decode_salvage(last.as_bytes());
    let salvaged = salvaged.unwrap();
    assert!(salvaged.closures.is_empty(), "{report:?}");
    assert!(encode(&salvaged, &Extras::default(), Mode::Normal).is_ok());
}
