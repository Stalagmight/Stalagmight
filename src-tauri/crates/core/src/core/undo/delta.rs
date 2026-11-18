//! Compact undo payloads: what turns one serialized project state into another.
//!
//! A command whose effect cannot be inverted field by field (a delete, a merge,
//! a split, a batch) used to record TWO whole-project JSON snapshots
//! (`UndoData::ProjectSnapshotPair`). On a 20,000-shot survey that is ~17 MB
//! per entry for an edit that touched one shot. [`ProjectDelta`] records only
//! what differs between the two states, in the shape of the same serialized
//! project, so it restores exactly what the snapshot pair restored:
//!
//! * the top-level members other than `lines`, edited member by member
//!   ([`Edit`]): a changed offset key, an added merge, a new datum;
//! * the lines, as runs over the source's lines ([`LineRun`]): a run
//!   of untouched lines costs two numbers;
//! * a touched line's shots, as runs over the source's SHOTS
//!   ([`ShotRun`]), matched by uuid in ANY source line and in either
//!   direction - so a Split Line Here, a Combine Lines or a Reverse
//!   Line, which move or reorder shots without changing them, costs a few
//!   numbers per run instead of every shot twice.
//!
//! A delta is directional: it turns its SOURCE (the live project at the moment
//! it is applied) into its TARGET (the other side of the entry). An undo entry
//! holds the delta to its `before` state; the step that applies it re-encodes
//! the reverse delta from the two states it has in hand, so the entry carries
//! one direction at a time ([`super::SnapshotDelta`]).
//!
//! Equality is bitwise on floats (`-0.0` is not `0.0`): a snapshot restore puts
//! back the recorded bits, and a delta has to as well.//!
//! The engine works on serialized TEXT ([`Doc`]): an untouched line stays a
//! borrowed slice of the live project's JSON, and only what differs is ever
//! parsed (`delta/doc.rs`). The value-level [`ProjectDelta::between`] and
//! [`ProjectDelta::apply`] are the same engine behind a serialization.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value};

use crate::models::LineId;

mod doc;
mod history;
mod json;
mod shape;
mod shots;
mod stations;

pub use doc::{Applied, Doc, LineIds, LineOut};
pub use history::HolderLine;
#[cfg(test)]
use json::same;

const LINES: &str = "lines";
const SHOTS: &str = "shots";
const UUID: &str = "uuid";
const ID: &str = "id";
const NEXT_SHOT_ID: &str = "next_shot_id";
const NEXT_LINE_ID: &str = "next_line_id";
const GAUGE_SHOT: &str = "gauge_shot_uuid";
const GAUGE_NEIGHBOUR: &str = "gauge_neighbour_uuid";

/// Arrays this short are replaced whole rather than diffed: a run list for a
/// three-number tuple is longer than the tuple.
const SMALL_ARRAY: usize = 4;

/// A top-level list this short is replaced whole: datums, held gauges and
/// the like are short, and a whole list does not depend on positions that a
/// load-time prune (`prune_held_gauges`) can shift.
const SMALL_LIST: usize = 64;

/// The top-level members whose array order carries no meaning: each
/// deserializes into a `HashMap` or a `BTreeSet`, and serializes sorted. They
/// are diffed as bags ([`Edit::Bag`]). `counter_parity_shots` is pruned on
/// load, which a positional edit could not survive.
///
/// These are the in-memory serde names: a delta is encoded from and applied to
/// the project as this build serializes it. Its on-disk form goes through
/// `models::disk_names` with the rest of the undo row (`undo/persistence.rs`).
const UNORDERED_MEMBERS: [&str; 5] = [
    "station_offsets",
    "station_comments",
    "station_names",
    "station_ids",
    "counter_parity_shots",
];

/// The edit distance past which the array diff stops looking for a minimal
/// script and replaces the differing middle whole. Bounds the diff's memory
/// (the trace is quadratic in it) on an edit that rewrites a long list.
const MAX_ARRAY_EDITS: usize = 512;

