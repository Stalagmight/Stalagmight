//! History entries: the persisted form is canonical and a fixpoint, and a
//! hostile entry (truncated, garbage, flipped bytes, another version,
//! oversized, out-of-range ids, runs that overlap or read past their source,
//! counts it does not materialise) is refused without a panic and without
//! quadratic work. A decoded hostile entry that passes the checks still
//! cannot make a step panic.

use std::time::{Duration, Instant};

use chrono::{TimeZone, Utc};

use super::testkit::{record_walk, Recorded};
use super::{
    decode_entry, encode_entry, redo, undo, EntryError, EntryLimits, HistoryEntry, HistoryRefs,
    PreviewRow, RecordedPreview, HISTORY_FORMAT_VERSION,
};

fn entry(changes: super::Changeset) -> HistoryEntry {
    HistoryEntry {
        timestamp: Utc.with_ymd_and_hms(2026, 11, 20, 12, 59, 0).unwrap(),
        kind: "update_shot".to_string(),
        label: "Update shot Main: 4 \u{2192} 5 \"quoted\" \u{e9}".to_string(),
        refs: HistoryRefs {
            lines: vec![3],
            stations: vec![7, 8],
            shots: vec!["uuid-1".to_string()],
            closures: vec![2],
            sections: vec![],
        },
        preview: RecordedPreview {
            rows: vec![PreviewRow {
                field: "Length".to_string(),
                old: "3.50".to_string(),
                new: "4.00".to_string(),
            }],
            more: 2,
        },
        changes,
    }
}

/// Entries of a few random walks, as text, with the state each undoes from.
fn samples() -> Vec<(String, super::super::Project, super::super::Project)> {
    let mut out = Vec::new();
    for seed in [3, 11, 19] {
        let Recorded {
            states, changes, ..
        } = record_walk(seed, 40);
        for (k, cs) in changes.into_iter().enumerate() {
            let text = encode_entry(&entry(cs)).unwrap();
            out.push((text, states[k].clone(), states[k + 1].clone()));
        }
    }
    out
}

#[test]
fn an_entry_is_canonical_compact_json_and_a_fixpoint() {
    for (text, _, _) in samples() {
        assert!(
            text.starts_with(&format!(
                "{{\"v\":{HISTORY_FORMAT_VERSION},\"t\":\"2026-11-20T12:59:00.000000000Z\",\"k\":"
            )),
            "{text}"
        );
        assert!(!text.contains('\n'), "one line");
        let back = decode_entry(&text, &EntryLimits::default()).unwrap();
        assert_eq!(
            encode_entry(&back).unwrap(),
            text,
            "decode then encode is the identity"
        );
        assert_eq!(back.label, entry(super::Changeset::default()).label);
        assert_eq!(back.refs.stations, vec![7, 8]);
        assert_eq!(back.preview.more, 2);
        // Text the serializer would write from the parsed value: canonical.
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(serde_json::to_string(&value).unwrap().len(), text.len());
    }
}

#[test]
fn an_entry_of_another_version_or_an_unknown_member_is_refused() {
    let (text, _, _) = samples().into_iter().nth(5).unwrap();
    let other = text.replacen("{\"v\":1,", "{\"v\":2,", 1);
    assert_eq!(
        decode_entry(&other, &EntryLimits::default()).unwrap_err(),
        EntryError::Version(2)
    );
    let extra = text.replacen("{\"v\":1,", "{\"v\":1,\"cmd\":{},", 1);
    assert!(matches!(
        decode_entry(&extra, &EntryLimits::default()),
        Err(EntryError::Syntax(_))
    ));
    let no_version = text.replacen("{\"v\":1,", "{", 1);
    assert!(decode_entry(&no_version, &EntryLimits::default()).is_err());
    let bad_time = text.replacen("2026-11-20T12:59:00", "yesterday-ish", 1);
    assert!(matches!(
        decode_entry(&bad_time, &EntryLimits::default()),
        Err(EntryError::Time(_))
    ));
}

