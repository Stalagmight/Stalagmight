//! Hostile version 2 input, fuzzed (C24, plan 11.2). The encoder's output
//! for a seeded survey (`test_helpers::generate`, rich and plain, small and
//! medium) is mutated by one targeted operator per seed, sometimes followed
//! by random byte flips:
//!
//! byte flips; truncation at every kind of row boundary; a member repeated;
//! members and record keys reordered; insignificant whitespace (in the
//! prefix too); a missing or false `end`; `NaN` / `Infinity` / out-of-range
//! literals; integers out of their range; deep nesting in unknown members,
//! history entries and records; huge and escape-heavy strings, bad escapes
//! and lone surrogates; other versions and solvers; unknown required
//! features; bad shot uuids; duplicate station ids; dangling references
//! (shot targets, line starts, datums, placements, closure ends, `via`
//! hints, Sections, hand moves); a loop of shots; closures to unknown
//! stations; anchors removed or doubled; rows duplicated or deleted; unknown
//! keys in records and unknown top-level members; a byte order mark; the
//! SQLite magic; history floods and an over-long line.
//!
//! Properties: `sniff`, `probe_bytes`, `decode` and `decode_salvage` never
//! panic and finish within a time bound on inputs of bounded size (the
//! allocation bound: a small file that claims a large count or id cannot make
//! the reader build anything large; no allocator hook, `unsafe_code` is
//! denied); every refusal is a typed `DecodeError` of the class the operator
//! must produce, with a message; whatever is accepted validates, re-encodes
//! canonically (decode of the re-encoding is the same project, and encoding
//! that is the same bytes), or is read-only and then refuses every encode
//! while a local-only rewrite keeps its survey; a semantically neutral edit
//! (reordering, whitespace, a final newline) decodes to the same project and
//! re-encodes to the original bytes; a repaired salvage validates.
//!
//! Windows: `STALAGMIGHT_FUZZ_SEED_START` / `STALAGMIGHT_FUZZ_SEEDS`;
//! `decode_fuzz_window` runs a fixed window in CI, `decode_fuzz_wide`
//! (ignored) 2,000 seeds. A failure prints one line: the seed, the operator,
//! the test that replays it alone, and what failed. Synthetic data only.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::test_helpers::{generate, Rng, Shape, MEDIUM, SMALL};
use super::{
    decode, decode_salvage, encode, probe_bytes, rewrite_local, sniff, DecodeError, DecodeNote,
    DecodeOptions, Decoded, EncodeError, Extras, Mode,
};

/// Seeds the CI window runs when no window is set (each operator several
/// times over).
const CI_SEEDS: u64 = 160;
/// Seeds the wide run takes when no window is set.
const WIDE_SEEDS: u64 = 2_000;
/// The longest one read may take (debug build, inputs up to a few MiB).
const TIME_BOUND: Duration = Duration::from_secs(20);

const TEST_PATH: &str = "persistence::stalx_v2::decode_fuzz_tests";

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

fn window(default_count: u64) -> (u64, u64) {
    (
        env_u64("STALAGMIGHT_FUZZ_SEED_START").unwrap_or(0),
        env_u64("STALAGMIGHT_FUZZ_SEEDS").unwrap_or(default_count),
    )
}

/// What a mutated file must do.
#[derive(Clone, Copy)]
enum Expect {
    /// Anything, as long as the generic properties hold.
    Any,
    /// A refusal of one of these classes.
    Refused(&'static [&'static str]),
    /// The same project and extras as the original; re-encodes to its bytes.
    Same,
    /// As `Same`, with an `OtherSolver` note.
    OtherSolver,
    /// Opens read-only (unknown feature or structural content).
    ReadOnly,
    /// Opens writable and keeps an unknown top-level member through a save.
    Preserved(&'static str),
    /// Opens with the history dropped (tier 3), the survey intact.
    HistoryDropped,
}

/// The class name of a refusal (stable, for expectations and counts).
fn class(e: &DecodeError) -> &'static str {
    match e {
        DecodeError::NotStalx => "not-stalx",
        DecodeError::Legacy => "legacy",
        DecodeError::ByteOrderMark => "bom",
        DecodeError::TooLarge { .. } => "too-large",
        DecodeError::NewerVersion { .. } => "newer-version",
        DecodeError::UnsupportedVersion { .. } => "unsupported-version",
        DecodeError::Conventions { .. } => "conventions",
        DecodeError::TooComplex { .. } => "too-complex",
        DecodeError::Truncated { .. } => "truncated",
        DecodeError::Syntax { .. } => "syntax",
        DecodeError::BadValue { .. } => "bad-value",
        DecodeError::Missing { .. } => "missing",
        DecodeError::DuplicateKey { .. } => "duplicate-key",
        DecodeError::TooDeep { .. } => "too-deep",
        DecodeError::MissingEnd => "missing-end",
        DecodeError::DuplicateStation { .. } => "duplicate-station",
        DecodeError::Invalid { .. } => "invalid",
    }
}

/// One mutated file.
struct Mutant {
    op: &'static str,
    bytes: Vec<u8>,
    expect: Expect,
    options: DecodeOptions,
}

/// A top-level member of the canonical text: its key and byte span (the
/// member's text up to and including its trailing `,\n`).
struct Member {
    key: String,
    start: usize,
    end: usize,
}

