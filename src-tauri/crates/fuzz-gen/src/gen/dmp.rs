//! mNemo `.dmp` generator: a semicolon-separated list of byte values holding
//! sections of fixed-size shot records (format versions 2 to 5), each closed
//! by its version's end-of-section record.
//!
//! Semantic knobs: the version (including unsupported ones and a version that
//! differs between sections), header dates at and past their limits, shot
//! types outside the enum, big-endian 16-bit readings at their extremes
//! (negative lengths and depths, 0x7FFF / 0x8000), missing or forged
//! start/end magics, a missing end record (an interrupted download), values
//! written outside 0..=255 (the reader keeps the low byte) and odd spellings
//! of the integers themselves.

use crate::fileset::FileSet;
use crate::gen::{count, damage, line_endings, Faults};
use crate::source::ByteSource;

const SECTION_MAGIC: [u8; 3] = [68, 89, 101];
const SHOT_START_MAGIC: [u8; 3] = [57, 67, 77];
const SHOT_END_MAGIC: [u8; 3] = [95, 25, 35];

fn shot_size(version: u8) -> usize {
    match version {
        2 => 16,
        3 => 21,
        4 => 29,
        _ => 35,
    }
}

fn push_i16(out: &mut Vec<u8>, v: i16) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// A 16-bit reading: usually `value * scale` of a plausible value, sometimes
/// an extreme.
fn reading16(src: &mut ByteSource<'_>, lo: f64, hi: f64, scale: f64) -> i16 {
    if src.chance(16) {
        return src.pick_or(&[i16::MAX, i16::MIN, -1, 0, -100, 1, 32_000, -32_000], 0);
    }
    (src.f64_in(lo, hi) * scale)
        .round()
        .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
}

fn shot(src: &mut ByteSource<'_>, f: Faults, version: u8, out: &mut Vec<u8>) {
    let start = out.len();
    if version >= 5 {
        if f.hit(src, 6) {
            out.extend_from_slice(src.bytes(3));
        } else {
            out.extend_from_slice(&SHOT_START_MAGIC);
        }
    }
    // Mostly standard shots, some control-station records, rarely garbage.
    let kind = match src.below(16) {
        0 => 0,
        1 => 1,
        2 if f.hit(src, 32) => src.u8(),
        _ => 2,
    };
    out.push(kind);
    push_i16(out, reading16(src, 0.0, 360.0, 10.0));
    push_i16(out, reading16(src, 0.0, 360.0, 10.0));
    push_i16(out, reading16(src, 0.0, 60.0, 100.0));
    push_i16(out, reading16(src, 0.0, 120.0, 100.0));
    push_i16(out, reading16(src, 0.0, 120.0, 100.0));
    push_i16(out, reading16(src, -90.0, 90.0, 10.0));
    push_i16(out, reading16(src, -90.0, 90.0, 10.0));
    if version >= 4 {
        for _ in 0..4 {
            push_i16(out, reading16(src, 0.0, 20.0, 100.0));
        }
    }
    if version >= 3 {
        push_i16(out, reading16(src, -5.0, 40.0, 10.0));
        let (h, m, s) = if src.chance(16) {
            (src.u8(), src.u8(), src.u8())
        } else {
            (
                src.below(24) as u8,
                src.below(60) as u8,
                src.below(60) as u8,
            )
        };
        out.extend_from_slice(&[h, m, s]);
    }
    out.push(if src.chance(16) { src.u8() } else { 0 });
    if version >= 5 {
        if f.hit(src, 6) {
            out.extend_from_slice(src.bytes(3));
        } else {
            out.extend_from_slice(&SHOT_END_MAGIC);
        }
    }
    // Keep the record its version's size unless a forged magic changed it on
    // purpose (a short or long record is its own case).
    let want = shot_size(version);
    let got = out.len() - start;
    if got < want && !f.hit(src, 8) {
        out.resize(start + want, 0);
    } else if got > want && !f.hit(src, 8) {
        out.truncate(start + want);
    }
}

fn end_record(version: u8, out: &mut Vec<u8>, buggy: bool) {
    if version >= 5 {
        out.extend_from_slice(&SHOT_START_MAGIC);
        out.push(3);
        if buggy {
            out.extend_from_slice(&[7, 8, 7, 8]);
            out.extend_from_slice(&[0; 24]);
        } else {
            out.extend_from_slice(&[0; 28]);
        }
        out.extend_from_slice(&SHOT_END_MAGIC);
    } else {
        out.push(3);
        out.extend(std::iter::repeat_n(0u8, shot_size(version) - 1));
    }
}

fn section(src: &mut ByteSource<'_>, f: Faults, version: u8, out: &mut Vec<u8>) {
    out.push(version);
    if version > 2 {
        if f.hit(src, 6) {
            out.extend_from_slice(src.bytes(3));
        } else {
            out.extend_from_slice(&SECTION_MAGIC);
        }
    }
    // Year (offset from 2000), month, day, hour, minute: valid mostly, then
    // the edges each check guards.
    if f.hit(src, 24) {
        for _ in 0..5 {
            out.push(src.pick_or(
                &[
                    0, 1, 12, 13, 23, 24, 28, 29, 30, 31, 32, 59, 60, 99, 100, 255,
                ],
                0,
            ));
        }
    } else {
        out.extend_from_slice(&[
            src.below(100) as u8,
            1 + src.below(12) as u8,
            1 + src.below(28) as u8,
            src.below(24) as u8,
            src.below(60) as u8,
        ]);
    }
    // Three name bytes: letters usually, sometimes bytes that are not UTF-8.
    for _ in 0..3 {
        out.push(if f.hit(src, 24) {
            src.u8()
        } else {
            b'A' + src.below(26) as u8
        });
    }
    out.push(if f.hit(src, 8) {
        src.u8()
    } else {
        src.below(2) as u8
    });
    let shots = count(src, 12, 3000);
    for _ in 0..shots {
        shot(src, f, version, out);
    }
    match src.below(16) {
        // An interrupted download: no end record.
        0 => {}
        1 => end_record(version, out, true),
        _ => end_record(version, out, false),
    }
}

/// Render one `.dmp` import.
pub fn generate(src: &mut ByteSource<'_>) -> FileSet {
    let f = Faults::pick(src);
    let version = if f.hit(src, 8) {
        src.u8()
    } else {
        2 + src.below(4) as u8
    };
    let mut bytes = Vec::new();
    let sections = 1 + count(src, 3, 40);
    for _ in 0..sections {
        // Rarely a section claims another version than the file.
        let v = if f.hit(src, 6) {
            2 + src.below(4) as u8
        } else {
            version
        };
        section(src, f, v, &mut bytes);
    }
    let mut text = String::with_capacity(bytes.len() * 4);
    for b in bytes {
        let field = match if f.on() { src.below(64) } else { 64 } {
            // The reader keeps the low byte of an i32: the same byte spelled
            // out of range must read the same.
            0 => format!("{}", i32::from(b) + 256),
            1 => format!("{}", i32::from(b) - 256),
            2 => format!(" {b} "),
            3 => format!("+{b}"),
            4 => format!("0{b}"),
            5 => (*src
                .pick(&["", "x", "1.5", "99999999999", "-", "\u{0}"])
                .unwrap_or(&""))
            .to_string(),
            _ => b.to_string(),
        };
        text.push_str(&field);
        text.push(';');
    }
    if src.chance(64) {
        text.push('\n');
    }
    let text = line_endings(src, &text);
    FileSet::single("survey.dmp", damage(src, text.into_bytes()))
}
