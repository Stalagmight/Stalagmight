//! A persisted history entry (plan 8.3): one compact, deterministic JSON
//! value per entry, the format's `history.undo` / `history.redo` element.
//!
//! ```text
//! {"v":1,"t":"2026-11-20T12:59:00.000000000Z","k":"update_shot",
//!  "label":"Update shot Main: 4 -> 5","refs":{"lines":[3],"shots":["..."]},
//!  "preview":{"rows":[["Length","3.50","4.00"]]},"ops":{...}}
//! ```
//!
//! - `v` is [`HISTORY_FORMAT_VERSION`]. An entry of any other version does
//!   not decode ([`EntryError::Version`]); the caller drops the history whole
//!   with the existing banner, as #808 did.
//! - `t` is the push time, RFC 3339 UTC with nanoseconds (the format reads it
//!   for the SpeleoDB watermark cut).
//! - `k` is the command kind (the icon mapping), `label` the record-time
//!   label, `refs` the stable ids the label and preview name, `preview` any
//!   rows worded at record time (a batch's sub-commands), `ops` the
//!   [`Changeset`].
//!
//! The forward `Command` is never persisted: its payload holds session shot
//! ids and positional slots (R2 M10). The text is canonical by the format's
//! rules (no whitespace, minimal escapes, shortest round-trip floats), so it
//! survives `stalx_v2::RawJson::parse` unchanged.

use std::fmt;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{LineId, SectionId, StationId};

use super::super::{ClosureId, MAX_SHOT_UUID_CHARS};
use super::{shape, Changeset};

/// The history entry format this build writes and reads. Bump it with any
/// change to the entry or changeset layout, or to the line hash.
pub const HISTORY_FORMAT_VERSION: u32 = 1;

/// One persisted undo or redo step.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub timestamp: DateTime<Utc>,
    /// The command kind, for the history icon (`update_shot`, `batch`, ...).
    pub kind: String,
    /// The label, resolved with the names the project held at record time.
    pub label: String,
    /// The stable ids the label and preview name.
    pub refs: HistoryRefs,
    /// Preview rows worded at record time (what the changeset cannot say).
    pub preview: RecordedPreview,
    pub changes: Changeset,
}

impl HistoryEntry {
    /// Cut what only labels and previews the entry to what [`decode_entry`]
    /// accepts under `limits`: references of each kind, shot uuids, preview
    /// rows (the rest counted in `more`) and strings. The changeset is not
    /// touched. Recording calls it, so an entry the session holds is never
    /// one the next open refuses, which would drop the whole history (review
    /// finding E-undo-pipeline-3).
    pub fn fit_limits(&mut self, limits: &EntryLimits) {
        let cut = |text: &mut String| {
            if text.chars().count() > limits.max_text_chars {
                let keep = limits.max_text_chars.saturating_sub(1);
                *text = text
                    .chars()
                    .take(keep)
                    .chain(std::iter::once('…'))
                    .collect();
            }
        };
        cut(&mut self.kind);
        cut(&mut self.label);
        let refs = &mut self.refs;
        refs.shots
            .retain(|u| u.chars().count() <= MAX_SHOT_UUID_CHARS);
        refs.lines.truncate(limits.max_refs);
        refs.stations.truncate(limits.max_refs);
        refs.shots.truncate(limits.max_refs);
        refs.closures.truncate(limits.max_refs);
        refs.sections.truncate(limits.max_refs);
        let rows = &mut self.preview.rows;
        if rows.len() > limits.max_preview_rows {
            let dropped = rows.len() - limits.max_preview_rows;
            rows.truncate(limits.max_preview_rows);
            self.preview.more = self.preview.more.saturating_add(dropped as u64);
        }
        for row in rows.iter_mut() {
            cut(&mut row.field);
            cut(&mut row.old);
            cut(&mut row.new);
        }
    }
}

/// Entities an entry names, by stable id (never session ids or slots).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryRefs {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<LineId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stations: Vec<StationId>,
    /// Shots by uuid.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shots: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closures: Vec<ClosureId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<SectionId>,
}

impl HistoryRefs {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
            && self.stations.is_empty()
            && self.shots.is_empty()
            && self.closures.is_empty()
            && self.sections.is_empty()
    }
}

/// Preview rows a command worded when it was recorded, and how many it left
/// out (`+N more`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedPreview {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<PreviewRow>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: u64,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

impl RecordedPreview {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.more == 0
    }
}

/// One preview row: `field: old -> new` (either side may be empty).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviewRow {
    pub field: String,
    pub old: String,
    pub new: String,
}

impl Serialize for PreviewRow {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(3))?;
        seq.serialize_element(&self.field)?;
        seq.serialize_element(&self.old)?;
        seq.serialize_element(&self.new)?;
        seq.end()
    }
}

impl<'de> Deserialize<'de> for PreviewRow {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = PreviewRow;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("[field, old, new]")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<PreviewRow, A::Error> {
                let mut next = || -> Result<String, A::Error> {
                    seq.next_element()?
                        .ok_or_else(|| de::Error::custom("a preview row needs three strings"))
                };
                let row = PreviewRow {
                    field: next()?,
                    old: next()?,
                    new: next()?,
                };
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("a preview row with extra elements"));
                }
                Ok(row)
            }
        }
        d.deserialize_seq(V)
    }
}

