//! Compact undo payloads: what turns one serialized project state into another.
//!
//! A command whose effect cannot be inverted field by field (a delete, a merge,
//! a split, a batch) used to record TWO whole-project JSON snapshots
//! (`UndoData::ProjectSnapshotPair`). On a 20,000-shot survey that is ~17 MB
//! per entry for an edit that touched one shot. [`ProjectDelta`] records only
//! what differs between the two states, in the shape of the same serialized
//! project, so it restores exactly what the snapshot pair restored:
//!
//! * the top-level members other than `sections`, edited member by member
//!   ([`Edit`]): a changed offset key, an added merge, a new datum;
//! * the sections, as runs over the source's sections ([`SectionRun`]): a run
//!   of untouched sections costs two numbers;
//! * a touched section's shots, as runs over the source's SHOTS
//!   ([`ShotRun`]), matched by uuid in ANY source section and in either
//!   direction - so a Split Section Here, a Combine Sections or a Reverse
//!   Section, which move or reorder shots without changing them, costs a few
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
//! The engine works on serialized TEXT ([`Doc`]): an untouched section stays a
//! borrowed slice of the live project's JSON, and only what differs is ever
//! parsed (`delta/doc.rs`). The value-level [`ProjectDelta::between`] and
//! [`ProjectDelta::apply`] are the same engine behind a serialization.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::models::SectionId;

mod doc;
mod history;
mod json;
mod shots;

pub use doc::{Applied, Doc, SectionIds, SectionOut};
pub use history::HolderSection;
#[cfg(test)]
use json::same;

const SECTIONS: &str = "sections";
const SHOTS: &str = "shots";
const UUID: &str = "uuid";
const ID: &str = "id";
const NEXT_SHOT_ID: &str = "next_shot_id";
const NEXT_SECTION_ID: &str = "next_section_id";
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
const UNORDERED_MEMBERS: [&str; 4] = [
    "station_offsets",
    "station_comments",
    "station_names",
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
    #[serde(rename = "b")]
    Bag {
        #[serde(rename = "d", default, skip_serializing_if = "Vec::is_empty")]
        del: Vec<Value>,
        #[serde(rename = "a", default, skip_serializing_if = "Vec::is_empty")]
        add: Vec<(usize, Value)>,
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

/// One run of the target's sections, over the source's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SectionRun {
    /// `len` source sections from `at`, verbatim.
    #[serde(rename = "k")]
    Keep(usize, usize),
    /// The source section at `at` (whose id is `id`, checked on apply), with
    /// its members other than `shots` edited, and its shots rebuilt when
    /// `shots` is set.
    #[serde(rename = "e")]
    Edit {
        at: usize,
        id: SectionId,
        #[serde(rename = "f", default, skip_serializing_if = "BTreeMap::is_empty")]
        fields: BTreeMap<String, Edit>,
        #[serde(rename = "s", default, skip_serializing_if = "Option::is_none")]
        shots: Option<Vec<ShotRun>>,
        /// The source side held no gauge pair here (see
        /// [`ProjectDelta::ungauged_sections`]).
        #[serde(rename = "u", default, skip_serializing_if = "is_false")]
        ungauged: bool,
    },
    /// A section the source does not hold: its members other than `shots`,
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

/// One run of a target section's shots, over the shots of ANY source section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShotRun {
    /// `n` shots of the source section at position `sec`, from index `at`:
    /// upwards, or downwards when `rev` (a reversed section). `flip` turns
    /// each copy around first, as Reverse Section turns a shot around
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
    /// Edits to the top-level members other than `sections`.
    #[serde(rename = "f", default, skip_serializing_if = "BTreeMap::is_empty")]
    fields: BTreeMap<String, Edit>,
    /// The target's sections, when they differ from the source's: the
    /// source's section count (checked on apply) and the runs.
    #[serde(rename = "s", default, skip_serializing_if = "Option::is_none")]
    sections: Option<(usize, Vec<SectionRun>)>,
}

/// Why a delta could not be encoded or applied. A message, because nothing
/// branches on it: it is logged and the step fails.
pub type DeltaError = String;

/// Where in the target a shot that the delta carried literally landed:
/// (section index, shot index).
pub(crate) type LiteralShot = (usize, usize);

/// One undo entry's compact payload ([`super::UndoData::Delta`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotDelta {
    /// Turns the live project into the state on the other side of the entry:
    /// its `before` while the entry is on the undo stack, its `after` while it
    /// is on the redo stack. Re-encoded by every step through the entry.
    #[serde(rename = "d")]
    pub delta: ProjectDelta,
    /// What the history preview reads from the `before` state, which the
    /// entry no longer holds whole. Recorded once, at capture.
    #[serde(rename = "p", default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<BeforePreview>,
}

/// The facts a history preview reads from an entry's `before` state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BeforePreview {
    /// The section a Delete Section removed: its name and shot count.
    Section { name: Option<String>, shots: usize },
    /// The GPS of the datum a Remove Zero Datum removed, when it had one.
    Datum(Option<crate::models::GpsCoordinates>),
}

impl SnapshotDelta {
    /// The entry for a capture's two snapshots: the delta from `after` (the
    /// live project when the entry is pushed) to `before`.
    pub fn from_pair(
        before: &str,
        after: &str,
        preview: Option<BeforePreview>,
    ) -> Result<Self, DeltaError> {
        Ok(Self {
            delta: ProjectDelta::between_texts(after, before)?,
            preview,
        })
    }

    /// The serialized length of this payload: what it costs the history.
    pub fn encoded_len(&self) -> usize {
        serde_json::to_string(self).map_or(0, |json| json.len())
    }
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
        self.fields.is_empty() && self.sections.is_none()
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
        let ids = SectionIds::of_doc(&doc)?;
        let target = self.apply_doc(&doc, &ids)?.into_doc()?.text();
        serde_json::from_str(&target).map_err(|e| e.to_string())
    }

    /// [`Self::apply`] to a split project whose sections' ids `ids` lists, in
    /// order ([`SectionIds::of_project`] for the live project).
    pub fn apply_doc<'a>(
        &self,
        source: &Doc<'a>,
        ids: &[SectionIds],
    ) -> Result<Applied<'a>, DeltaError> {
        doc::apply(self, source, ids)
    }
}

fn shots_array(section: &Value) -> Option<&Vec<Value>> {
    section.get(SHOTS)?.as_array()
}

fn section_id_of(section: &Value) -> Option<u64> {
    section.get(ID)?.as_u64()
}

fn section_id_of_map(section: &Map<String, Value>) -> Option<SectionId> {
    SectionId::try_from(section.get(ID)?.as_u64()?).ok()
}

#[cfg(test)]
#[path = "delta_tests.rs"]
mod tests;
