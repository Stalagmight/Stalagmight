//! Hostile input: depth bombs, huge counts, NaN literals, duplicate keys and
//! ids, dangling references, cycles, truncation at every byte, a BOM, bytes
//! that are not a project. Every one is a typed refusal, never a panic.

use std::fmt::Write as _;

use crate::models::survey::Violation;

use super::test_helpers::{append_to_lines, edit_line_header, generate, text_of, Shape, SMALL};
use super::types::Limit;
use super::{decode, encode, DecodeError, DecodeOptions, Decoded, Mode};

fn small_text(seed: u64) -> String {
    let (p, extras) = generate(seed, SMALL);
    text_of(&encode(&p, &extras, Mode::Normal).unwrap()).to_string()
}

fn open(text: &str) -> Result<Decoded, DecodeError> {
    decode(text.as_bytes(), &DecodeOptions::default())
}

fn violations(text: &str) -> Vec<Violation> {
    match open(text) {
        Err(DecodeError::Invalid { violations }) => violations,
        other => panic!("expected an invariant refusal, got {other:?}"),
    }
}

/// The first shot row of the file: `(row, uuid, to)`.
fn first_row(text: &str) -> (String, String, u32) {
    let row = text
        .lines()
        .find(|l| l.starts_with("[\""))
        .unwrap()
        .to_string();
    let uuid = row[2..row[2..].find('"').unwrap() + 2].to_string();
    let after = &row[uuid.len() + 4..];
    let to: u32 = after[..after.find(',').unwrap()].parse().unwrap();
    (row, uuid, to)
}

#[test]
fn depth_bombs_are_refused_without_overflowing_the_stack() {
    let base = small_text(1);
    let deep = format!("{}1{}", "[".repeat(100_000), "]".repeat(100_000));
    // In preserved (unknown) content: read iteratively, refused by depth.
    let unknown = base.replacen(
        "\n\"end\":true",
        &format!("\n\"zz\":{deep},\n\"end\":true"),
        1,
    );
    assert!(matches!(
        open(&unknown).unwrap_err(),
        DecodeError::TooDeep { .. }
    ));
    // In a structural member: serde's recursion limit.
    let stations = base.replacen("\"stations\":[\n", &format!("\"stations\":[\n{deep},\n"), 1);
    assert!(open(&stations).is_err());
    // As an unknown key's value inside a record: skipped iteratively, read-only.
    let in_row = base.replacen(",\"start\":", &format!(",\"zz\":{deep},\"start\":"), 1);
    let decoded = open(&in_row).unwrap();
    assert!(decoded.read_only().is_some());
    // In a tier-2 object: refused by depth.
    let in_project = base.replacen("\"project\":{", &format!("\"project\":{{\"zz\":{deep},"), 1);
    assert!(matches!(
        open(&in_project).unwrap_err(),
        DecodeError::TooDeep { .. }
    ));
}

#[test]
fn counts_over_a_cap_are_refused() {
    let (p, extras) = generate(
        2,
        Shape {
            lines: 30,
            max_shots: 10,
            rich: true,
        },
    );
    let bytes = encode(&p, &extras, Mode::Normal).unwrap();
    let cases: [(Limit, fn(&mut super::CountLimits)); 4] = [
        (Limit::Stations, |l| l.stations = 5),
        (Limit::Shots, |l| l.shots = 5),
        (Limit::Lines, |l| l.lines = 3),
        (Limit::CreditPeople, |l| l.credit_people = 1),
    ];
    for (limit, set) in cases {
        let mut options = DecodeOptions::default();
        set(&mut options.limits);
        match decode(&bytes, &options) {
            Err(DecodeError::TooComplex { limit: hit, .. }) => assert_eq!(hit, limit),
            other => panic!("{limit:?}: {other:?}"),
        }
    }
    let mut options = DecodeOptions::default();
    options.max_file_bytes = 100;
    assert!(matches!(
        decode(&bytes, &options),
        Err(DecodeError::TooLarge { .. })
    ));
    // Preserved unknown members are capped too.
    let base = text_of(&bytes);
    let many = (0..50).fold(String::new(), |mut s, i| {
        let _ = writeln!(s, "\"zz{i}\":{i},");
        s
    });
    let text = base.replacen("\n\"end\":true", &format!("\n{many}\"end\":true"), 1);
    let mut options = DecodeOptions::default();
    options.limits.preserved = 10;
    assert!(matches!(
        decode(text.as_bytes(), &options),
        Err(DecodeError::TooComplex {
            limit: Limit::Preserved,
            ..
        })
    ));
}

