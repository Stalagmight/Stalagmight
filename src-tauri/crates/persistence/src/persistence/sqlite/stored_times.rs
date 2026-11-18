//! Times the `.stalx` stores as RFC 3339 text: what the save writes, and the
//! value a time read back is brought to.
//!
//! `parse_from_rfc3339` accepts more than `to_rfc3339` can write back as text
//! a reader parses. An offset like `+23:59` on year 0000 or 9999 moves the
//! instant past the range once in UTC, and `to_rfc3339` then writes
//! `-0001-...` or `+10000-...`. Nothing parses that, and the project's own
//! times are read strictly, so a file saved with one never opened again.
//! SpeleoDB's Python reader (`datetime.fromisoformat`) also refuses year 0
//! and a leap second (`:60`). Only a crafted file or undo entry carries such
//! a time.
//!
//! A time is normalized on the way in as well as on the way out, so the value
//! in memory is the one the next save writes: a `.plnx` dive plan is stamped
//! with the project's `created_at` string and matched by it.

use chrono::{DateTime, Utc};

/// 0001-01-01T00:00:00Z, the first instant every reader parses.
const FIRST_SECOND: i64 = -62_135_596_800;
/// 9999-12-31T23:59:59Z, the last second every reader parses.
const LAST_SECOND: i64 = 253_402_300_799;

/// `at` moved into what every reader parses: years 0001-9999 in UTC and no
/// leap second. A time before is the first instant of 0001, a time after is
/// the last nanosecond of 9999, and a leap second is the nanosecond before
/// it. Any other time is unchanged.
pub(super) fn normalized(at: DateTime<Utc>) -> DateTime<Utc> {
    use chrono::Timelike as _;
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

/// `at` as the RFC 3339 text the save writes, [`normalized`] first.
pub(super) fn to_stored(at: DateTime<Utc>) -> String {
    normalized(at).to_rfc3339()
}

/// A stored time, [`normalized`]. Fails where `parse_from_rfc3339` does.
pub(super) fn parse_stored(text: &str) -> chrono::ParseResult<DateTime<Utc>> {
    Ok(normalized(
        DateTime::parse_from_rfc3339(text)?.with_timezone(&Utc),
    ))
}
