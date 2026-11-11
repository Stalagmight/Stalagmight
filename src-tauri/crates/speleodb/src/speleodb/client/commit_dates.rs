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
//! SDK's way; everything else in it is still held to the SDK contract.

use chrono::{DateTime, FixedOffset, SecondsFormat, Utc};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// A date the SDK reads, standing in for one it cannot while the reply decodes.
/// It never reaches the app: the caller takes every revision date from
/// [`Reply::dates`], and a reduced `latest_commit` carries no date at all.
const STAND_IN: &str = "1970-01-01T00:00:00Z";

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

/// `2026/11/10 03:18`, the old server's zone-less wall time.
fn is_wall_time(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 16
        && bytes.iter().enumerate().all(|(i, &b)| match i {
            4 | 7 => b == b'/',
            10 => b == b' ',
            13 => b == b':',
            _ => b.is_ascii_digit(),
        })
}

/// Whether the SDK reads `value` as a commit date.
fn sdk_readable(value: &Value) -> bool {
    value.as_str().and_then(sdk_instant).is_some()
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

/// Decode `body`, a 2xx reply the SDK refused, once every commit date it cannot
/// read is out of the way:
/// - each embedded `latest_commit` with such a date becomes that commit's bare id:
///   the SDK's own string form of the field, and the only part of it the app reads
///   (`client::project`);
/// - each revision in a top-level `commits` list keeps its place with its date
///   swapped for a stand-in, and [`Reply::dates`] carries the dates to show.
///
/// `None` when the reply holds no such date: the refusal is something else, and
/// stays the SDK's contract mismatch. The decoder's error when it still does not
/// decode, which names what else the reply gets wrong (the SDK's own error may
/// name only a date).
pub(super) fn decode_reply<T: DeserializeOwned>(
    body: &[u8],
) -> Option<Result<Reply<T>, serde_json::Error>> {
    let mut value: Value = serde_json::from_slice(body).ok()?;
    let mut changed = reduce_latest_commits(&mut value);
    let mut dates = Vec::new();
    if let Some(commits) = value.get_mut("commits").and_then(Value::as_array_mut) {
        for commit in commits {
            let Some(date) = commit.get_mut("authored_date") else {
                dates.push(None);
                continue;
            };
            dates.push(date.as_str().and_then(display_date));
            if !sdk_readable(date) {
                *date = Value::from(STAND_IN);
                changed = true;
            }
        }
    }
    if !changed {
        return None;
    }
    Some(serde_json::from_value(value).map(|value| Reply { value, dates }))
}

/// Reduce every `latest_commit` object under `value` whose date the SDK cannot
/// read to its `id`. Whether any was reduced.
fn reduce_latest_commits(value: &mut Value) -> bool {
    match value {
        Value::Object(map) => {
            let mut changed = false;
            if let Some(commit) = map.get_mut("latest_commit") {
                let id = commit
                    .get("authored_date")
                    .filter(|date| !sdk_readable(date))
                    .and(commit.get("id"))
                    .filter(|id| id.is_string())
                    .cloned();
                if let Some(id) = id {
                    *commit = id;
                    changed = true;
                }
            }
            for child in map.values_mut() {
                changed |= reduce_latest_commits(child);
            }
            changed
        }
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |changed, item| reduce_latest_commits(item) | changed),
        _ => false,
    }
}