/// How one JSON value becomes another.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Edit {
    /// The target value, whole.
    #[serde(rename = "s")]
    Set(Value),
    /// The member is absent in the target. Only meaningful as an object
    /// member's edit.
    #[serde(rename = "d")]
    Del,
    /// Both are objects: per-member edits, every member not named is kept.
    #[serde(rename = "o")]
    Obj(BTreeMap<String, Edit>),
    /// Both are arrays: the source's length, checked on apply, and the target
    /// run by run over the source. Positional, so a source whose length is
    /// not the recorded one refuses rather than keeping the wrong elements.
    #[serde(rename = "a")]
    Arr(usize, Vec<Run>),
    /// Both are arrays whose ORDER carries no meaning - they deserialize into a
    /// map or a set (`station_offsets`, `counter_parity_shots`, ...): the
    /// elements to remove, each matched by value, and the ones to add, each
    /// with its index in the target. Unlike a positional edit it survives a
    /// load or a save that pruned an unrelated element out of the list: the
    /// index only orders the result, which the restore re-sorts anyway.
    ///
    /// For a station-keyed member (`delta/stations.rs`) a `[station key,
    /// value]` pair whose key names a station of its state is written as a
    /// position instead: `station_del` names stations of the SOURCE,
    /// `station_add` stations of the TARGET (with the pair's index in the
    /// target), as `(line id, station index, value)`. The apply rebuilds each
    /// key from that state's shots before the bag is applied.
    #[serde(rename = "b")]
    Bag {
        #[serde(rename = "d", default, skip_serializing_if = "Vec::is_empty")]
        del: Vec<Value>,
        #[serde(rename = "a", default, skip_serializing_if = "Vec::is_empty")]
        add: Vec<(usize, Value)>,
        #[serde(rename = "sd", default, skip_serializing_if = "Vec::is_empty")]
        station_del: Vec<(LineId, u32, Value)>,
        #[serde(rename = "sa", default, skip_serializing_if = "Vec::is_empty")]
        station_add: Vec<(usize, LineId, u32, Value)>,
    },
}

/// One run of a target array, over the source array.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Run {
    /// `len` source elements from `at`, verbatim.
    #[serde(rename = "k")]
    Keep(usize, usize),
    /// Literal elements.
    #[serde(rename = "p")]
    Put(Vec<Value>),
    /// The source element at `at`, edited.
    #[serde(rename = "m")]
    Mod(usize, Box<Edit>),
}

/// One run of the target's lines, over the source's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LineRun {
    /// `len` source lines from `at`, verbatim.
    #[serde(rename = "k")]
    Keep(usize, usize),
    /// The source line at `at` (whose id is `id`, checked on apply), with
    /// its members other than `shots` edited, and its shots rebuilt when
    /// `shots` is set.
    #[serde(rename = "e")]
    Edit {
        at: usize,
        id: LineId,
        #[serde(rename = "f", default, skip_serializing_if = "BTreeMap::is_empty")]
        fields: BTreeMap<String, Edit>,
        #[serde(rename = "s", default, skip_serializing_if = "Option::is_none")]
        shots: Option<Vec<ShotRun>>,
        /// The source side held no gauge pair here (see
        /// [`ProjectDelta::ungauged_lines`]).
        #[serde(rename = "u", default, skip_serializing_if = "is_false")]
        ungauged: bool,
    },
    /// A line the source does not hold: its members other than `shots`,
    /// and its shots (`None` when the members carry `shots` themselves,
    /// because it was not an array).
    #[serde(rename = "n")]
    New {
        #[serde(rename = "f")]
        fields: Map<String, Value>,
        #[serde(rename = "s", default, skip_serializing_if = "Option::is_none")]
        shots: Option<Vec<ShotRun>>,
    },
}