/// Bounds on an entry read back from a file (untrusted input).
#[derive(Debug, Clone, Copy)]
pub struct EntryLimits {
    /// The longest entry text accepted.
    pub max_bytes: usize,
    /// The most references of each kind.
    pub max_refs: usize,
    /// The most recorded preview rows.
    pub max_preview_rows: usize,
    /// The longest kind, label or preview string, in characters.
    pub max_text_chars: usize,
}

impl Default for EntryLimits {
    fn default() -> Self {
        Self {
            max_bytes: 64 << 20,
            max_refs: 4096,
            max_preview_rows: 64,
            max_text_chars: 4096,
        }
    }
}

/// Why an entry could not be written or read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryError {
    /// The entry text is longer than [`EntryLimits::max_bytes`].
    TooLarge(usize),
    /// Not one JSON entry of this layout.
    Syntax(String),
    /// Written by another history format version.
    Version(u64),
    /// The time is not RFC 3339.
    Time(String),
    /// The changeset breaks a structural rule.
    Shape(String),
    /// A count or length past [`EntryLimits`].
    Limit(&'static str),
    /// A non-finite number (refused on write).
    NonFinite,
}

impl fmt::Display for EntryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge(n) => write!(f, "history entry is {n} bytes, past the limit"),
            Self::Syntax(e) => write!(f, "history entry is unreadable: {e}"),
            Self::Version(v) => write!(
                f,
                "history entry has format version {v}; this build reads {HISTORY_FORMAT_VERSION}"
            ),
            Self::Time(t) => write!(f, "history entry time {t:?} is not RFC 3339"),
            Self::Shape(e) => write!(f, "history entry is malformed: {e}"),
            Self::Limit(what) => write!(f, "history entry has too many or too long {what}"),
            Self::NonFinite => f.write_str("history entry holds a non-finite number"),
        }
    }
}

impl std::error::Error for EntryError {}

#[derive(Serialize)]
struct WireOut<'a> {
    v: u32,
    t: String,
    k: &'a str,
    label: &'a str,
    #[serde(skip_serializing_if = "HistoryRefs::is_empty")]
    refs: &'a HistoryRefs,
    #[serde(skip_serializing_if = "RecordedPreview::is_empty")]
    preview: &'a RecordedPreview,
    ops: &'a Changeset,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireIn {
    v: u64,
    t: String,
    k: String,
    label: String,
    #[serde(default)]
    refs: HistoryRefs,
    #[serde(default)]
    preview: RecordedPreview,
    ops: Changeset,
}

#[derive(Deserialize)]
struct VersionOnly {
    v: u64,
}

/// The persisted text of `entry` (one canonical JSON value). Refused when a
/// number is not finite or the changeset breaks a structural rule.
pub fn encode_entry(entry: &HistoryEntry) -> Result<String, EntryError> {
    if !entry.changes.finite() {
        return Err(EntryError::NonFinite);
    }
    shape::check(&entry.changes).map_err(EntryError::Shape)?;
    let wire = WireOut {
        v: HISTORY_FORMAT_VERSION,
        t: entry.timestamp.to_rfc3339_opts(SecondsFormat::Nanos, true),
        k: &entry.kind,
        label: &entry.label,
        refs: &entry.refs,
        preview: &entry.preview,
        ops: &entry.changes,
    };
    serde_json::to_string(&wire).map_err(|e| EntryError::Syntax(e.to_string()))
}

/// Read an entry back (untrusted input): size first, then the version, then
/// the layout, the limits and the changeset's structural rules. The
/// changeset carries no session ids: a step mints them.
pub fn decode_entry(text: &str, limits: &EntryLimits) -> Result<HistoryEntry, EntryError> {
    if text.len() > limits.max_bytes {
        return Err(EntryError::TooLarge(text.len()));
    }
    let version: VersionOnly =
        serde_json::from_str(text).map_err(|e| EntryError::Syntax(e.to_string()))?;
    if version.v != u64::from(HISTORY_FORMAT_VERSION) {
        return Err(EntryError::Version(version.v));
    }
    let wire: WireIn = serde_json::from_str(text).map_err(|e| EntryError::Syntax(e.to_string()))?;
    let _ = wire.v;
    let timestamp = DateTime::parse_from_rfc3339(&wire.t)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| EntryError::Time(wire.t.clone()))?;
    let long = |s: &str| s.chars().count() > limits.max_text_chars;
    if long(&wire.k) || long(&wire.label) {
        return Err(EntryError::Limit("kind or label characters"));
    }
    let refs = &wire.refs;
    if [
        refs.lines.len(),
        refs.stations.len(),
        refs.shots.len(),
        refs.closures.len(),
        refs.sections.len(),
    ]
    .iter()
    .any(|&n| n > limits.max_refs)
        || refs
            .shots
            .iter()
            .any(|u| u.chars().count() > MAX_SHOT_UUID_CHARS)
    {
        return Err(EntryError::Limit("references"));
    }
    if wire.preview.rows.len() > limits.max_preview_rows
        || wire
            .preview
            .rows
            .iter()
            .any(|r| long(&r.field) || long(&r.old) || long(&r.new))
    {
        return Err(EntryError::Limit("preview rows"));
    }
    shape::check(&wire.ops).map_err(EntryError::Shape)?;
    Ok(HistoryEntry {
        timestamp,
        kind: wire.k,
        label: wire.label,
        refs: wire.refs,
        preview: wire.preview,
        changes: wire.ops,
    })
}