/// The canonical text's top-level members after the first line (which holds
/// `format`, `version`, `solver` and `requires`). Canonical text has a
/// newline only between records and members, and every member line starts
/// with its key (records start with `[` or `{`).
fn members(text: &str) -> Vec<Member> {
    let mut starts: Vec<(usize, String)> = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        if at > 0 && line.starts_with('"') {
            let key_end = line[1..].find('"').map_or(1, |i| i + 1);
            starts.push((at, line[1..key_end].to_string()));
        }
        at += line.len();
    }
    let mut out = Vec::new();
    for (i, (start, key)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map_or(text.len(), |(s, _)| *s);
        out.push(Member {
            key: key.clone(),
            start: *start,
            end,
        });
    }
    out
}

/// The record lines of the list member `key` (each without its comma and
/// newline): `(start, end)` byte spans.
fn rows(text: &str, key: &str) -> Vec<(usize, usize)> {
    let Some(m) = members(text).into_iter().find(|m| m.key == key) else {
        return Vec::new();
    };
    let body = &text[m.start..m.end];
    let mut out = Vec::new();
    let mut at = m.start;
    for line in body.split_inclusive('\n') {
        if line.starts_with('[') || line.starts_with('{') {
            let trimmed = line.trim_end_matches('\n').trim_end_matches(',');
            out.push((at, at + trimmed.len()));
        }
        at += line.len();
    }
    out
}

/// A token outside strings.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tok {
    Number,
    Str,
}

/// Every number and string token of `text`, with its span.
fn tokens(text: &str) -> Vec<(usize, usize, Tok)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
                out.push((start, i.min(b.len()), Tok::Str));
            }
            b'-' | b'0'..=b'9' => {
                let start = i;
                while i < b.len() && matches!(b[i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
                {
                    i += 1;
                }
                out.push((start, i, Tok::Number));
            }
            _ => i += 1,
        }
    }
    out
}

/// The member a byte offset is in.
fn member_at(text: &str, at: usize) -> String {
    members(text)
        .into_iter()
        .find(|m| m.start <= at && at < m.end)
        .map_or_else(String::new, |m| m.key)
}

fn splice(text: &str, start: usize, end: usize, with: &str) -> String {
    format!("{}{with}{}", &text[..start], &text[end..])
}

/// Members whose numbers are survey content (not tier 2 or 3).
const SURVEY: [&str; 9] = [
    "counters",
    "sections",
    "stations",
    "lines",
    "datums",
    "placements",
    "closures",
    "ignored_warnings",
    "settings",
];

fn nested(depth: usize) -> String {
    format!("{}1{}", "[".repeat(depth), "]".repeat(depth))
}

/// The value of the first integer token after `needle` in `row` (a record
/// line), with its span in the whole text.
fn int_after(text: &str, row: (usize, usize), needle: &str) -> Option<(usize, usize)> {
    let line = &text[row.0..row.1];
    let at = line.find(needle)? + needle.len();
    let digits = line[at..].bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0).then_some((row.0 + at, row.0 + at + digits))
}

/// The `to` cell of a shot row: the integer after its uuid string.
fn shot_to(text: &str, row: (usize, usize)) -> Option<(usize, usize)> {
    let line = &text[row.0..row.1];
    if !line.starts_with("[\"") {
        return None;
    }
    let uuid_end = tokens(line).first().map(|t| t.1)?;
    let digits = line[uuid_end + 1..]
        .bytes()
        .take_while(u8::is_ascii_digit)
        .count();
    (digits > 0).then_some((row.0 + uuid_end + 1, row.0 + uuid_end + 1 + digits))
}

const DANGLING: &str = "987654321";

/// `text` without the record at `start..end` and its separator (the comma
/// after it, or before it when it is the last of its list).
fn delete_row(text: &str, start: usize, end: usize) -> String {
    if text[end..].starts_with(",\n") {
        return splice(text, start, end + 2, "");
    }
    match text[..start].strip_suffix(",\n") {
        Some(before) => format!("{before}{}", &text[end..]),
        None => splice(text, start, end, ""),
    }
}