#[test]
fn a_truncated_or_byte_flipped_entry_never_panics_and_never_breaks_a_step() {
    let limits = EntryLimits::default();
    let mut decoded_variants = 0usize;
    for (text, before, after) in samples().into_iter().step_by(3) {
        let bytes = text.as_bytes();
        for cut in 0..bytes.len() {
            if let Ok(prefix) = std::str::from_utf8(&bytes[..cut]) {
                assert!(
                    decode_entry(prefix, &limits).is_err(),
                    "a cut at {cut} decoded"
                );
            }
        }
        for at in 0..bytes.len() {
            for replacement in [b'0', b'9', b'"', b'[', b'-', b'}'] {
                let mut flipped = bytes.to_vec();
                flipped[at] = replacement;
                let Ok(flipped) = String::from_utf8(flipped) else {
                    continue;
                };
                let Ok(hostile) = decode_entry(&flipped, &limits) else {
                    continue;
                };
                decoded_variants += 1;
                // Whatever passed the checks must step or refuse, never panic.
                let mut live = after.clone();
                if undo(&mut live, &hostile.changes).is_err() {
                    assert_eq!(live, after, "a refused undo changed the project");
                }
                let mut live = before.clone();
                if redo(&mut live, &hostile.changes).is_err() {
                    assert_eq!(live, before, "a refused redo changed the project");
                }
            }
        }
    }
    assert!(
        decoded_variants > 100,
        "only {decoded_variants} flips decoded"
    );
}

#[test]
fn garbage_and_oversized_entries_are_refused() {
    let limits = EntryLimits::default();
    for garbage in [
        "",
        "null",
        "[]",
        "{}",
        "{\"v\":1}",
        "\u{feff}{\"v\":1,\"t\":\"x\",\"k\":\"k\",\"label\":\"l\",\"ops\":{}}",
        "{\"v\":1e400}",
        "{\"v\":1,\"t\":\"2026-01-01T00:00:00Z\",\"k\":\"k\",\"label\":\"l\",\"ops\":{\"datums\":{\"n\":[0,1],\"h\":[[0,[],[{\"station\":1}]]]}}}",
    ] {
        assert!(decode_entry(garbage, &limits).is_err(), "{garbage:?} decoded");
    }
    let small = EntryLimits {
        max_bytes: 100,
        ..EntryLimits::default()
    };
    let (text, _, _) = samples().into_iter().last().unwrap();
    if text.len() > 100 {
        assert_eq!(
            decode_entry(&text, &small).unwrap_err(),
            EntryError::TooLarge(text.len())
        );
    }
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    let wrapped = format!(
        "{{\"v\":1,\"t\":\"2026-01-01T00:00:00Z\",\"k\":\"k\",\"label\":\"l\",\"ops\":{deep}}}"
    );
    assert!(
        decode_entry(&wrapped, &limits).is_err(),
        "a depth bomb decoded"
    );
    let long_label = format!(
        "{{\"v\":1,\"t\":\"2026-01-01T00:00:00Z\",\"k\":\"k\",\"label\":\"{}\",\"ops\":{{}}}}",
        "x".repeat(5_000)
    );
    assert_eq!(
        decode_entry(&long_label, &limits).unwrap_err(),
        EntryError::Limit("kind or label characters")
    );
}

/// An entry around `ops` (a changeset's JSON).
fn wrap(ops: &str) -> String {
    format!("{{\"v\":1,\"t\":\"2026-01-01T00:00:00Z\",\"k\":\"k\",\"label\":\"l\",\"ops\":{ops}}}")
}

fn shape_error(ops: &str) -> String {
    match decode_entry(&wrap(ops), &EntryLimits::default()) {
        Err(EntryError::Shape(why)) => why,
        other => panic!("{ops}: not a shape error: {other:?}"),
    }
}