/// One run of a target line's shots, over the shots of ANY source line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShotRun {
    /// `n` shots of the source line at position `sec`, from index `at`:
    /// upwards, or downwards when `rev` (a reversed line). `flip` turns
    /// each copy around first, as Reverse Line turns a shot around
    /// (`shots::flip_shot`; the flag is its swap-left-right option). `m` edits the
    /// copies, by offset into the run, ascending.
    #[serde(rename = "c")]
    Copy {
        sec: usize,
        at: usize,
        n: usize,
        #[serde(default, skip_serializing_if = "is_false")]
        rev: bool,
        #[serde(rename = "f", default, skip_serializing_if = "Option::is_none")]
        flip: Option<bool>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        m: Vec<(usize, Edit)>,
    },
    /// Literal shots. Written as tables of rows where consecutive shots share
    /// their keys (`shots::shot_rows`), so the keys are not repeated per shot.
    #[serde(rename = "p", with = "shots::shot_rows")]
    Put(Vec<Value>),
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The difference between two serialized projects, as the edits that turn the
/// SOURCE into the TARGET. See the module docs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectDelta {
    /// Edits to the top-level members other than `lines`.
    #[serde(rename = "f", default, skip_serializing_if = "BTreeMap::is_empty")]
    fields: BTreeMap<String, Edit>,
    /// The target's lines, when they differ from the source's: the
    /// source's line count (checked on apply) and the runs.
    #[serde(rename = "s", default, skip_serializing_if = "Option::is_none")]
    lines: Option<(usize, Vec<LineRun>)>,
}

/// Why a delta could not be encoded or applied. A message, because nothing
/// branches on it: it is logged and the step fails.
pub type DeltaError = String;

/// Where in the target a shot that the delta carried literally landed:
/// (line index, shot index).
pub(crate) type LiteralShot = (usize, usize);

/// One undo entry's compact payload ([`super::UndoData::Delta`]).
///
/// The delta is held SERIALIZED, as the JSON its row writes: a parsed
/// [`ProjectDelta`] - a tree of maps, vectors and `serde_json::Value`s, a
/// one-member edit of a shot a `BTreeMap` node of its own - took 15 to 25
/// times its JSON size on the heap (on a 20,000-shot survey, 115 KB for a
/// Delete Line whose JSON is 7.6 KB, 1.45 MB for a 1,000-cell paste's
/// 33 KB), while the history's byte budget counts the JSON. It is parsed
/// where it is used ([`Self::delta`]: a step, the load's holder walk and uuid
/// remap, the shape check of a row read back), each a few kilobytes.
///
/// The row is written as it always was: `d` is the delta's JSON object, not
/// a string. A row's whole envelope goes through `serde_json::Value` on its
/// way to and from the disk (`models::disk_names`), which parses the held
/// text into the same object a parsed delta serializes to, and reads it back
/// into text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotDelta {
    /// Turns the live project into the state on the other side of the entry:
    /// its `before` while the entry is on the undo stack, its `after` while it
    /// is on the redo stack. Re-encoded by every step through the entry.
    #[serde(rename = "d")]
    delta: Box<RawValue>,
    /// What the history preview reads from the `before` state, which the
    /// entry no longer holds whole. Recorded once, at capture.
    #[serde(rename = "p", default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<BeforePreview>,
    /// How many sub-commands of the entry's `Batch` its stored command
    /// leaves out (`undo::trimmed_batch`): a step never reads them, and the
    /// history preview counts them. Zero for every other entry.
    ///
    /// FORMAT EVOLUTION (additive): absent in rows written before it existed,
    /// whose batches are stored whole.
    #[serde(rename = "n", default, skip_serializing_if = "is_zero")]
    pub omitted_commands: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// The facts a history preview reads from an entry's `before` state.
///
/// Persisted inside the entry, so its variant tags cross the disk boundary
/// through `models::disk_names` like every other undo key: `Line` is written
/// `Section`, the name it had before the Section -> Line rename.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BeforePreview {
    /// The line a Delete Line removed: its name and shot count.
    Line { name: Option<String>, shots: usize },
    /// The GPS of the datum a Remove Zero Datum removed, when it had one.
    Datum(Option<crate::models::GpsCoordinates>),
    /// The names of the named lines a Batch deleted (its first
    /// `DeleteLine` sub-commands, as many as its preview lists): the label
    /// and preview of a Delete Line inside the batch name the line from here
    /// once it is gone, as they do from a snapshot pair's `before`.
    DeletedLines(Vec<(LineId, String)>),
}

impl SnapshotDelta {
    /// The payload holding `delta`, with the facts a preview reads.
    pub fn new(delta: &ProjectDelta, preview: Option<BeforePreview>) -> Result<Self, DeltaError> {
        Ok(Self {
            delta: held(delta)?,
            preview,
            omitted_commands: 0,
        })
    }