/// One targeted mutation of `text` (the canonical encoding) chosen by
/// `which`, or `None` when the file has nothing to mutate that way.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per operator; splitting it scatters the catalogue"
)]
fn mutate(text: &str, which: usize, rng: &mut Rng) -> Option<Mutant> {
    let m = |op, bytes: String, expect| Mutant {
        op,
        bytes: bytes.into_bytes(),
        expect,
        options: DecodeOptions::default(),
    };
    let ms = members(text);
    let middle: Vec<&Member> = ms.iter().filter(|m| m.key != "end").collect();
    let line_rows = rows(text, "lines");
    let shot_rows: Vec<(usize, usize)> = line_rows
        .iter()
        .copied()
        .filter(|r| text[r.0..].starts_with('['))
        .collect();
    let header_rows: Vec<(usize, usize)> = line_rows
        .iter()
        .copied()
        .filter(|r| text[r.0..].starts_with('{'))
        .collect();
    let pick = |rng: &mut Rng, v: &[(usize, usize)]| (!v.is_empty()).then(|| *rng.pick(v));
    Some(match which {
        0 => {
            // Truncation at a row boundary: before or after a newline, or one
            // byte into the next row.
            let newlines: Vec<usize> = text.match_indices('\n').map(|(i, _)| i).collect();
            let at = *rng.pick(&newlines) + [0, 1, 2][rng.below(3)];
            if at >= text.len() - 1 {
                // Only the final newline (or nothing) is gone: complete.
                return Some(m(
                    "truncate-row",
                    text[..at.min(text.len())].to_string(),
                    Expect::Same,
                ));
            }
            m(
                "truncate-row",
                text[..at].to_string(),
                Expect::Refused(&["truncated", "missing-end", "syntax"]),
            )
        }
        1 => {
            let end = text.rfind("\"end\":true")?;
            match rng.below(3) {
                0 => m(
                    "end-missing",
                    format!("{}}}\n", text[..end].trim_end_matches(['\n', ','])),
                    Expect::Refused(&["missing-end"]),
                ),
                1 => m(
                    "end-false",
                    splice(text, end, end + 10, "\"end\":false"),
                    Expect::Refused(&["missing-end", "bad-value"]),
                ),
                _ => m(
                    "end-not-a-bool",
                    splice(text, end, end + 10, "\"end\":\"true\""),
                    Expect::Refused(&["missing-end", "bad-value"]),
                ),
            }
        }
        2 => {
            // A member twice.
            let member = rng.pick(&middle);
            let copy = text[member.start..member.end].to_string();
            let at = ms.last()?.start;
            m(
                "duplicate-member",
                splice(text, at, at, &copy),
                Expect::Refused(&["duplicate-key"]),
            )
        }
        3 => {
            // Two members swapped: the order carries no meaning.
            if middle.len() < 2 {
                return None;
            }
            let i = rng.below(middle.len());
            let j = (i + 1 + rng.below(middle.len() - 1)) % middle.len();
            let (a, b) = if middle[i].start < middle[j].start {
                (middle[i], middle[j])
            } else {
                (middle[j], middle[i])
            };
            let out = format!(
                "{}{}{}{}{}",
                &text[..a.start],
                &text[b.start..b.end],
                &text[a.end..b.start],
                &text[a.start..a.end],
                &text[b.end..]
            );
            m("reorder-members", out, Expect::Same)
        }
        4 => {
            // A record's (or a header member's) keys in another order.
            let mut objects: Vec<(usize, usize)> = Vec::new();
            for key in [
                "sections",
                "datums",
                "placements",
                "closures",
                "ignored_warnings",
            ] {
                objects.extend(rows(text, key));
            }
            objects.extend(header_rows.iter().copied());
            for key in ["project", "settings", "counters", "summary", "speleodb"] {
                if let Some(mm) = ms.iter().find(|mm| mm.key == key) {
                    let value_start = mm.start + key.len() + 3;
                    let value_end = text[..mm.end].trim_end_matches(['\n', ',']).len();
                    objects.push((value_start, value_end));
                }
            }
            let (start, end) = pick(rng, &objects)?;
            let value: serde_json::Value = serde_json::from_str(&text[start..end]).ok()?;
            // `serde_json::Map` sorts its keys: another order than the file's.
            let sorted = serde_json::to_string(&value).ok()?;
            if sorted == text[start..end] {
                return None;
            }
            m(
                "reorder-keys",
                splice(text, start, end, &sorted),
                Expect::Same,
            )
        }
        5 => {
            // Insignificant whitespace between tokens, the prefix included.
            let mut out = String::with_capacity(text.len() * 2);
            let ws = [" ", "\t", "\r\n", "  \n "];
            let mut in_string = false;
            let mut escaped = false;
            for (i, c) in text.char_indices() {
                out.push(c);
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if c == '\\' {
                        escaped = true;
                    } else if c == '"' {
                        in_string = false;
                    }
                    continue;
                }
                if c == '"' {
                    in_string = true;
                } else if matches!(c, ',' | ':' | '[' | '{') && (i < 40 || rng.chance(0.05)) {
                    out.push_str(ws[rng.below(ws.len())]);
                }
            }
            let out = if rng.chance(0.5) {
                out.replacen("{\"format\"", "{ \"format\"", 1)
            } else {
                out
            };
            m("whitespace", out, Expect::Same)
        }
        6 => {
            // A non-finite or out-of-range literal where a survey number goes.
            let numbers: Vec<(usize, usize)> = tokens(text)
                .into_iter()
                .filter(|t| t.2 == Tok::Number && SURVEY.contains(&member_at(text, t.0).as_str()))
                .map(|t| (t.0, t.1))
                .collect();
            let (start, end) = pick(rng, &numbers)?;
            let literal = [
                "NaN",
                "Infinity",
                "-Infinity",
                "1e400",
                "-1e999",
                "nan",
                "+1",
                ".5",
                "01",
            ][rng.below(9)];
            m(
                "non-finite",
                splice(text, start, end, literal),
                Expect::Refused(&["syntax", "bad-value"]),
            )
        }
        7 => {
            // An integer out of its range (where any integer is).
            let ints: Vec<(usize, usize)> = tokens(text)
                .into_iter()
                .filter(|t| {
                    t.2 == Tok::Number
                        && !text[t.0..t.1].contains(['.', 'e', 'E'])
                        && SURVEY.contains(&member_at(text, t.0).as_str())
                })
                .map(|t| (t.0, t.1))
                .collect();
            let (start, end) = pick(rng, &ints)?;
            let value = [
                "4294967296",
                "9007199254740993",
                "-1",
                "2147483648",
                "18446744073709551616",
                "1.5",
                "1e2",
                "-0",
                "2147483647",
            ][rng.below(9)];
            m(
                "int-out-of-range",
                splice(text, start, end, value),
                Expect::Any,
            )
        }
        8 => {
            // Deep nesting: unknown member, record key, history entry, local.
            let depth = [16, 64, 127, 128, 129, 1_000, 200_000][rng.below(7)];
            let end = ms.last()?.start;
            match rng.below(4) {
                0 => m(
                    "deep-unknown-member",
                    splice(text, end, end, &format!("\"zz_deep\":{},\n", nested(depth))),
                    if depth <= 64 {
                        Expect::Preserved("\"zz_deep\":")
                    } else if depth >= 1_000 {
                        Expect::Refused(&["too-deep"])
                    } else {
                        Expect::Any
                    },
                ),
                1 => {
                    let at = text.find(",\"start\":")?;
                    m(
                        "deep-record-key",
                        splice(text, at, at, &format!(",\"zz\":{}", nested(depth))),
                        Expect::ReadOnly,
                    )
                }
                2 => {
                    let at = text.find("\"history\":{\"undo\":[\n")? + 19;
                    m(
                        "deep-history",
                        splice(text, at, at, &format!("{},\n", nested(depth))),
                        if depth >= 1_000 {
                            Expect::HistoryDropped
                        } else {
                            Expect::Any
                        },
                    )
                }
                _ => {
                    let at = text.find("\"local\":{")? + 9;
                    m(
                        "deep-local",
                        splice(text, at, at, &format!("\"zz\":{},", nested(depth))),
                        Expect::Any,
                    )
                }
            }
        }
        9 => {
            // A huge, escape-heavy station name; bad escapes; a lone surrogate.
            let names: Vec<(usize, usize)> = rows(text, "stations")
                .into_iter()
                .filter_map(|r| {
                    let t = tokens(&text[r.0..r.1]);
                    t.iter()
                        .find(|t| t.2 == Tok::Str)
                        .map(|t| (r.0 + t.0, r.0 + t.1))
                })
                .collect();
            let (start, end) = pick(rng, &names)?;
            match rng.below(4) {
                0 => {
                    let unit = ["\\\"", "\\\\", "\\u0001", "\u{1f987}", "a", "\\n"][rng.below(6)];
                    let n = [1_000, 100_000, 400_000][rng.below(3)];
                    m(
                        "huge-string",
                        splice(text, start, end, &format!("\"{}\"", unit.repeat(n))),
                        Expect::Any,
                    )
                }
                1 => m(
                    "bad-escape",
                    splice(
                        text,
                        start,
                        end,
                        ["\"\\x41\"", "\"\\u12\"", "\"\\\"", "\"a\\\""][rng.below(4)],
                    ),
                    Expect::Refused(&["syntax", "bad-value", "truncated"]),
                ),
                2 => m(
                    "lone-surrogate",
                    splice(text, start, end, "\"\\ud800x\""),
                    Expect::Refused(&["syntax", "bad-value"]),
                ),
                _ => {
                    // Over the size cap.
                    let mut mutant = m(
                        "too-large",
                        text.to_string(),
                        Expect::Refused(&["too-large"]),
                    );
                    mutant.options.max_file_bytes = (text.len() / 2) as u64;
                    mutant
                }
            }
        }
        10 => {
            // Another version or solver.
            if rng.chance(0.6) {
                let version = [
                    "1",
                    "0",
                    "3",
                    "99",
                    "\"2\"",
                    "2.0",
                    "-2",
                    "1e3",
                    "99999999999999999999",
                ][rng.below(9)];
                let at = text.find("\"version\":")? + 10;
                m(
                    "version",
                    splice(text, at, at + 1, version),
                    Expect::Refused(&[
                        "newer-version",
                        "unsupported-version",
                        "not-stalx",
                        "syntax",
                    ]),
                )
            } else {
                let at = text.find("\"solver\":")? + 9;
                match rng.below(3) {
                    0 => m(
                        "solver",
                        splice(text, at, at + 1, ["0", "2", "99"][rng.below(3)]),
                        Expect::OtherSolver,
                    ),
                    1 => m(
                        "solver-not-a-number",
                        splice(text, at, at + 1, "\"1\""),
                        Expect::Refused(&["syntax", "bad-value"]),
                    ),
                    _ => m(
                        "solver-huge",
                        splice(text, at, at + 1, "18446744073709551616"),
                        Expect::Refused(&["syntax", "bad-value"]),
                    ),
                }
            }
        }
        11 => {
            // Required features.
            let at = text.find("\"requires\":[]")?;
            let span = (at, at + 13);
            match rng.below(3) {
                0 => m(
                    "requires-unknown",
                    splice(text, span.0, span.1, "\"requires\":[\"zz-future\"]"),
                    Expect::ReadOnly,
                ),
                1 => m(
                    "requires-not-strings",
                    splice(text, span.0, span.1, "\"requires\":[1]"),
                    Expect::Refused(&["syntax", "bad-value"]),
                ),
                _ => m(
                    "requires-two",
                    splice(text, span.0, span.1, "\"requires\":[\"zz-b\",\"zz-a\"]"),
                    Expect::ReadOnly,
                ),
            }
        }
        12 => {
            // A bad shot uuid.
            let (start, end) = pick(rng, &shot_rows)?;
            let uuid = tokens(&text[start..end])
                .first()
                .map(|t| (start + t.0, start + t.1))?;
            let other = shot_rows.iter().find(|r| r.0 != start).and_then(|r| {
                tokens(&text[r.0..r.1])
                    .first()
                    .map(|t| text[r.0 + t.0..r.0 + t.1].to_string())
            });
            let value = match rng.below(5) {
                0 => "\"\"".to_string(),
                1 => format!("\"{}\"", "u".repeat(129)),
                2 => "123".to_string(),
                3 => "null".to_string(),
                _ => other?,
            };
            m(
                "bad-uuid",
                splice(text, uuid.0, uuid.1, &value),
                Expect::Refused(&["invalid", "bad-value", "syntax"]),
            )
        }
        13 => {
            let stations = rows(text, "stations");
            let (start, end) = pick(rng, &stations)?;
            let row = text[start..end].to_string();
            let at = stations.last()?.1;
            m(
                "duplicate-station",
                splice(text, at, at, &format!(",\n{row}")),
                Expect::Refused(&["duplicate-station", "invalid", "bad-value"]),
            )
        }
        14 => {
            // A dangling reference.
            let mut targets: Vec<(usize, usize)> = Vec::new();
            match rng.below(7) {
                0 => targets.extend(shot_rows.iter().filter_map(|r| shot_to(text, *r))),
                1 => targets.extend(
                    header_rows
                        .iter()
                        .filter_map(|r| int_after(text, *r, "\"start\":")),
                ),
                2 => {
                    for r in rows(text, "datums") {
                        targets.extend(int_after(text, r, "{\"station\":"));
                        targets.extend(int_after(text, r, "\"via\":"));
                    }
                }
                3 => targets.extend(
                    rows(text, "placements")
                        .into_iter()
                        .filter_map(|r| int_after(text, r, "{\"station\":")),
                ),
                4 => {
                    for r in rows(text, "closures") {
                        targets.extend(int_after(text, r, "\"a\":"));
                        targets.extend(int_after(text, r, "\"b\":"));
                        targets.extend(int_after(text, r, "_via\":"));
                    }
                }
                5 => targets.extend(
                    header_rows
                        .iter()
                        .filter_map(|r| int_after(text, *r, "\"section\":")),
                ),
                _ => targets.extend(
                    header_rows
                        .iter()
                        .filter_map(|r| int_after(text, *r, "\"hand\":[[")),
                ),
            }
            let (start, end) = pick(rng, &targets)?;
            m(
                "dangling",
                splice(text, start, end, DANGLING),
                Expect::Refused(&["invalid", "bad-value"]),
            )
        }
        15 => {
            // A line of one shot between two stations of one chain: a loop.
            let header = pick(rng, &header_rows)?;
            let start = int_after(text, header, "\"start\":")?;
            let first_shot = line_rows
                .iter()
                .position(|r| *r == header)
                .and_then(|i| line_rows.get(i + 1))?;
            let to = shot_to(text, *first_shot)?;
            let row = format!(
                ",\n{{\"id\":42000,\"color\":\"#000\",\"direction\":\"in\",\"quality\":1.0,\"start\":{}}},\n[\"zz-loop\",{},3.0,0.0,0.0,0.0,0.0]",
                &text[start.0..start.1],
                &text[to.0..to.1]
            );
            let at = line_rows.last()?.1;
            m(
                "cycle",
                splice(text, at, at, &row),
                Expect::Refused(&["invalid"]),
            )
        }
        16 => {
            // A closure to a station that does not exist.
            let station = rows(text, "stations").first().and_then(|r| {
                let line = &text[r.0 + 1..r.1];
                line.split(',').next().map(str::to_string)
            })?;
            let closure = format!(
                "{{\"id\":999999,\"a\":{station},\"b\":{DANGLING},\"strategy\":\"batch\"}}"
            );
            let out = match rows(text, "closures").last() {
                Some(&(_, at)) => splice(text, at, at, &format!(",\n{closure}")),
                None => {
                    let at = ms
                        .iter()
                        .find(|mm| {
                            matches!(
                                mm.key.as_str(),
                                "ignored_warnings" | "imports" | "history" | "end"
                            )
                        })?
                        .start;
                    splice(text, at, at, &format!("\"closures\":[\n{closure}\n],\n"))
                }
            };
            m(
                "closure-unknown-station",
                out,
                Expect::Refused(&["invalid"]),
            )
        }
        17 => {
            // An anchor removed or doubled.
            let placements = rows(text, "placements");
            let (start, end) = pick(rng, &placements)?;
            if rng.chance(0.5) {
                m(
                    "placement-removed",
                    delete_row(text, start, end),
                    Expect::Refused(&["invalid"]),
                )
            } else {
                let row = text[start..end].to_string();
                m(
                    "placement-doubled",
                    splice(text, end, end, &format!(",\n{row}")),
                    Expect::Refused(&["invalid"]),
                )
            }
        }
        18 => {
            // Any row duplicated or deleted.
            let mut all: Vec<(usize, usize)> = Vec::new();
            for mm in &ms {
                all.extend(rows(text, &mm.key));
            }
            let (start, end) = pick(rng, &all)?;
            if rng.chance(0.5) {
                let row = text[start..end].to_string();
                m(
                    "row-duplicated",
                    splice(text, end, end, &format!(",\n{row}")),
                    Expect::Any,
                )
            } else {
                m("row-deleted", delete_row(text, start, end), Expect::Any)
            }
        }
        19 => {
            // Unknown content: in a record (read-only) or at the top (kept).
            if rng.chance(0.5) {
                let mut objects: Vec<usize> = Vec::new();
                for key in [
                    "datums",
                    "placements",
                    "closures",
                    "sections",
                    "ignored_warnings",
                ] {
                    objects.extend(rows(text, key).into_iter().map(|r| r.0 + 1));
                }
                objects.extend(header_rows.iter().map(|r| r.0 + 1));
                if objects.is_empty() {
                    return None;
                }
                let at = *rng.pick(&objects);
                m(
                    "unknown-record-key",
                    splice(text, at, at, "\"zz_future\":{\"a\":1},"),
                    Expect::ReadOnly,
                )
            } else {
                let at = ms
                    .iter()
                    .find(|mm| mm.key == "history" || mm.key == "end")?
                    .start;
                m(
                    "unknown-member",
                    splice(text, at, at, "\"zz_future\":{\"a\":[1,2.50,\"s\"]},\n"),
                    Expect::Preserved("\"zz_future\":{\"a\":[1,2.50,\"s\"]}"),
                )
            }
        }
        20 => {
            if rng.chance(0.5) {
                m("bom", format!("\u{feff}{text}"), Expect::Refused(&["bom"]))
            } else {
                let mut bytes = b"SQLite format 3\0".to_vec();
                bytes.extend_from_slice(text.as_bytes());
                Mutant {
                    op: "sqlite-magic",
                    bytes,
                    expect: Expect::Refused(&["legacy"]),
                    options: DecodeOptions::default(),
                }
            }
        }
        21 => {
            // A history flood (dropped, not refused) or an over-long line
            // (refused): small claims, bounded work.
            if rng.chance(0.5) {
                let at = text.find("\n\"end\":true")? + 1;
                let entry = "{\"t\":\"2026-01-01T00:00:00.000000000Z\"}";
                let flood = std::iter::repeat_n(entry, 70_000)
                    .collect::<Vec<_>>()
                    .join(",\n");
                let out = match ms.iter().find(|mm| mm.key == "history") {
                    Some(h) => splice(
                        text,
                        h.start,
                        h.end,
                        &format!("\"history\":{{\"undo\":[\n{flood}\n]}},\n"),
                    ),
                    None => splice(
                        text,
                        at,
                        at,
                        &format!("\"history\":{{\"undo\":[\n{flood}\n]}},\n"),
                    ),
                };
                m("history-flood", out, Expect::HistoryDropped)
            } else {
                let header = pick(rng, &header_rows)?;
                let start = int_after(text, header, "\"start\":")?;
                let mut extra = String::with_capacity(4_000_000);
                let base: u64 = 1_500_000_000;
                let _ = write!(
                    extra,
                    ",\n{{\"id\":42001,\"color\":\"#000\",\"direction\":\"in\",\"quality\":1.0,\"start\":{}}}",
                    &text[start.0..start.1]
                );
                for k in 0..100_000u64 {
                    let _ = write!(extra, ",\n[\"zz{k}\",{},1.0,0.0,0.0,0.0,0.0]", base + k);
                }
                let at = line_rows.last()?.1;
                m(
                    "line-over-the-cap",
                    splice(text, at, at, &extra),
                    Expect::Refused(&["invalid", "too-complex"]),
                )
            }
        }
        _ => {
            // Byte flips.
            let mut bytes = text.as_bytes().to_vec();
            for _ in 0..1 + rng.below(4) {
                let at = rng.below(bytes.len());
                bytes[at] = [
                    0x00, 0xff, b'"', b'\\', b'{', b'}', b'[', b']', b',', b':', b'0', b'9', b'-',
                    b'e', b'n', b' ', b'\n', 0xc3,
                ][rng.below(18)];
            }
            Mutant {
                op: "byte-flips",
                bytes,
                expect: Expect::Any,
                options: DecodeOptions::default(),
            }
        }
    })
}

