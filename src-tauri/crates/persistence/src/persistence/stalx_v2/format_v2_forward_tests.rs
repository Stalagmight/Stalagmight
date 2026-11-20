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
