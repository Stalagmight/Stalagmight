//! Structure-aware generators: one per importer, each rendering a valid-ish
//! survey file from a [`ByteSource`], plus the semantic mutators they share.
//!
//! A generator starts from a well-formed file of the format (so the input
//! reaches the deep parts of the importer, not just its first syntax check)
//! and lets the source bend the parts that went wrong in real files: units and
//! unit-scaled sentinels (finding HFI-1 was a `-1` "inherit depth" marker that
//! a feet-to-metre conversion turned into `-0.3048`), declinations, number
//! spellings, encodings and byte-order marks, line endings, huge declared
//! counts, deep nesting and truncation.

pub mod compass;
pub mod dmp;
pub mod gpx;
pub mod tml;
pub mod walls;

use crate::fileset::FileSet;
use crate::oracle::Format;
use crate::source::ByteSource;

/// Render one import of `format` from `src`.
pub fn generate(format: Format, src: &mut ByteSource<'_>) -> FileSet {
    match format {
        Format::Dmp => dmp::generate(src),
        Format::Tml => tml::generate(src, true),
        Format::Tmlu => tml::generate(src, false),
        Format::Srv => walls::generate_srv(src),
        Format::Wpj => walls::generate_wpj(src),
        Format::Dat => compass::generate_dat(src),
        Format::Mak => compass::generate_mak(src),
        Format::Gpx => gpx::generate(src),
    }
}

/// Metres per foot (exact, the international foot).
pub const FEET_TO_M: f64 = 0.3048;

/// A length unit a survey file can declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LengthUnit {
    Meters,
    Feet,
}

impl LengthUnit {
    pub fn pick(src: &mut ByteSource<'_>) -> Self {
        if src.bool() {
            Self::Feet
        } else {
            Self::Meters
        }
    }

    /// `metres` expressed in this unit.
    pub fn from_meters(self, metres: f64) -> f64 {
        match self {
            Self::Meters => metres,
            Self::Feet => metres / FEET_TO_M,
        }
    }
}

/// A "no value / inherit" marker as files in the wild spell it, including the
/// unit-scaled forms a converting tool leaves behind (the HFI-1 class): `-1`
/// itself, `-1` scaled feet->metres and metres->feet, the classic `-999` /
/// `-9.9` / `-9999` placeholders, and zero.
pub fn sentinel(src: &mut ByteSource<'_>) -> f64 {
    const SENTINELS: &[f64] = &[
        -1.0,
        -FEET_TO_M,
        -1.0 / FEET_TO_M,
        -999.0,
        -999.0 * FEET_TO_M,
        -9.9,
        -9.9 / FEET_TO_M,
        -9999.0,
        0.0,
        -0.0,
        -1.0e-6,
    ];
    src.pick_or(SENTINELS, -1.0)
}

/// A value for a numeric field: usually a plausible reading in `lo..=hi`,
/// sometimes a sentinel, sometimes an edge value (see [`ByteSource::number`]).
pub fn reading(src: &mut ByteSource<'_>, lo: f64, hi: f64) -> f64 {
    if src.chance(12) {
        sentinel(src)
    } else {
        src.number(lo, hi)
    }
}

/// Spell `v` the way some writer might: fixed decimals, many decimals,
/// scientific notation, a leading `+`, padded zeros, a trailing dot, a comma
/// decimal separator, or the textual non-finite spellings.
pub fn spell(src: &mut ByteSource<'_>, v: f64) -> String {
    if !src.chance(48) {
        return format!("{v:.2}");
    }
    match src.below(10) {
        0 => format!("{v}"),
        1 => format!("{v:.6}"),
        2 => format!("{v:e}"),
        3 => format!("+{v:.1}"),
        4 => format!("{v:08.3}"),
        5 => format!("{}.", v.trunc()),
        6 => format!("{v:.2}").replace('.', ","),
        7 => format!("{v:.0}"),
        8 => (*src
            .pick(&[
                "NaN", "nan", "inf", "-inf", "Infinity", "", "-", ".", "1e999", "0x10", "1_000",
            ])
            .unwrap_or(&"NaN"))
        .to_string(),
        _ => format!("{v:.40}"),
    }
}

/// A text encoding to write a rendered file in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    /// Windows-1252 (Latin-1 range written as single bytes; others as `?`).
    Cp1252,
    Utf16LeBom,
    Utf16BeBom,
}

impl Encoding {
    /// Mostly UTF-8; the others often enough to keep their paths covered.
    pub fn pick(src: &mut ByteSource<'_>) -> Self {
        match src.below(10) {
            0 => Self::Utf8Bom,
            1 => Self::Cp1252,
            2 => Self::Utf16LeBom,
            3 => Self::Utf16BeBom,
            _ => Self::Utf8,
        }
    }