    /// The entry for a capture's two snapshots: the delta from `after` (the
    /// live project when the entry is pushed) to `before`.
    pub fn from_pair(
        before: &str,
        after: &str,
        preview: Option<BeforePreview>,
    ) -> Result<Self, DeltaError> {
        Self::new(&ProjectDelta::between_texts(after, before)?, preview)
    }

    /// The delta this entry holds, parsed. Every row passed
    /// [`ProjectDelta::check_shape`] when it was read and every encode checks
    /// its result, so this fails only on a payload built by hand.
    pub fn delta(&self) -> Result<ProjectDelta, DeltaError> {
        serde_json::from_str(self.delta.get())
            .map_err(|e| format!("an undo entry's delta cannot be read: {e}"))
    }

    /// Hold `delta` in place of the current one.
    pub fn set_delta(&mut self, delta: &ProjectDelta) -> Result<(), DeltaError> {
        self.delta = held(delta)?;
        Ok(())
    }

    /// Run `edit` over the held delta and keep what it leaves when it says it
    /// changed something (the load's uuid remap). A delta that cannot be read
    /// or written back is left as it is.
    pub fn edit(&mut self, edit: impl FnOnce(&mut ProjectDelta) -> bool) -> bool {
        let Ok(mut delta) = self.delta() else {
            return false;
        };
        edit(&mut delta) && self.set_delta(&delta).is_ok()
    }

    /// The serialized length of this payload: what it costs the history, and
    /// what it holds in memory.
    pub fn encoded_len(&self) -> usize {
        serde_json::to_string(self).map_or(0, |json| json.len())
    }
}

/// `delta` as an entry holds it.
fn held(delta: &ProjectDelta) -> Result<Box<RawValue>, DeltaError> {
    serde_json::value::to_raw_value(delta).map_err(|e| e.to_string())
}

fn to_text(value: &Value) -> Result<String, DeltaError> {
    serde_json::to_string(value).map_err(|e| e.to_string())
}

impl ProjectDelta {
    /// The edits that turn `source` into `target`, two serialized projects.
    pub fn between(source: &Value, target: &Value) -> Result<Self, DeltaError> {
        Self::between_texts(&to_text(source)?, &to_text(target)?)
    }

    /// [`Self::between`] for two projects as text.
    pub fn between_texts(source: &str, target: &str) -> Result<Self, DeltaError> {
        Self::between_docs(&Doc::parse(source)?, &Doc::parse(target)?)
    }

    /// [`Self::between`] for two split projects.
    pub fn between_docs(source: &Doc<'_>, target: &Doc<'_>) -> Result<Self, DeltaError> {
        doc::encode(source, target)
    }

    /// True when the two states were identical.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.lines.is_none()
    }

    /// The target this delta was encoded towards, rebuilt from `source`.
    ///
    /// Shot ids are SETTLED on the way out: a shot this delta carries
    /// literally keeps the in-session id it was recorded with unless that id
    /// is now taken (the loader renumbers every shot, so after a reopen it can
    /// be), and every id allocator is raised past the ids it could otherwise
    /// hand out twice. In a session that has not reloaded nothing collides and
    /// this changes nothing.
    pub fn apply(&self, source: &Value) -> Result<Value, DeltaError> {
        let text = to_text(source)?;
        let doc = Doc::parse(&text)?;
        let ids = LineIds::of_doc(&doc)?;
        let target = self.apply_doc(&doc, &ids)?.into_doc()?.text();
        serde_json::from_str(&target).map_err(|e| e.to_string())
    }

    /// [`Self::apply`] to a split project whose lines' ids `ids` lists, in
    /// order ([`LineIds::of_project`] for the live project).
    pub fn apply_doc<'a>(
        &self,
        source: &Doc<'a>,
        ids: &[LineIds],
    ) -> Result<Applied<'a>, DeltaError> {
        doc::apply(self, source, ids)
    }
}

fn shots_array(line: &Value) -> Option<&Vec<Value>> {
    line.get(SHOTS)?.as_array()
}

fn line_id_of(line: &Value) -> Option<u64> {
    line.get(ID)?.as_u64()
}

fn line_id_of_map(line: &Map<String, Value>) -> Option<LineId> {
    LineId::try_from(line.get(ID)?.as_u64()?).ok()
}

#[cfg(test)]
#[path = "delta_tests.rs"]
mod tests;