/// How many targeted operators there are (the byte flips are the last).
const OPERATORS: usize = 23;

/// Run `f`, refusing a panic or an overlong run.
fn bounded<T>(what: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    let started = Instant::now();
    let out = catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        let text = p
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        format!("{what} panicked: {text}")
    })?;
    let took = started.elapsed();
    if took > TIME_BOUND {
        return Err(format!("{what} took {took:?}"));
    }
    Ok(out)
}

/// The generic properties of an accepted file: valid, and either writable
/// and canonical (a fixpoint from its first re-encoding), or read-only and
/// refusing every encode while a local-only rewrite keeps the survey.
fn accepted(input: &[u8], d: &Decoded) -> Result<Option<Vec<u8>>, String> {
    if let Some(v) = d.project.validate().first() {
        return Err(format!("accepted an invalid survey: {v}"));
    }
    if d.read_only().is_some() {
        for mode in [Mode::Normal, Mode::DetachedCopy] {
            match encode(&d.project, &d.extras, mode) {
                Err(EncodeError::ReadOnly(_)) => {}
                other => return Err(format!("a read-only file encoded: {:?}", other.err())),
            }
        }
        if let Ok(out) = bounded("rewrite_local", || rewrite_local(input, &d.extras))? {
            let again = decode(&out, &DecodeOptions::default())
                .map_err(|e| format!("a local rewrite does not decode: {e}"))?;
            if again.read_only().is_none() || again.project != d.project {
                return Err("a local rewrite lost the survey or its read-only reason".to_string());
            }
        }
        return Ok(None);
    }
    let first = encode(&d.project, &d.extras, Mode::Normal)
        .map_err(|e| format!("an accepted file does not re-encode: {e}"))?;
    let again = decode(&first, &DecodeOptions::default())
        .map_err(|e| format!("the re-encoding does not decode: {e}"))?;
    if again.project != d.project {
        return Err("the re-encoding decodes to another project".to_string());
    }
    let second = encode(&again.project, &again.extras, Mode::Normal)
        .map_err(|e| format!("the second encode failed: {e}"))?;
    if second != first {
        return Err("the re-encoding is not a fixpoint".to_string());
    }
    Ok(Some(first))
}