    pub fn encode(self, text: &str) -> Vec<u8> {
        match self {
            Self::Utf8 => text.as_bytes().to_vec(),
            Self::Utf8Bom => [&[0xEF, 0xBB, 0xBF][..], text.as_bytes()].concat(),
            Self::Cp1252 => text
                .chars()
                .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
                .collect(),
            Self::Utf16LeBom => {
                let mut out = vec![0xFF, 0xFE];
                for unit in text.encode_utf16() {
                    out.extend_from_slice(&unit.to_le_bytes());
                }
                out
            }
            Self::Utf16BeBom => {
                let mut out = vec![0xFE, 0xFF];
                for unit in text.encode_utf16() {
                    out.extend_from_slice(&unit.to_be_bytes());
                }
                out
            }
        }
    }
}

/// Rewrite line endings: `\n` (kept), `\r\n`, a lone `\r`, or a mix.
pub fn line_endings(src: &mut ByteSource<'_>, text: &str) -> String {
    match src.below(8) {
        0 => text.replace('\n', "\r\n"),
        1 => text.replace('\n', "\r"),
        2 => {
            let mut out = String::with_capacity(text.len() + 16);
            for (i, line) in text.split('\n').enumerate() {
                if i > 0 {
                    out.push_str(if i % 3 == 0 { "\r\n" } else { "\n" });
                }
                out.push_str(line);
            }
            out
        }
        _ => text.to_string(),
    }
}

/// Structural damage applied to a rendered file, rarely: truncation at any
/// byte, a dropped or repeated region, a run of junk, a few flipped bytes. The
/// bulk of the inputs stay intact so the deep paths stay reachable.
pub fn damage(src: &mut ByteSource<'_>, mut bytes: Vec<u8>) -> Vec<u8> {
    if !src.chance(40) || bytes.is_empty() {
        return bytes;
    }
    let len = bytes.len();
    let at = src.below(len);
    match src.below(6) {
        0 => bytes.truncate(at),
        1 => {
            let end = (at + 1 + src.below(64)).min(len);
            bytes.drain(at..end);
        }
        2 => {
            let end = (at + 1 + src.below(256)).min(len);
            let region = bytes.get(at..end).unwrap_or(&[]).to_vec();
            let times = 1 + src.below(8);
            for _ in 0..times {
                bytes.splice(at..at, region.iter().copied());
            }
        }
        3 => {
            let junk = src.bytes(32).to_vec();
            bytes.splice(at..at, junk);
        }
        4 => {
            for _ in 0..=src.below(4) {
                let i = src.below(bytes.len());
                if let Some(b) = bytes.get_mut(i) {
                    *b ^= 1 << src.below(8);
                }
            }
        }
        _ => {
            // A NUL or a stray high byte in the middle of the text.
            let b = src.pick_or(&[0x00, 0x81, 0xC3, 0xFF, 0x1A], 0);
            bytes.insert(at, b);
        }
    }
    bytes
}

/// Per-file switch for local faults (a bad attribute, an odd spelling, a stray
/// byte in one record). Decided once per file: with per-record odds alone, a
/// file of thousands of records would almost never come out valid, and the
/// importer's deep paths (conversion, apply) would go unexercised. About one
/// file in four is "hostile"; the rest are well formed apart from their
/// semantic choices (units, sentinels, encodings, counts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Faults {
    on: bool,
}

impl Faults {
    pub fn pick(src: &mut ByteSource<'_>) -> Self {
        Self { on: src.chance(64) }
    }

    /// Never fires.
    pub fn none() -> Self {
        Self { on: false }
    }

    pub fn on(self) -> bool {
        self.on
    }

    /// True with probability `num`/256 in a hostile file, never otherwise.
    /// Consumes the same decision either way, so a file's layout does not
    /// shift when its hostility flips.
    pub fn hit(self, src: &mut ByteSource<'_>, num: u8) -> bool {
        src.chance(num) && self.on
    }
}

/// A count that is usually small but sometimes large enough to stress loops
/// and allocations (`small_max` normally, up to `big_max` rarely).
pub fn count(src: &mut ByteSource<'_>, small_max: usize, big_max: usize) -> usize {
    if src.chance(6) {
        src.below(big_max.saturating_add(1))
    } else {
        src.below(small_max.saturating_add(1))
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::{generate, Encoding};
    use crate::oracle::Format;
    use crate::source::{seed_bytes, ByteSource};

    #[test]
    fn generators_are_deterministic_and_produce_the_entry_file() {
        for format in Format::ALL {
            for seed in 0..8 {
                let bytes = seed_bytes(seed, 4096);
                let a = generate(format, &mut ByteSource::new(&bytes));
                let b = generate(format, &mut ByteSource::new(&bytes));
                assert_eq!(a, b, "{format:?} seed {seed} is not deterministic");
                assert_eq!(a.entry_name(), format.entry_name(), "{format:?} entry name");
            }
            // A dry source still renders a file.
            let dry = generate(format, &mut ByteSource::new(&[]));
            assert!(!dry.files.is_empty(), "{format:?} dry");
        }
    }

    #[test]
    fn encodings_write_their_byte_order_marks() {
        assert_eq!(
            Encoding::Utf8Bom.encode("a").first().copied().unwrap(),
            0xEF
        );
        assert_eq!(Encoding::Utf16LeBom.encode("a"), vec![0xFF, 0xFE, b'a', 0]);
        assert_eq!(Encoding::Cp1252.encode("\u{e9}\u{20ac}"), vec![0xE9, b'?']);
    }
}
