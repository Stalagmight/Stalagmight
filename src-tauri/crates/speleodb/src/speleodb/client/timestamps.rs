//! Preserve the app's UTC timestamp strings at the SDK boundary.
use time::{macros::format_description, OffsetDateTime, UtcOffset};

use super::SpeleoDbError;

/// Project details use +00:00; list serialization historically uses Z.
/// Chrono's automatic precision emits zero, three, six or nine fraction digits.
pub(super) fn project(date: OffsetDateTime, suffix: &str) -> Result<String, SpeleoDbError> {
    let Some(date) = date.checked_to_offset(UtcOffset::UTC) else {
        return extended_utc(date, chrono::SecondsFormat::AutoSi, suffix == "Z");
    };
    let base = date
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second]"
        ))
        .map_err(|e| SpeleoDbError::Parse(e.to_string()))?;
    let nanos = date.nanosecond();
    if nanos == 0 {
        return Ok(format!("{base}{suffix}"));
    }
    let (fraction, digits) = if nanos.is_multiple_of(1_000_000) {
        (nanos / 1_000_000, 3)
    } else if nanos.is_multiple_of(1_000) {
        (nanos / 1_000, 6)
    } else {
        (nanos, 9)
    };
    Ok(format!("{base}.{fraction:0digits$}{suffix}"))
}

/// Revision history always uses UTC with exactly three fraction digits.
pub(super) fn revision(date: OffsetDateTime) -> Result<String, SpeleoDbError> {
    let Some(date) = date.checked_to_offset(UtcOffset::UTC) else {
        return extended_utc(date, chrono::SecondsFormat::Millis, true);
    };
    date.format(format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
    ))
    .map_err(|e| SpeleoDbError::Parse(e.to_string()))
}

/// An SDK RFC3339 timestamp can cross time's year limit solely due to its offset.
/// Keep the existing extended UTC representation without widening time's range
/// for unrelated consumers in the workspace.
fn extended_utc(
    date: OffsetDateTime,
    precision: chrono::SecondsFormat,
    use_z: bool,
) -> Result<String, SpeleoDbError> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(date.unix_timestamp(), date.nanosecond())
        .map(|date| date.to_rfc3339_opts(precision, use_z))
        .ok_or_else(|| SpeleoDbError::Parse("timestamp is outside the supported UTC range".into()))
}