#[test]
fn non_finite_literals_are_refused() {
    let base = small_text(3);
    let (row, _, _) = first_row(&base);
    let fields: Vec<&str> = row.split(',').collect();
    for literal in [
        "NaN",
        "Infinity",
        "-Infinity",
        "1e400",
        "-1e999",
        "nan",
        "0x10",
        "+1",
        ".5",
        "01",
    ] {
        let mut edited = fields.clone();
        edited[2] = literal;
        let text = base.replacen(&row, &edited.join(","), 1);
        let error = open(&text).unwrap_err();
        assert!(
            matches!(
                error,
                DecodeError::Syntax { .. } | DecodeError::BadValue { .. }
            ),
            "{literal}: {error:?}"
        );
    }
}

/// Review finding B-derived-solver-3: a finite but absurd distance (the
/// documented example's first shot at 1e200 m) made a valid file whose raw
/// layout summed to infinity and whose Computed View was NaN. Version 2
/// bounds a distance at `MAX_MEASUREMENT_M`: past it the file is invalid
/// (refused with the repair offer, which does not guess at it), at it the
/// file opens and every derived position is finite.
#[test]
fn a_finite_but_absurd_distance_is_refused() {
    let (p, extras) = super::format_v2_doc_tests::example();
    let original = String::from_utf8(encode(&p, &extras, Mode::Normal).unwrap()).unwrap();
    let uuid = "\"5f0c2a1e-0000-4000-8000-000000000001\",1,";
    let row = format!("{uuid}12.5,90.0,90.0,");
    assert!(original.contains(&row));
    for length in [
        "1e200",
        "-1e200",
        "1000000.000001",
        "1.7976931348623157e308",
    ] {
        let text = original.replacen(&row, &format!("{uuid}{length},0.0,0.0,"), 1);
        match open(&text) {
            Err(DecodeError::Invalid { violations }) => assert!(
                violations
                    .iter()
                    .any(|v| matches!(v, Violation::OutOfBounds { .. })),
                "{length}: {violations:?}"
            ),
            other => panic!("{length}: {other:?}"),
        }
    }
    let text = original.replacen(&row, &format!("{uuid}1000000.0,0.0,0.0,"), 1);
    let decoded = open(&text).unwrap();
    let raw = decoded.project.raw_positions();
    assert!(raw.iter().all(|(_, c)| c.iter().all(|v| v.is_finite())));
    let adjusted = decoded
        .project
        .adjusted_positions(&crate::models::config::ClosureAdjustmentConfig::default());
    assert!(adjusted
        .iter()
        .all(|(_, c)| c.iter().all(|v| v.is_finite())));
}