#[test]
fn a_structurally_broken_changeset_is_refused_by_the_shape_rules() {
    let side = |n: u32, segs: &str| format!("{{\"n\":{n},\"h\":1,\"s\":[{segs}]}}");
    let line = |id: u32, before: &str, after: &str| {
        format!("{{\"id\":{id},\"before\":{before},\"after\":{after}}}")
    };
    // A run reading a line the entry does not record.
    let why = shape_error(&format!(
        "{{\"lines\":[{}]}}",
        line(1, &side(2, "[1,0,2,0,null]"), &side(2, "[9,0,2,0,null]"))
    ));
    assert!(why.contains("does not record"), "{why}");
    // A run past the end of what it reads.
    let why = shape_error(&format!(
        "{{\"lines\":[{}]}}",
        line(1, &side(2, "[1,0,2,0,null]"), &side(3, "[1,0,3,0,null]"))
    ));
    assert!(
        why.contains("past the end") || why.contains("hold"),
        "{why}"
    );
    // Two runs taking the same source shot.
    let why = shape_error(&format!(
        "{{\"lines\":[{}]}}",
        line(
            1,
            &side(4, "[1,0,4,0,null]"),
            &side(4, "[1,0,2,0,null],[1,1,2,0,null]")
        )
    ));
    assert!(why.contains("same shot"), "{why}");
    // A repeat span claiming four billion cells from a few bytes.
    let why = shape_error(&format!(
        "{{\"lines\":[{}]}}",
        line(
            1,
            &side(2, "[1,0,2,0,null]"),
            &side(2, "[1,0,2,0,null,{\"len\":[[0,4000000000,1.0]]}]")
        )
    ));
    assert!(why.contains("out of range"), "{why}");
    // A credit the table does not hold, a value a field does not take.
    for edits in [
        "{\"cr\":[[0,[0]]]}",
        "{\"len\":[[0,[\"long\"]]]}",
        "{\"k\":[[0,[\"Sideways\"]]]}",
    ] {
        let why = shape_error(&format!(
            "{{\"lines\":[{}]}}",
            line(
                1,
                &side(2, "[1,0,2,0,null]"),
                &side(2, &format!("[1,0,2,0,null,{edits}]"))
            )
        ));
        assert!(why.contains("value"), "{edits}: {why}");
    }
    // Out-of-range ids and counters.
    let why = shape_error(&format!(
        "{{\"lines\":[{}]}}",
        line(50_000, &side(0, ""), &side(0, ""))
    ));
    assert!(why.contains("id space"), "{why}");
    let why = shape_error("{\"counters\":[[0,1,0],[4294967295,1,0]]}");
    assert!(why.contains("counter"), "{why}");
    let why = shape_error("{\"stations\":{\"add\":[[3000000000,\"x\"]]}}");
    assert!(why.contains("id space"), "{why}");
    // A line created without the order knowing it.
    let why = shape_error(
        "{\"lines\":[{\"id\":4,\"after\":{\"n\":0,\"h\":1,\"line\":{\"color\":\"#000\",\"dir\":\"in\",\"q\":1.0,\"start\":0},\"s\":[]}}]}",
    );
    assert!(why.contains("order"), "{why}");
    // List hunks out of order or past the list.
    let why = shape_error("{\"closures\":{\"n\":[1,1],\"h\":[[5,[],[]]]}}");
    assert!(why.contains("closures"), "{why}");
    let why = shape_error("{\"order\":{\"n\":[2,2],\"h\":[[1,[5],[6]],[0,[4],[7]]]}}");
    assert!(why.contains("order"), "{why}");
    // Station field columns whose ids and values disagree.
    let why = shape_error("{\"stations\":{\"name\":[[[1,3]],[\"a\"],[\"b\"]]}}");
    assert!(why.contains("name"), "{why}");
}