/// Judge one mutant against the original (`base`, its decode `orig`).
fn judge(mutant: &Mutant, base: &[u8], orig: &Decoded) -> Result<String, String> {
    let input = &mutant.bytes;
    let _ = bounded("sniff", || sniff(input))?;
    let _ = bounded("probe", || probe_bytes(input))?;
    if input.len() < 1_000_000 {
        let (salvaged, _) = bounded("salvage", || decode_salvage(input))?;
        if let Some(p) = salvaged {
            if let Some(v) = p.validate().first() {
                return Err(format!("salvage returned an invalid survey: {v}"));
            }
        }
    }
    let result = bounded("decode", || decode(input, &mutant.options))?;
    let verdict = match &result {
        Ok(d) => {
            let encoded = accepted(input, d)?;
            if d.read_only().is_some() {
                "read-only".to_string()
            } else {
                let _ = encoded;
                "accepted".to_string()
            }
        }
        Err(e) => {
            if e.to_string().is_empty() {
                return Err(format!("a refusal without a message: {e:?}"));
            }
            format!("refused:{}", class(e))
        }
    };
    let same = |d: &Decoded| -> Result<(), String> {
        if d.project != orig.project || d.extras != orig.extras {
            return Err("decodes to another project or extras".to_string());
        }
        let bytes = encode(&d.project, &d.extras, Mode::Normal).map_err(|e| e.to_string())?;
        if bytes != base {
            return Err("re-encodes to other bytes than the original".to_string());
        }
        Ok(())
    };
    let short = |e: &DecodeError| {
        let mut s = format!("{e:?}");
        s.truncate(300);
        s
    };
    match (mutant.expect, &result) {
        (Expect::Any, _) => {}
        (Expect::Refused(classes), Err(e)) if classes.contains(&class(e)) => {}
        (Expect::Refused(classes), Err(e)) => {
            return Err(format!(
                "refused as {} ({}), expected one of {classes:?}",
                class(e),
                short(e)
            ))
        }
        (Expect::Refused(classes), Ok(_)) => {
            return Err(format!("accepted, expected a refusal of {classes:?}"))
        }
        (Expect::Same, Ok(d)) => same(d)?,
        (Expect::OtherSolver, Ok(d)) => {
            if !d
                .notes
                .iter()
                .any(|n| matches!(n, DecodeNote::OtherSolver { .. }))
            {
                return Err("another solver without its note".to_string());
            }
            let mut d = d.clone();
            d.notes.clear();
            same(&d)?;
        }
        (Expect::ReadOnly, Ok(d)) if d.read_only().is_some() => {}
        (Expect::ReadOnly, Ok(_)) => return Err("opened writable, expected read-only".to_string()),
        (Expect::Preserved(needle), Ok(d)) => {
            if d.read_only().is_some() {
                return Err("an unknown top-level member opened read-only".to_string());
            }
            let bytes = encode(&d.project, &d.extras, Mode::Normal).map_err(|e| e.to_string())?;
            if !String::from_utf8_lossy(&bytes).contains(needle) || d.project != orig.project {
                return Err("an unknown top-level member was not kept through a save".to_string());
            }
        }
        (Expect::HistoryDropped, Ok(d)) => {
            if !d
                .notes
                .iter()
                .any(|n| matches!(n, DecodeNote::HistoryDropped { .. }))
                || !d.extras.history.is_empty()
                || d.project != orig.project
            {
                return Err("an unreadable history was not dropped whole".to_string());
            }
        }
        (_, Err(e)) => {
            return Err(format!(
                "refused as {} ({}), expected it to open",
                class(e),
                short(e)
            ))
        }
    }
    Ok(verdict)
}