/// An alias is `[id]`, `[id, name]` or `[id, name or null, line]` (review
/// finding UX-4), and its line must pass through its station.
/// Archived records (RS 8.1) are objects `{id, name, comment?, external?,
/// lines?}`: a missing id or name refuses, an unknown key opens read-only, and
/// a line that does not pass the station is a stale record line (A2). The
/// U1 alias rows of an earlier #827 build still read, naively, as records
/// (RS-F: the 8.2 upgrade).
#[test]
fn archived_rows_are_checked() {
    let (p, extras) = super::format_v2_doc_tests::example();
    let original = String::from_utf8(encode(&p, &extras, Mode::Normal).unwrap()).unwrap();
    let record = "\"archived\":[{\"id\":7,\"name\":\"B0\",\"lines\":[1]}]";
    assert!(original.contains(record), "the example names Side");
    for (row, ok) in [
        ("\"archived\":[{\"id\":7,\"name\":\"B0\"}]", true),
        (
            "\"archived\":[{\"id\":7,\"name\":\"B0\",\"comment\":\"x\",\"lines\":[1]}]",
            true,
        ),
        ("\"archived\":[{\"id\":7,\"lines\":[1]}]", false),
        ("\"archived\":[{\"name\":\"B0\",\"lines\":[1]}]", false),
        (
            "\"archived\":[{\"id\":7,\"name\":\"B0\",\"lines\":[\"1\"]}]",
            false,
        ),
        ("\"archived\":[[7,\"B0\",1]]", false),
        ("\"aliases\":[[7,\"B0\",1]]", true),
        ("\"aliases\":[[7,null,1]]", true),
        ("\"aliases\":[[7,null]]", false),
    ] {
        let text = original.replacen(record, row, 1);
        let opened = open(&text);
        assert_eq!(opened.is_ok(), ok, "{row}: {:?}", opened.err());
    }
    let unknown = original.replacen(
        record,
        "\"archived\":[{\"id\":7,\"name\":\"B0\",\"lines\":[1],\"later\":true}]",
        1,
    );
    assert!(
        open(&unknown).unwrap().read_only().is_some(),
        "an unknown key is tier 1"
    );
    // Line 2 ("Unconnected") does not pass station 2.
    let text = original.replacen(
        record,
        "\"archived\":[{\"id\":7,\"name\":\"B0\",\"lines\":[2]}]",
        1,
    );
    assert_eq!(
        violations(&text),
        vec![Violation::StaleArchivedLine {
            station: 2,
            archived: 7,
            line: 2
        }]
    );
}

#[test]
fn duplicate_keys_are_refused() {
    let base = small_text(4);
    let cases = [
        base.replacen("\n\"end\":true", "\n\"project\":{},\n\"end\":true", 1),
        base.replacen(",\"start\":", ",\"color\":\"#000\",\"start\":", 1),
        base.replacen(
            "\"counters\":{\"station\":",
            "\"counters\":{\"section\":0,\"station\":",
            1,
        ),
        base.replacen(
            "\"project\":{\"name\":",
            "\"project\":{\"name\":\"x\",\"name\":",
            1,
        ),
    ];
    for text in cases {
        assert_ne!(text, base);
        assert!(
            matches!(open(&text).unwrap_err(), DecodeError::DuplicateKey { .. }),
            "{}",
            &text[..200]
        );
    }
    // Inside a shot row's extras object (a streamed record).
    let text = base.replacen(",{\"kind\":", ",{\"kind\":\"jump\",\"kind\":", 1);
    if text != base {
        assert!(matches!(
            open(&text).unwrap_err(),
            DecodeError::DuplicateKey { .. }
        ));
    }
}

#[test]
fn duplicate_ids_and_dangling_references_are_refused() {
    let base = small_text(5);
    // A second record for station 0.
    let text = base.replacen("\"stations\":[\n", "\"stations\":[\n[0,\"dup\"],\n", 1);
    assert_eq!(
        open(&text).unwrap_err(),
        DecodeError::DuplicateStation { id: 0 }
    );
    // A shot pointing at a station that does not exist.
    let (row, uuid, to) = first_row(&base);
    let dangling = row.replacen(
        &format!("\"{uuid}\",{to},"),
        &format!("\"{uuid}\",987654,"),
        1,
    );
    let v = violations(&base.replacen(&row, &dangling, 1));
    assert!(v.iter().any(|v| matches!(
        v,
        Violation::DanglingShotTarget {
            station: 987_654,
            ..
        }
    )));
    // A repeated shot uuid.
    let rows: Vec<&str> = base.lines().filter(|l| l.starts_with("[\"")).collect();
    if rows.len() >= 2 {
        let (_, second_uuid, _) = first_row(&base.replacen(rows[0], "", 1));
        let text = base.replacen(&format!("\"{second_uuid}\""), &format!("\"{uuid}\""), 1);
        let v = violations(&text);
        assert!(v
            .iter()
            .any(|v| matches!(v, Violation::DuplicateShotUuid { .. })));
    }
    // A repeated line id.
    let text = edit_line_header(&base, 1, |h| h.replacen("{\"id\":1,", "{\"id\":0,", 1));
    if text != base {
        let v = violations(&text);
        assert!(v
            .iter()
            .any(|v| matches!(v, Violation::DuplicateLineId { line: 0 })));
    }
    // An id out of the integer range.
    let text = base.replacen("\"stations\":[\n[0,", "\"stations\":[\n[4294967296,", 1);
    assert!(matches!(
        open(&text).unwrap_err(),
        DecodeError::BadValue { .. }
    ));
    let text = base.replacen(
        "\"counters\":{\"station\":",
        "\"counters\":{\"closure\":9007199254740992,\"station\":",
        1,
    );
    assert!(open(&text).is_err());
}

