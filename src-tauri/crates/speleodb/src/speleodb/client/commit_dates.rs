//! Commit dates across SpeleoDB's revision-date change (server a679010c6,
//! 2026-11-10).
//!
//! A server from before the change sends a commit's `authored_date` as wall-clock
//! time in its own zone (America/New_York) with no offset, `2026/11/10 03:18`. A
//! server after it sends an ISO-8601 UTC instant, `2026-11-10T08:18:23.123456Z`.
//! SDK 0.6.2 decodes the field as a UTC instant and refuses the old shape, so
//! against an older server every reply that carries a commit fails to decode:
//! the revision list, and every project reply, whose `latest_commit` embeds the
//! newest commit in full.
//!
//! [`decode_reply`] decodes such a reply again with only those dates out of the
//! SDK's way; everything else in it, the rest of each commit included, is still
//! held to the SDK contract.

use chrono::{DateTime, FixedOffset, NaiveDateTime, SecondsFormat, Utc};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// A date the SDK reads, standing in for one it cannot while the reply decodes.
/// It never reaches the app: the caller takes every revision date from
/// [`Reply::dates`], and the app reads only the id of a `latest_commit`.
const STAND_IN: &str = "1970-01-01T00:00:00Z";

/// The old server's wall-time format.
const WALL_TIME: &str = "%Y/%m/%d %H:%M";

/// A revision date as the frontend gets it (`SpeleoRevision::authored_date`).
///
/// An instant becomes UTC with milliseconds, `2026-11-10T08:18:23.123Z`, the
/// precision every webview's `Date` is required to read. The old server's wall
/// time passes through unchanged: the frontend converts it with the webview's
/// time-zone data (`src/utils/speleoRevisionTime.ts`), which the backend lacks.
/// Anything else is `None`, so the history shows no time rather than a wrong one.
pub(super) fn display_date(raw: &str) -> Option<String> {
    let text = raw.trim();
    if is_wall_time(text) {
        return Some(text.to_owned());
    }
    sdk_instant(text).map(|date| instant(date.with_timezone(&Utc)))
}

/// `date` as [`display_date`] renders an instant.
pub(super) fn instant(date: DateTime<Utc>) -> String {
    date.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// `text` read the way the SDK reads a commit date (chrono's own `FromStr`, which
/// its serde support uses).
fn sdk_instant(text: &str) -> Option<DateTime<FixedOffset>> {
    text.parse().ok()
}

/// `2026/11/10 03:18`, the old server's zone-less wall time: exactly that shape,
/// and a real date and time.
pub(super) fn is_wall_time(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 16
        && bytes.iter().enumerate().all(|(i, &b)| match i {
            4 | 7 => b == b'/',
            10 => b == b' ',
            13 => b == b':',
            _ => b.is_ascii_digit(),
        })
        && NaiveDateTime::parse_from_str(text, WALL_TIME).is_ok()
}

/// Swap `date` for [`STAND_IN`] when it is a string the SDK cannot read. Whether
/// it was swapped. A missing date, or one that is not a string, is left for the
/// SDK to refuse.
fn set_aside(date: &mut Value) -> bool {
    let unreadable = date
        .as_str()
        .is_some_and(|text| sdk_instant(text).is_none());
    if unreadable {
        *date = Value::from(STAND_IN);
    }
    unreadable
}

/// A 2xx reply the SDK refused, decoded with its unreadable commit dates set aside.
pub(super) struct Reply<T> {
    pub(super) value: T,
    /// The display date ([`display_date`]) of each entry of the reply's top-level
    /// `commits` list, in order. Empty when the SDK decoded the reply itself, or the
    /// reply has no revision list.
    pub(super) dates: Vec<Option<String>>,
}

impl<T> Reply<T> {
    /// A reply the SDK decoded itself.
    pub(super) fn typed(value: T) -> Self {
        Self {
            value,
            dates: Vec::new(),
        }
    }
}

/// Decode `body`, a 2xx reply the SDK could not deserialize, with every commit
/// date it cannot read set aside: the date of each embedded `latest_commit` (the
/// app reads only that commit's id) and of each revision in a top-level `commits`
/// list (whose dates to show [`Reply::dates`] carries). Only a date that is a
/// string is set aside; the rest of each commit, and of the reply, is still
/// decoded by the SDK's own types.
///
/// `None` when the reply holds no such date: the refusal is something else, and
/// stays the SDK's contract mismatch. The decoder's error when it still does not
/// decode, which names what else the reply gets wrong (the SDK's own error may
/// name only a date). The re-decode goes through a JSON value, so unlike the SDK's
/// own pass it keeps the last of two duplicate keys rather than refusing them.
pub(super) fn decode_reply<T: DeserializeOwned>(
    body: &[u8],
) -> Option<Result<Reply<T>, serde_json::Error>> {
    let mut value: Value = serde_json::from_slice(body).ok()?;
    let mut changed = set_aside_latest_commit_dates(&mut value);
    let mut dates = Vec::new();
    if let Some(commits) = value.get_mut("commits").and_then(Value::as_array_mut) {
        for commit in commits {
            let Some(date) = commit.get_mut("authored_date") else {
                dates.push(None);
                continue;
            };
            dates.push(date.as_str().and_then(display_date));
            changed |= set_aside(date);
        }
    }
    if !changed {
        return None;
    }
    Some(serde_json::from_value(value).map(|value| Reply { value, dates }))
}

/// Set aside the date of every `latest_commit` object under `value`. Whether any
/// was.
fn set_aside_latest_commit_dates(value: &mut Value) -> bool {
    match value {
        Value::Object(map) => {
            let mut changed = map
                .get_mut("latest_commit")
                .and_then(|commit| commit.get_mut("authored_date"))
                .is_some_and(set_aside);
            for child in map.values_mut() {
                changed |= set_aside_latest_commit_dates(child);
            }
            changed
        }
        Value::Array(items) => items.iter_mut().fold(false, |changed, item| {
            set_aside_latest_commit_dates(item) | changed
        }),
        _ => false,
    }
}
