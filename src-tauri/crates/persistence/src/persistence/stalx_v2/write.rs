//! Scalar writers with the format's canonical spellings, and the small object
//! builder the encoder uses. Every byte the encoder emits goes through here,
//! so the canonical rules live in one place:
//!
//! - strings: UTF-8, escaped minimally (`"`, `\`, and U+0000-U+001F; `\b`,
//!   `\f`, `\n`, `\r`, `\t` short, the rest `\u00xx` lowercase);
//! - floats: finite only, the shortest text that reads back to the same bits
//!   (ryu), `-0.0` kept, whole numbers with `.0`;
//! - integers: decimal, at most [`MAX_INTEGER`];
//! - times: RFC 3339 UTC with nine fractional digits and `Z`, after the #810
//!   clamp into years 0001-9999;
//! - dates: `YYYY-MM-DD`.

use std::io::Write as _;

use chrono::{DateTime, NaiveDate, SecondsFormat, Timelike as _, Utc};

use super::error::EncodeError;
use super::MAX_INTEGER;

/// 0001-01-01T00:00:00Z, the first instant every reader parses.
const FIRST_SECOND: i64 = -62_135_596_800;
/// 9999-12-31T23:59:59Z, the last second every reader parses.
const LAST_SECOND: i64 = 253_402_300_799;

/// `at` moved into what every reader parses: years 0001-9999 in UTC and no
/// leap second (the #810 rule of `sqlite/stored_times.rs`, which this format
/// shares). Any other time is unchanged.
pub(super) fn normalized_time(at: DateTime<Utc>) -> DateTime<Utc> {
    let (Some(first), Some(last)) = (
        DateTime::from_timestamp(FIRST_SECOND, 0),
        DateTime::from_timestamp(LAST_SECOND, 999_999_999),
    ) else {
        return at;
    };
    let at = at.clamp(first, last);
    if at.nanosecond() >= 1_000_000_000 {
        return at.with_nanosecond(999_999_999).unwrap_or(at);
    }
    at
}

/// The stored text of a time: [`normalized_time`], nine fractional digits, `Z`.
pub(super) fn time_text(at: DateTime<Utc>) -> String {
    normalized_time(at).to_rfc3339_opts(SecondsFormat::Nanos, true)
}

/// Parse a stored time (any RFC 3339 offset), [`normalized_time`].
pub(super) fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|t| normalized_time(t.with_timezone(&Utc)))
}

/// Parse a stored date (`YYYY-MM-DD`; a sign before years outside 0-9999).
pub(super) fn parse_date(text: &str) -> Option<NaiveDate> {
    text.parse::<NaiveDate>().ok().map(normalized_date)
}

/// `day` clamped into 0001-01-01 ..= 9999-12-31, the dates every reader
/// parses as `YYYY-MM-DD` (the dates' version of the #810 rule).
pub(super) fn normalized_date(day: NaiveDate) -> NaiveDate {
    match (
        NaiveDate::from_ymd_opt(1, 1, 1),
        NaiveDate::from_ymd_opt(9999, 12, 31),
    ) {
        (Some(first), Some(last)) => day.clamp(first, last),
        _ => day,
    }
}

/// A JSON string, escaped the canonical way.
pub(super) fn string(out: &mut Vec<u8>, value: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    let bytes = value.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let short = match b {
            b'"' => b'"',
            b'\\' => b'\\',
            0x08 => b'b',
            0x0c => b'f',
            b'\n' => b'n',
            b'\r' => b'r',
            b'\t' => b't',
            0x00..=0x1f => b'u',
            _ => continue,
        };
        out.extend_from_slice(&bytes[start..i]);
        out.push(b'\\');
        out.push(short);
        if short == b'u' {
            out.extend_from_slice(b"00");
            out.push(HEX[usize::from(b >> 4)]);
            out.push(HEX[usize::from(b & 0x0f)]);
        }
        start = i + 1;
    }
    out.extend_from_slice(&bytes[start..]);
    out.push(b'"');
}

/// A finite float in its shortest round-trip spelling, or the error that
/// names where the non-finite value is.
pub(super) fn float(
    out: &mut Vec<u8>,
    value: f64,
    at: impl FnOnce() -> String,
) -> Result<(), EncodeError> {
    if !value.is_finite() {
        return Err(EncodeError::NonFinite { at: at(), value });
    }
    let mut buffer = ryu::Buffer::new();
    out.extend_from_slice(buffer.format_finite(value).as_bytes());
    Ok(())
}

/// An integer, refused above [`MAX_INTEGER`].
pub(super) fn integer(
    out: &mut Vec<u8>,
    value: u64,
    at: impl FnOnce() -> String,
) -> Result<(), EncodeError> {
    if value > MAX_INTEGER {
        return Err(EncodeError::IntegerTooLarge { at: at(), value });
    }
    small(out, value);
    Ok(())
}

/// An integer known to be small (ids below 2^32, counts).
pub(super) fn small(out: &mut Vec<u8>, value: u64) {
    // Writing into a Vec cannot fail.
    let _ = write!(out, "{value}");
}

/// A signed integer known to be small (an epoch year).
pub(super) fn signed(out: &mut Vec<u8>, value: i64) {
    let _ = write!(out, "{value}");
}

pub(super) fn boolean(out: &mut Vec<u8>, value: bool) {
    out.extend_from_slice(if value { b"true" } else { b"false" });
}

pub(super) fn time(out: &mut Vec<u8>, at: DateTime<Utc>) {
    string(out, &time_text(at));
}

pub(super) fn date(out: &mut Vec<u8>, day: NaiveDate) {
    string(out, &normalized_date(day).format("%Y-%m-%d").to_string());
}

/// An object being written: `{`, then `"key":value` pairs, then `}`.
pub(super) struct Obj<'a> {
    out: &'a mut Vec<u8>,
    first: bool,
}

impl<'a> Obj<'a> {
    pub(super) fn open(out: &'a mut Vec<u8>) -> Self {
        out.push(b'{');
        Self { out, first: true }
    }

    /// Start a pair and hand back the buffer for its value.
    pub(super) fn key(&mut self, key: &str) -> &mut Vec<u8> {
        if !self.first {
            self.out.push(b',');
        }
        self.first = false;
        string(self.out, key);
        self.out.push(b':');
        self.out
    }

    /// Write preserved `(key, value)` pairs after the known ones.
    pub(super) fn preserved(&mut self, pairs: &[(String, super::RawJson)]) {
        for (key, value) in pairs {
            self.key(key).extend_from_slice(value.as_str().as_bytes());
        }
    }

    pub(super) fn close(self) {
        self.out.push(b'}');
    }
}