#[test]
fn a_cycle_without_a_closure_is_refused() {
    // Line 1 ends where line 0 starts: with line 0's own chain, a loop.
    let (p, extras) = (0..1000)
        .map(|seed| {
            generate(
                seed,
                Shape {
                    lines: 1,
                    max_shots: 4,
                    rich: false,
                },
            )
        })
        .find(|(p, _)| p.lines()[0].shots.len() >= 2)
        .unwrap();
    let mut q = p.clone();
    let chain = q.line_stations(q.lines()[0].id).unwrap().to_vec();
    let shots = vec![crate::models::ShotData {
        uuid: Some("loop".to_string()),
        length: 3.0,
        ..Default::default()
    }];
    q.add_line_on_stations(
        crate::models::survey::LineMeta::default(),
        &[chain[chain.len() - 1], chain[0]],
        shots,
    )
    .unwrap();
    assert!(!q.validate().is_empty());
    // Write it by hand: encode refuses an invalid model.
    assert!(matches!(
        encode(&q, &extras, Mode::Normal),
        Err(super::EncodeError::Invalid { .. })
    ));
    let text = text_of(&encode(&p, &extras, Mode::Normal).unwrap()).to_string();
    let header = format!(
        "{{\"id\":99,\"color\":\"#000\",\"direction\":\"in\",\"quality\":1.0,\"start\":{}}},\n[\"loop\",{},3.0,0.0,0.0,0.0,0.0]",
        chain[chain.len() - 1],
        chain[0]
    );
    let edited = append_to_lines(&text, &header);
    let v = violations(&edited);
    assert!(
        v.iter()
            .any(|v| matches!(v, Violation::Cycle { line: 99, .. })),
        "{v:?}"
    );
}

#[test]
fn every_truncation_is_refused() {
    let bytes = encode(&generate(7, SMALL).0, &generate(7, SMALL).1, Mode::Normal).unwrap();
    // Dropping only the final newline leaves a complete document.
    for cut in 0..bytes.len() - 1 {
        if let Ok(d) = decode(&bytes[..cut], &DecodeOptions::default()) {
            panic!("a {cut}-byte prefix decoded: {:?}", d.project.name);
        }
    }
    assert!(decode(&bytes[..bytes.len() - 1], &DecodeOptions::default()).is_ok());
    // A truncation inside the body is reported as such.
    let half = &bytes[..bytes.len() / 2];
    assert!(matches!(
        decode(half, &DecodeOptions::default()).unwrap_err(),
        DecodeError::Truncated { .. }
    ));
}

#[test]
fn a_bom_other_formats_and_garbage_are_refused() {
    let base = small_text(8);
    let bom = format!("\u{feff}{base}");
    assert_eq!(open(&bom).unwrap_err(), DecodeError::ByteOrderMark);
    let mut sqlite = b"SQLite format 3\0".to_vec();
    sqlite.extend_from_slice(&[0u8; 100]);
    assert_eq!(
        decode(&sqlite, &DecodeOptions::default()).unwrap_err(),
        DecodeError::Legacy
    );
    for junk in [
        "",
        "{}",
        "[]",
        "{\"version\":2,\"format\":\"stalx\"}",
        "{\"format\":\"stalz\"",
    ] {
        assert_eq!(open(junk).unwrap_err(), DecodeError::NotStalx, "{junk}");
    }
    let mut not_utf8 = base.into_bytes();
    let at = not_utf8.len() / 2;
    not_utf8[at] = 0xff;
    assert!(matches!(
        decode(&not_utf8, &DecodeOptions::default()).unwrap_err(),
        DecodeError::Syntax { .. }
    ));
}