/// The base file of a seed.
fn base_of(seed: u64) -> Vec<u8> {
    let shape = match seed % 8 {
        0 => MEDIUM,
        1 | 2 => Shape {
            rich: false,
            ..SMALL
        },
        _ => SMALL,
    };
    let (p, extras) = generate(seed, shape);
    encode(&p, &extras, Mode::Normal).unwrap_or_default()
}

/// One seed: its operator, then the verdict. A second, random flip on top
/// sometimes (then anything may happen, as long as the generic properties
/// hold).
fn run_seed(seed: u64) -> Result<(&'static str, String), (&'static str, String)> {
    let base = base_of(seed);
    let text = String::from_utf8(base.clone()).map_err(|e| ("base", e.to_string()))?;
    let orig = decode(&base, &DecodeOptions::default()).map_err(|e| ("base", e.to_string()))?;
    let mut rng = Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) | 1);
    let which = (seed as usize) % OPERATORS;
    let Some(mut mutant) = mutate(&text, which, &mut rng) else {
        return Ok(("skipped", String::new()));
    };
    if which != OPERATORS - 1 && rng.chance(0.15) && !mutant.bytes.is_empty() {
        let at = rng.below(mutant.bytes.len());
        mutant.bytes[at] = [b'"', b'{', b'0', b',', 0xff][rng.below(5)];
        mutant.expect = Expect::Any;
        mutant.op = "operator-and-flip";
    }
    let op = mutant.op;
    judge(&mutant, &base, &orig)
        .map(|v| (op, v))
        .map_err(|e| (op, e))
}