/// Large hostile entries decode (or refuse) in linear time: a column of a
/// hundred thousand one-cell spans, a hundred thousand station id runs, a
/// hundred thousand list hunks.
#[test]
fn a_large_hostile_entry_costs_linear_work() {
    let n = super::super::MAX_SHOTS_PER_LINE;
    let spans: Vec<String> = (0..n).map(|i| format!("[{i},[1.5]]")).collect();
    let side = format!(
        "{{\"n\":{n},\"h\":1,\"s\":[[1,0,{n},0,null,{{\"l\":[{}]}}]]}}",
        spans.join(",")
    );
    let lines = format!(
        "{{\"lines\":[{{\"id\":1,\"before\":{{\"n\":{n},\"h\":1,\"s\":[[1,0,{n},0,null]]}},\"after\":{side}}}]}}"
    );
    let runs: Vec<String> = (0..n).map(|i| format!("[{},1]", i * 2)).collect();
    let names: Vec<String> = (0..n).map(|_| "\"a\"".to_string()).collect();
    let stations = format!(
        "{{\"stations\":{{\"name\":[[{}],[{}],[{}]]}}}}",
        runs.join(","),
        names.join(","),
        names.join(",")
    );
    let hunks: Vec<String> = (0..n)
        .map(|i| format!("[{},[\"x\"],[\"y\"]]", i * 2))
        .collect();
    let hashes = format!(
        "{{\"line_hashes\":{{\"n\":[{},{}],\"h\":[{}]}}}}",
        2 * n,
        2 * n,
        hunks.join(",")
    );
    for ops in [lines, stations, hashes] {
        let text = wrap(&ops);
        let started = Instant::now();
        let decoded = decode_entry(&text, &EntryLimits::default());
        let took = started.elapsed();
        assert!(decoded.is_ok(), "{:?}", decoded.err());
        assert!(
            took < Duration::from_secs(20),
            "{} bytes took {took:?} to decode",
            text.len()
        );
        // Applying it to a project that does not match refuses quickly.
        let mut p = super::super::test_helpers::project();
        let started = Instant::now();
        let changes = decoded.unwrap().changes;
        assert!(undo(&mut p, &changes).is_err());
        assert!(redo(&mut p, &changes).is_err());
        assert!(started.elapsed() < Duration::from_secs(20));
    }
}

#[test]
fn a_non_finite_number_is_refused_on_write() {
    let Recorded {
        states, changes, ..
    } = record_walk(5, 30);
    let mut cs = changes
        .into_iter()
        .find(|c| !c.lines.is_empty())
        .expect("a walk edits a line");
    let _ = states;
    let line = &mut cs.lines[0];
    let side = line.after.as_mut().or(line.before.as_mut()).unwrap();
    side.s.push(super::shots::Seg::Rows(super::shots::Rows {
        uuids: vec!["nan".to_string()],
        cols: super::values::Cols(vec![(
            super::ShotField::Length,
            vec![super::values::Span::Values(
                0,
                vec![super::Val::Num(f64::NAN)],
            )],
        )]),
    }));
    side.n += 1;
    assert_eq!(encode_entry(&entry(cs)).unwrap_err(), EntryError::NonFinite);
}

/// A station image's alias carries the line that knew the station by it
/// (review finding UX-4): `[id]`, `[id, name]`, `[id, name, line]` or
/// `[id, null, line]`; a null name with no line, or a fourth element, is
/// refused.
#[test]
fn an_alias_row_carries_its_line() {
    use super::super::{Station, StationAlias};
    use super::stations::StationRow;
    let mut station = Station::new(3, "3");
    for (id, name, line) in [
        (7, None, None),
        (8, Some("0"), None),
        (9, Some("1"), Some(2)),
        (10, None, Some(4)),
    ] {
        station.aliases.push(StationAlias {
            id,
            name: name.map(str::to_string),
            line,
        });
    }
    let text = serde_json::to_string(&StationRow(station.clone())).unwrap();
    assert_eq!(
        text,
        r#"[3,"3",{"aliases":[[7],[8,"0"],[9,"1",2],[10,null,4]]}]"#
    );
    let back: StationRow = serde_json::from_str(&text).unwrap();
    assert_eq!(back.0, station);
    for bad in [
        r#"[3,"3",{"aliases":[[7,null]]}]"#,
        r#"[3,"3",{"aliases":[[7,"0",2,5]]}]"#,
    ] {
        assert!(serde_json::from_str::<StationRow>(bad).is_err(), "{bad}");
    }
}