#[test]
fn malformed_rows_are_refused() {
    let base = small_text(9);
    let (row, _, _) = first_row(&base);
    let cases = [
        // A row before any header.
        base.replacen("\"lines\":[\n", &format!("\"lines\":[\n{row},\n"), 1),
        // Too few cells.
        base.replacen(&row, "[\"x\",1,2.0]", 1),
        // A fifth LRUD cell.
        base.replacen(
            &row,
            &format!(
                "{},1.0,1.0,1.0,1.0,1.0]",
                row.trim_end_matches(']').split(",{").next().unwrap()
            ),
            1,
        ),
        // Something after the extras object.
        base.replacen(
            &row,
            &format!(
                "{},{{}},1]",
                row.trim_end_matches(']').split(",{").next().unwrap()
            ),
            1,
        ),
        // A string where a number goes.
        base.replacen(
            &row,
            &row.replacen(',', ",\"1\",", 2).replacen(",\"1\",", ",", 1),
            1,
        ),
    ];
    for text in cases {
        assert!(
            open(&text).is_err(),
            "{}",
            text.lines().nth(20).unwrap_or("")
        );
    }
}

/// No byte flip anywhere in a small file makes the decoder panic.
#[test]
fn single_byte_corruption_never_panics() {
    let bytes = encode(&generate(10, SMALL).0, &generate(10, SMALL).1, Mode::Normal).unwrap();
    for i in 0..bytes.len() {
        for b in [b'0', b'"', b'{', b']', b',', b'x', 0] {
            let mut corrupt = bytes.clone();
            corrupt[i] = b;
            let _ = decode(&corrupt, &DecodeOptions::default());
        }
    }
}

/// Unknown keys in records count against the preserved-content cap, so an
/// object with very many of them is refused quickly instead of scanned
/// quadratically.
#[test]
fn a_flood_of_unknown_keys_is_refused() {
    let base = small_text(11);
    let flood = (0..20_000).fold(String::new(), |mut s, i| {
        let _ = write!(s, "\"zz{i}\":0,");
        s
    });
    for (from, to) in [
        (",\"start\":", format!(",{flood}\"start\":")),
        ("{\"station\":", format!("{{{flood}\"station\":")),
    ] {
        let text = base.replacen(from, &to, 1);
        if text == base {
            continue;
        }
        let started = std::time::Instant::now();
        assert!(matches!(
            open(&text).unwrap_err(),
            DecodeError::TooComplex {
                limit: Limit::Preserved,
                ..
            }
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}

/// A file cut inside a multi-byte character, or inside its first tokens, is
/// a truncated file, not bad text or another format.
#[test]
fn cuts_inside_a_character_or_the_prefix_are_truncations() {
    let base = small_text(12);
    let bytes = base.as_bytes();
    let multi = (0..bytes.len()).find(|&i| bytes[i] >= 0xc0).unwrap();
    assert!(matches!(
        decode(&bytes[..=multi], &DecodeOptions::default()).unwrap_err(),
        DecodeError::Truncated { .. }
    ));
    for cut in [
        "{\"format\":\"stalx\"",
        "{\"format\":\"stalx\",\"v",
        "{\"format\":\"stalx\",\"version\":",
    ] {
        assert!(
            matches!(open(cut).unwrap_err(), DecodeError::Truncated { .. }),
            "{cut}"
        );
    }
    assert_eq!(open("{\"form").unwrap_err(), DecodeError::NotStalx);
    // Duplicate keys in the summary are refused like anywhere else.
    let text = base.replacen(
        "\"summary\":{\"name\":",
        "\"summary\":{\"name\":\"x\",\"name\":",
        1,
    );
    assert!(matches!(
        open(&text).unwrap_err(),
        DecodeError::DuplicateKey { .. }
    ));
}