#[derive(Default)]
struct Totals {
    seeds: u64,
    verdicts: BTreeMap<String, u64>,
    failures: Vec<(u64, &'static str, String)>,
}

fn run_window(start: u64, count: u64, threads: usize) -> Totals {
    let next = AtomicU64::new(start);
    let totals = Mutex::new(Totals::default());
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| loop {
                let seed = next.fetch_add(1, Ordering::Relaxed);
                if seed >= start + count {
                    break;
                }
                let outcome = run_seed(seed);
                let mut t = totals
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                t.seeds += 1;
                match outcome {
                    Ok((op, verdict)) => {
                        *t.verdicts.entry(format!("{op} {verdict}")).or_default() += 1
                    }
                    Err((op, why)) => t.failures.push((seed, op, why)),
                }
            });
        }
    });
    totals
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn report_and_assert(totals: &mut Totals, test: &str) {
    totals.failures.sort_by_key(|f| f.0);
    for (seed, op, why) in &totals.failures {
        eprintln!(
            "C24 decode fuzz FAILED seed={seed} op={op} replay=\"STALAGMIGHT_FUZZ_SEED_START={seed} \
             STALAGMIGHT_FUZZ_SEEDS=1 cargo test -p stalagmight-persistence --lib {TEST_PATH}::{test} \
             -- --include-ignored\" detail={}",
            why.replace('\n', " | ")
        );
    }
    eprintln!(
        "C24 decode fuzz: seeds={} failed={}",
        totals.seeds,
        totals.failures.len()
    );
    assert!(
        totals.failures.is_empty(),
        "{} seed(s) failed, see the lines above",
        totals.failures.len()
    );
}

#[test]
fn decode_fuzz_window() {
    let (start, count) = window(CI_SEEDS);
    let mut totals = run_window(start, count, 2);
    report_and_assert(&mut totals, "decode_fuzz_window");
    if env_u64("STALAGMIGHT_FUZZ_SEEDS").is_some() {
        return;
    }
    // Every operator was reached at least once, and every neutral edit
    // really was accepted (not skipped).
    let reached: Vec<&str> = totals
        .verdicts
        .keys()
        .filter_map(|k| k.split(' ').next())
        .collect();
    for op in [
        "truncate-row",
        "duplicate-member",
        "reorder-members",
        "reorder-keys",
        "whitespace",
        "non-finite",
        "int-out-of-range",
        "version",
        "bad-uuid",
        "duplicate-station",
        "dangling",
        "cycle",
        "closure-unknown-station",
        "byte-flips",
    ] {
        assert!(
            reached.contains(&op),
            "the window never ran {op}: {:#?}",
            totals.verdicts
        );
    }
}

/// `cargo test -p stalagmight-persistence --lib decode_fuzz_wide -- --ignored
/// --nocapture`: 2,000 seeds (or the env window) on `STALAGMIGHT_FUZZ_THREADS`
/// threads (default 4); prints every operator's verdicts.
#[test]
#[ignore = "wide run: 2,000 seeds by default; set STALAGMIGHT_FUZZ_SEED_START / STALAGMIGHT_FUZZ_SEEDS"]
fn decode_fuzz_wide() {
    let (start, count) = window(WIDE_SEEDS);
    let threads = env_u64("STALAGMIGHT_FUZZ_THREADS").map_or(4, |n| n as usize);
    let mut totals = run_window(start, count, threads);
    for (verdict, n) in &totals.verdicts {
        eprintln!("verdict: {verdict} = {n}");
    }
    report_and_assert(&mut totals, "decode_fuzz_wide");
}

/// Truncation at EVERY row boundary of a rich medium file (the operator
/// above samples them): never a project, always a typed truncation, except
/// the final newline alone.
#[test]
fn every_row_boundary_truncation_is_a_typed_refusal() {
    let base = base_of(8);
    let text = String::from_utf8(base.clone()).unwrap_or_default();
    let newlines: Vec<usize> = text.match_indices('\n').map(|(i, _)| i).collect();
    let last = text.len() - 1;
    for &at in &newlines {
        for cut in [at, at + 1] {
            let result = decode(&base[..cut.min(base.len())], &DecodeOptions::default());
            if cut >= last {
                assert!(result.is_ok(), "cut at {cut}: {:?}", result.err());
                continue;
            }
            match result {
                Err(
                    DecodeError::Truncated { .. }
                    | DecodeError::MissingEnd
                    | DecodeError::Syntax { .. },
                ) => {}
                other => panic!(
                    "cut at {cut} of {}: {:?}",
                    base.len(),
                    other.map(|d| d.project.name)
                ),
            }
        }
    }
}

/// A `version` that is not a number (`"2"`, `null`, `true`) is a version
/// this build never wrote, not a truncated file (fuzz seeds 1367 and 1942:
/// the sniff reported "the file ends early" for a complete file).
#[test]
fn a_version_that_is_not_a_number_is_unsupported_not_truncated() {
    for seed in [1367, 1942] {
        match run_seed(seed) {
            Ok(_) => {}
            Err((op, why)) => panic!("seed {seed} ({op}): {why}"),
        }
    }
    let base = String::from_utf8(base_of(3)).unwrap_or_default();
    for token in ["\"2\"", "null", "true", "\"\"", "[2]", "{}"] {
        let text = base.replacen("\"version\":2", &format!("\"version\":{token}"), 1);
        match decode(text.as_bytes(), &DecodeOptions::default()) {
            Err(DecodeError::UnsupportedVersion { found }) => assert!(!found.is_empty(), "{token}"),
            other => panic!("{token}: {:?}", other.map(|d| d.project.name)),
        }
        // A local-only rewrite of such a file is refused the same way.
        assert!(
            rewrite_local(text.as_bytes(), &Extras::default()).is_err(),
            "{token}"
        );
    }
    // A file cut inside the version token is still truncated.
    let cut = &base.as_bytes()[..base.find("\"version\":").unwrap_or(0) + 11];
    assert!(matches!(
        decode(cut, &DecodeOptions::default()),
        Err(DecodeError::Truncated { .. })
    ));
}
