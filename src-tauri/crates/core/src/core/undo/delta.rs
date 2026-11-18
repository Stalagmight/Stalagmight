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
//! back the recorded bits, and a delta has to as well.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use crate::models::{SectionId, FEATURE_ID_STRIDE};

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
    /// ([`flip_shot`]; the flag is its swap-left-right option). `m` edits the
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
    /// their keys ([`shot_rows`]), so the keys are not repeated per shot.
    #[serde(rename = "p", with = "shot_rows")]
    Put(Vec<Value>),
}

/// `ShotRun::Put` as the history stores it: consecutive objects with the same
/// keys become one table - the keys once, then a row of values per shot - and
/// anything else stays as it is. Read back to the same values in order.
mod shot_rows {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::{Map, Value};

    #[derive(Serialize, Deserialize)]
    enum Chunk {
        #[serde(rename = "o")]
        One(Value),
        #[serde(rename = "t")]
        Table { k: Vec<String>, r: Vec<Vec<Value>> },
    }

    fn keys_of(value: &Value) -> Option<Vec<&String>> {
        value.as_object().map(|object| object.keys().collect())
    }

    pub(super) fn serialize<S: Serializer>(values: &[Value], serializer: S) -> Result<S::Ok, S::Error> {
        let mut chunks: Vec<Chunk> = Vec::new();
        let mut i = 0;
        while i < values.len() {
            let keys = keys_of(&values[i]);
            let mut j = i + 1;
            while keys.is_some() && j < values.len() && keys_of(&values[j]) == keys {
                j += 1;
            }
            match keys {
                Some(keys) if j - i > 1 => chunks.push(Chunk::Table {
                    k: keys.into_iter().cloned().collect(),
                    r: values[i..j]
                        .iter()
                        .filter_map(Value::as_object)
                        .map(|object| object.values().cloned().collect())
                        .collect(),
                }),
                _ => chunks.extend(values[i..j].iter().cloned().map(Chunk::One)),
            }
            i = j;
        }
        chunks.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Value>, D::Error> {
        let chunks = Vec::<Chunk>::deserialize(deserializer)?;
        let mut out = Vec::new();
        for chunk in chunks {
            match chunk {
                Chunk::One(value) => out.push(value),
                Chunk::Table { k, r } => {
                    for row in r {
                        if row.len() != k.len() {
                            return Err(serde::de::Error::custom("a shot row does not match its keys"));
                        }
                        let object: Map<String, Value> = k.iter().cloned().zip(row).collect();
                        out.push(Value::Object(object));
                    }
                }
            }
        }
        Ok(out)
    }
}

/// One shot turned around the way Reverse Section turns it
/// (`crate::models::reverse_shots`): the bearing reversed, the depths and the
/// pitches swapped end for end (the pitches negated), and left and right
/// swapped when `swap_lr`. A member that is missing, or not a number where a
/// number is expected, is left as it is: whatever this does not reproduce,
/// the copy's edits (`m`) carry, so the result is exact either way.
fn flip_shot(shot: &Value, swap_lr: bool) -> Value {
    let Value::Object(members) = shot else {
        return shot.clone();
    };
    let mut out = members.clone();
    let number = |key: &str| members.get(key).and_then(Value::as_f64);
    let set = |out: &mut Map<String, Value>, key: &str, value: f64| {
        if let Some(n) = Number::from_f64(value) {
            out.insert(key.to_string(), Value::Number(n));
        }
    };
    if let (Some(a_in), Some(a_out)) = (number("azimuth_in"), number("azimuth_out")) {
        let (r_in, r_out) = crate::models::reversed_azimuths(a_in, a_out);
        set(&mut out, "azimuth_in", r_in);
        set(&mut out, "azimuth_out", r_out);
    }
    let swap = |out: &mut Map<String, Value>, a: &str, b: &str| {
        let (va, vb) = (members.get(a).cloned(), members.get(b).cloned());
        match vb {
            Some(v) => out.insert(a.to_string(), v),
            None => out.remove(a),
        };
        match va {
            Some(v) => out.insert(b.to_string(), v),
            None => out.remove(b),
        };
    };
    swap(&mut out, "depth_in", "depth_out");
    let negated = |key: &str| -> Option<Value> {
        match members.get(key)? {
            Value::Number(n) => n.as_f64().and_then(|f| Number::from_f64(-f)).map(Value::Number),
            other => Some(other.clone()),
        }
    };
    match negated("pitch_out") {
        Some(v) => out.insert("pitch_in".to_string(), v),
        None => out.remove("pitch_in"),
    };
    match negated("pitch_in") {
        Some(v) => out.insert("pitch_out".to_string(), v),
        None => out.remove("pitch_out"),
    };
    if swap_lr {
        swap(&mut out, "left", "right");
    }
    Value::Object(out)
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
        let before: Value = serde_json::from_str(before).map_err(|e| e.to_string())?;
        let after: Value = serde_json::from_str(after).map_err(|e| e.to_string())?;
        Ok(Self {
            delta: ProjectDelta::between(&after, &before)?,
            preview,
        })
    }

    /// The serialized length of this payload: what it costs the history.
    pub fn encoded_len(&self) -> usize {
        serde_json::to_string(self).map_or(0, |json| json.len())
    }
}

impl ProjectDelta {
    /// The edits that turn `source` into `target`, two serialized projects.
    pub fn between(source: &Value, target: &Value) -> Result<Self, DeltaError> {
        let (Value::Object(src), Value::Object(tgt)) = (source, target) else {
            return Err("a project snapshot is a JSON object".to_string());
        };
        let mut fields = BTreeMap::new();
        for (key, from) in src {
            if key == SECTIONS {
                continue;
            }
            match tgt.get(key) {
                None => {
                    fields.insert(key.clone(), Edit::Del);
                }
                Some(to) => {
                    if let Some(edit) = diff_member(key, from, to) {
                        fields.insert(key.clone(), edit);
                    }
                }
            }
        }
        for (key, to) in tgt {
            if key != SECTIONS && !src.contains_key(key) {
                fields.insert(key.clone(), Edit::Set(to.clone()));
            }
        }
        let src_sections = sections_of(src)?;
        let tgt_sections = sections_of(tgt)?;
        Ok(Self {
            fields,
            sections: encode_sections(src_sections, tgt_sections)
                .map(|runs| (src_sections.len(), runs)),
        })
    }

    /// True when the two states were identical.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.sections.is_none()
    }

    /// The target this delta was encoded towards, rebuilt from `source`.
    ///
    /// Shot ids are SETTLED on the way out ([`settle_ids`]): a shot this delta
    /// carries literally keeps the in-session id it was recorded with unless
    /// that id is now taken (the loader renumbers every shot, so after a
    /// reopen it can be), and every id allocator is raised past the ids it
    /// could otherwise hand out twice. In a session that has not reloaded
    /// nothing collides and this changes nothing.
    pub fn apply(&self, source: &Value) -> Result<Value, DeltaError> {
        let Value::Object(src) = source else {
            return Err("a project snapshot is a JSON object".to_string());
        };
        let mut out = apply_members(src, &self.fields, Some(SECTIONS))?;
        let src_sections = sections_of(src)?;
        let sections = match &self.sections {
            None => src_sections.to_vec(),
            Some((count, runs)) => {
                if *count != src_sections.len() {
                    return Err(format!(
                        "the delta was recorded over {count} sections, the source holds {}",
                        src_sections.len()
                    ));
                }
                let mut literal = Vec::new();
                let mut built = build_sections(src_sections, runs, &mut literal)?;
                settle_ids(&mut built, &literal)?;
                built
            }
        };
        settle_section_counter(&mut out, &sections);
        // Keep the member even when the source had none, so a target always
        // reads as a project.
        out.insert(SECTIONS.to_string(), Value::Array(sections));
        Ok(Value::Object(out))
    }

    /// Ids of the sections the TARGET holds whose shots this delta rebuilds
    /// and whose gauge pair it does not touch, where the SOURCE held no pair.
    ///
    /// A save captures a missing gauge pair with no undo entry of its own, so
    /// by the time this delta is applied the live project can hold a pair
    /// that neither recorded state had. A snapshot restore carries such a
    /// pair across only when the restored section can resolve it (see
    /// `reapply_heals_after_restore`); a delta, which keeps every member it
    /// does not edit, would carry it regardless. These are the sections where
    /// that can matter - the shots differ, so a pair naming a live shot can
    /// name one the target lacks - and the step applies the same resolve rule
    /// to them.
    pub fn ungauged_sections(&self) -> Vec<SectionId> {
        self.sections
            .iter()
            .flat_map(|(_, runs)| runs)
            .filter_map(|run| match run {
                SectionRun::Edit {
                    id, ungauged: true, ..
                } => Some(*id),
                _ => None,
            })
            .collect()
    }

    /// Every shot uuid this delta carries literally, with the section the
    /// target holds it in (a shot it puts back whole, or a uuid an edit writes onto a copied
    /// shot; gauge and station-key uuids are not shots).
    pub fn shot_uuid_mentions(&self) -> Vec<(SectionId, &str)> {
        let mut out = Vec::new();
        let Some((_, runs)) = &self.sections else {
            return out;
        };
        for run in runs {
            let (section, shots) = match run {
                SectionRun::Keep(..) => continue,
                SectionRun::Edit { id, shots, .. } => (Some(*id), shots),
                SectionRun::New { fields, shots } => (section_id_of_map(fields), shots),
            };
            let (Some(section), Some(shots)) = (section, shots) else {
                continue;
            };
            for shot_run in shots {
                match shot_run {
                    ShotRun::Put(literal) => out.extend(
                        literal
                            .iter()
                            .filter_map(|s| s.get(UUID)?.as_str())
                            .map(|uuid| (section, uuid)),
                    ),
                    ShotRun::Copy { m, .. } => out.extend(m.iter().filter_map(|(_, edit)| {
                        let Edit::Obj(members) = edit else {
                            return None;
                        };
                        match members.get(UUID) {
                            Some(Edit::Set(Value::String(uuid))) => Some((section, uuid.as_str())),
                            _ => None,
                        }
                    })),
                }
            }
        }
        out
    }

    /// Apply shot-uuid renames (the load's uuid heal, `uuid_remap.rs`) to
    /// everything this delta carries literally: each shot's `uuid` and each
    /// section's gauge pair, keyed by the section the target holds them in,
    /// and every station key anywhere in its values. The delta's runs over the
    /// source are positional and carry no uuid, so they need nothing. Returns
    /// whether anything changed.
    pub fn remap(&mut self, rename: &dyn Fn(SectionId, &str) -> Option<String>) -> bool {
        let mut changed = false;
        for edit in self.fields.values_mut() {
            changed |= edit_values_mut(edit, &mut |v| {
                crate::models::remap_station_keys_json(v, rename)
            });
        }
        let Some((_, runs)) = &mut self.sections else {
            return changed;
        };
        for run in runs {
            match run {
                SectionRun::Keep(..) => {}
                SectionRun::Edit {
                    id, fields, shots, ..
                } => {
                    let id = *id;
                    for (key, edit) in fields.iter_mut() {
                        if key == GAUGE_SHOT || key == GAUGE_NEIGHBOUR {
                            if let Edit::Set(value) = edit {
                                changed |= rename_string(value, id, rename);
                            }
                        }
                        changed |= edit_values_mut(edit, &mut |v| {
                            crate::models::remap_station_keys_json(v, rename)
                        });
                    }
                    if let Some(shots) = shots {
                        changed |= remap_shot_runs(shots, id, rename);
                    }
                }
                SectionRun::New { fields, shots } => {
                    let Some(id) = section_id_of_map(fields) else {
                        continue;
                    };
                    for key in [GAUGE_SHOT, GAUGE_NEIGHBOUR] {
                        if let Some(value) = fields.get_mut(key) {
                            changed |= rename_string(value, id, rename);
                        }
                    }
                    if let Some(Value::Array(literal)) = fields.get_mut(SHOTS) {
                        for shot in literal {
                            if let Some(value) = shot.get_mut(UUID) {
                                changed |= rename_string(value, id, rename);
                            }
                        }
                    }
                    for value in fields.values_mut() {
                        changed |= crate::models::remap_station_keys_json(value, rename);
                    }
                    if let Some(shots) = shots {
                        changed |= remap_shot_runs(shots, id, rename);
                    }
                }
            }
        }
        changed
    }

    /// `view` - which shots, by uuid, each section of the SOURCE holds, in list
    /// order - stepped through this delta: the same view of the target. `None`
    /// when the delta does not fit the view (its recorded section count, or a
    /// run past the end).
    ///
    /// What the load's uuid remap walks a history with (`uuid_remap.rs`): it
    /// needs to know which section holds which shot in every recorded state,
    /// and a delta carries no state whole. A section the delta does not touch
    /// is shared with `view`, so a step costs the sections it touches.
    pub fn step_holders(&self, view: &[HolderSection]) -> Option<Vec<HolderSection>> {
        let Some((count, runs)) = &self.sections else {
            return Some(view.to_vec());
        };
        if *count != view.len() {
            return None;
        }
        let mut out = Vec::with_capacity(view.len());
        for run in runs {
            match run {
                SectionRun::Keep(at, len) => {
                    out.extend_from_slice(view.get(*at..at.checked_add(*len)?)?);
                }
                SectionRun::Edit { at, id, shots, .. } => {
                    let (held_id, held) = view.get(*at)?;
                    if held_id != id {
                        return None;
                    }
                    let shots = match shots {
                        None => std::rc::Rc::clone(held),
                        Some(runs) => std::rc::Rc::new(holder_shots(view, runs)?),
                    };
                    out.push((*id, shots));
                }
                SectionRun::New { fields, shots } => {
                    let id = section_id_of_map(fields)?;
                    let held = match shots {
                        Some(runs) => holder_shots(view, runs)?,
                        None => fields
                            .get(SHOTS)
                            .and_then(Value::as_array)
                            .map(|literal| literal.iter().map(literal_uuid).collect())
                            .unwrap_or_default(),
                    };
                    out.push((id, std::rc::Rc::new(held)));
                }
            }
        }
        Some(out)
    }

    /// Pin a renamed uuid onto every shot this delta COPIES into a section
    /// under an id that `rename` renames there. `view` is the holder view of
    /// the source this delta will be applied to.
    ///
    /// A copy carries no uuid - it takes the source shot's - so [`Self::remap`]
    /// cannot reach it, yet a copy across sections (a split, a combine, or a
    /// shot re-created from a same-uuid shot elsewhere) can land an id the
    /// load's heal renamed in that section. The pin is an ordinary uuid edit
    /// on the copy. Returns whether anything changed.
    pub fn rename_copies(
        &mut self,
        view: &[HolderSection],
        rename: &dyn Fn(SectionId, &str) -> Option<String>,
    ) -> bool {
        let Some((_, runs)) = &mut self.sections else {
            return false;
        };
        let mut changed = false;
        for run in runs {
            let (section, shots) = match run {
                SectionRun::Keep(..) => continue,
                SectionRun::Edit { id, shots, .. } => (Some(*id), shots),
                SectionRun::New { fields, shots } => (section_id_of_map(fields), shots),
            };
            let (Some(section), Some(shots)) = (section, shots) else {
                continue;
            };
            for shot_run in shots {
                let ShotRun::Copy {
                    sec, at, n, rev, m, ..
                } = shot_run
                else {
                    continue;
                };
                let Some((_, held)) = view.get(*sec) else {
                    continue;
                };
                for k in 0..*n {
                    let index = if *rev { at.checked_sub(k) } else { at.checked_add(k) };
                    let Some(source) = index.and_then(|i| held.get(i)) else {
                        break;
                    };
                    let slot = m.iter().position(|(offset, _)| *offset == k);
                    let pinned = slot.and_then(|i| match &m[i].1 {
                        Edit::Obj(members) => members.get(UUID),
                        _ => None,
                    });
                    let effective = match pinned {
                        Some(Edit::Set(value)) => value.as_str().map(str::to_string),
                        Some(Edit::Del) => None,
                        _ => source.as_deref().map(str::to_string),
                    };
                    let Some(new) = effective.as_deref().and_then(|u| rename(section, u)) else {
                        continue;
                    };
                    let edit = Edit::Set(Value::String(new));
                    match slot {
                        Some(i) => match &mut m[i].1 {
                            Edit::Obj(members) => {
                                members.insert(UUID.to_string(), edit);
                            }
                            other => {
                                // A copy edited whole is not a copy; leave it.
                                let _ = other;
                                continue;
                            }
                        },
                        None => {
                            let mut members = BTreeMap::new();
                            members.insert(UUID.to_string(), edit);
                            let i = m.partition_point(|(offset, _)| *offset < k);
                            m.insert(i, (k, Edit::Obj(members)));
                        }
                    }
                    changed = true;
                }
            }
        }
        changed
    }

    /// The facts about one section of the TARGET that a history preview reads
    /// without the project: its name and shot count, when this delta carries
    /// the section whole (a section a delete removed, on the undo side).
    pub fn new_section(&self, id: SectionId) -> Option<(Option<String>, usize)> {
        self.sections.iter().flat_map(|(_, runs)| runs).find_map(|run| match run {
            SectionRun::New { fields, shots } if section_id_of_map(fields) == Some(id) => {
                let name = fields
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let count = match shots {
                    Some(runs) => runs
                        .iter()
                        .map(|r| match r {
                            ShotRun::Copy { n, .. } => *n,
                            ShotRun::Put(v) => v.len(),
                        })
                        .sum(),
                    None => fields.get(SHOTS).and_then(Value::as_array).map_or(0, Vec::len),
                };
                Some((name, count))
            }
            _ => None,
        })
    }
}

/// One section of a holder view ([`ProjectDelta::step_holders`]): its id and
/// its shots' uuids in list order (`None` for a shot without one).
pub type HolderSection = (SectionId, std::rc::Rc<Vec<Option<std::rc::Rc<str>>>>);

fn literal_uuid(shot: &Value) -> Option<std::rc::Rc<str>> {
    shot.get(UUID).and_then(Value::as_str).map(std::rc::Rc::from)
}

fn holder_shots(view: &[HolderSection], runs: &[ShotRun]) -> Option<Vec<Option<std::rc::Rc<str>>>> {
    let mut out = Vec::new();
    for run in runs {
        match run {
            ShotRun::Copy {
                sec, at, n, rev, m, ..
            } => {
                let (_, shots) = view.get(*sec)?;
                let mut mods = m.iter().peekable();
                for k in 0..*n {
                    let index = if *rev { at.checked_sub(k)? } else { at.checked_add(k)? };
                    let mut uuid = shots.get(index)?.as_ref().map(std::rc::Rc::clone);
                    if let Some((_, Edit::Obj(members))) = mods.next_if(|(offset, _)| *offset == k) {
                        match members.get(UUID) {
                            Some(Edit::Set(value)) => {
                                uuid = value.as_str().map(std::rc::Rc::from);
                            }
                            Some(Edit::Del) => uuid = None,
                            _ => {}
                        }
                    }
                    out.push(uuid);
                }
            }
            ShotRun::Put(literal) => out.extend(literal.iter().map(literal_uuid)),
        }
    }
    Some(out)
}

fn rename_string(
    value: &mut Value,
    section: SectionId,
    rename: &dyn Fn(SectionId, &str) -> Option<String>,
) -> bool {
    let Some(new) = value.as_str().and_then(|old| rename(section, old)) else {
        return false;
    };
    *value = Value::String(new);
    true
}

fn remap_shot_runs(
    runs: &mut [ShotRun],
    section: SectionId,
    rename: &dyn Fn(SectionId, &str) -> Option<String>,
) -> bool {
    let mut changed = false;
    for run in runs {
        match run {
            ShotRun::Put(literal) => {
                for shot in literal {
                    if let Some(value) = shot.get_mut(UUID) {
                        changed |= rename_string(value, section, rename);
                    }
                    changed |= crate::models::remap_station_keys_json(shot, rename);
                }
            }
            ShotRun::Copy { m, .. } => {
                for (_, edit) in m {
                    if let Edit::Obj(members) = edit {
                        if let Some(Edit::Set(value)) = members.get_mut(UUID) {
                            changed |= rename_string(value, section, rename);
                        }
                    }
                    changed |= edit_values_mut(edit, &mut |v| {
                        crate::models::remap_station_keys_json(v, rename)
                    });
                }
            }
        }
    }
    changed
}

/// Visit every literal value inside `edit` mutably; `f` returns whether it
/// changed one.
fn edit_values_mut(edit: &mut Edit, f: &mut dyn FnMut(&mut Value) -> bool) -> bool {
    match edit {
        Edit::Set(value) => f(value),
        Edit::Del => false,
        Edit::Obj(members) => {
            let mut changed = false;
            for member in members.values_mut() {
                changed |= edit_values_mut(member, f);
            }
            changed
        }
        Edit::Arr(_, runs) => {
            let mut changed = false;
            for run in runs {
                match run {
                    Run::Keep(..) => {}
                    Run::Put(values) => {
                        for value in values {
                            changed |= f(value);
                        }
                    }
                    Run::Mod(_, inner) => changed |= edit_values_mut(inner, f),
                }
            }
            changed
        }
        Edit::Bag { del, add } => {
            let mut changed = false;
            for value in del.iter_mut().chain(add.iter_mut().map(|(_, v)| v)) {
                changed |= f(value);
            }
            changed
        }
    }
}

fn sections_of(project: &Map<String, Value>) -> Result<&[Value], DeltaError> {
    match project.get(SECTIONS) {
        None => Ok(&[]),
        Some(Value::Array(sections)) => Ok(sections),
        Some(_) => Err("a project snapshot's sections are an array".to_string()),
    }
}

fn section_id_of(section: &Value) -> Option<u64> {
    section.get(ID)?.as_u64()
}

fn section_id_of_map(section: &Map<String, Value>) -> Option<SectionId> {
    SectionId::try_from(section.get(ID)?.as_u64()?).ok()
}

// === Bitwise equality and hashing ===

/// Which of serde_json's three number representations a number has: equal
/// values in different representations are different JSON.
fn number_kind(n: &Number) -> u8 {
    if n.is_f64() {
        0
    } else if n.is_u64() {
        1
    } else {
        2
    }
}

fn same_number(a: &Number, b: &Number) -> bool {
    if number_kind(a) != number_kind(b) {
        return false;
    }
    if a.is_f64() {
        return a.as_f64().map(f64::to_bits) == b.as_f64().map(f64::to_bits);
    }
    if a.is_u64() {
        return a.as_u64() == b.as_u64();
    }
    a.as_i64() == b.as_i64()
}

/// JSON equality with floats compared by bits.
pub(crate) fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => same_number(x, y),
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x
                    .iter()
                    .zip(y)
                    .all(|((ka, va), (kb, vb))| ka == kb && same(va, vb))
        }
        _ => false,
    }
}

fn hash_into(value: &Value, h: &mut DefaultHasher) {
    match value {
        Value::Null => 0u8.hash(h),
        Value::Bool(b) => {
            1u8.hash(h);
            b.hash(h);
        }
        Value::Number(n) => {
            2u8.hash(h);
            number_kind(n).hash(h);
            if let Some(f) = n.as_f64().filter(|_| n.is_f64()) {
                f.to_bits().hash(h);
            } else if let Some(u) = n.as_u64() {
                u.hash(h);
            } else if let Some(i) = n.as_i64() {
                i.hash(h);
            }
        }
        Value::String(s) => {
            3u8.hash(h);
            s.hash(h);
        }
        Value::Array(items) => {
            4u8.hash(h);
            items.len().hash(h);
            for item in items {
                hash_into(item, h);
            }
        }
        Value::Object(members) => {
            5u8.hash(h);
            members.len().hash(h);
            for (key, member) in members {
                key.hash(h);
                hash_into(member, h);
            }
        }
    }
}

fn hash_of(value: &Value) -> u64 {
    let mut h = DefaultHasher::new();
    hash_into(value, &mut h);
    h.finish()
}

// === Generic diff ===

/// The edit that turns `from` into `to`, or `None` when they are equal.
pub(crate) fn diff(from: &Value, to: &Value) -> Option<Edit> {
    if same(from, to) {
        return None;
    }
    Some(match (from, to) {
        (Value::Object(a), Value::Object(b)) => Edit::Obj(diff_members(a, b)),
        (Value::Array(a), Value::Array(b)) if b.len() > SMALL_ARRAY && !a.is_empty() => {
            Edit::Arr(a.len(), diff_array(a, b))
        }
        _ => Edit::Set(to.clone()),
    })
}

/// [`diff`] for a top-level member of the project, which knows which lists are
/// unordered ([`UNORDERED_MEMBERS`]) and which are short ([`SMALL_LIST`]).
fn diff_member(key: &str, from: &Value, to: &Value) -> Option<Edit> {
    if same(from, to) {
        return None;
    }
    match (from, to) {
        (Value::Array(a), Value::Array(b)) if UNORDERED_MEMBERS.contains(&key) => {
            Some(diff_bag(a, b))
        }
        (Value::Array(_), Value::Array(b)) if b.len() <= SMALL_LIST => Some(Edit::Set(to.clone())),
        _ => diff(from, to),
    }
}

/// The elements of `from` that `to` lacks and the reverse, as multisets.
fn diff_bag(from: &[Value], to: &[Value]) -> Edit {
    let mut unmatched: HashMap<u64, Vec<usize>> = HashMap::new();
    for (j, value) in to.iter().enumerate() {
        unmatched.entry(hash_of(value)).or_default().push(j);
    }
    let mut matched = vec![false; to.len()];
    let mut del = Vec::new();
    for value in from {
        let slot = unmatched.get_mut(&hash_of(value)).and_then(|candidates| {
            let k = candidates.iter().position(|&j| same(&to[j], value))?;
            Some(candidates.swap_remove(k))
        });
        match slot {
            Some(j) => matched[j] = true,
            None => del.push(value.clone()),
        }
    }
    let add = to
        .iter()
        .enumerate()
        .zip(&matched)
        .filter(|(_, hit)| !**hit)
        .map(|((j, value), _)| (j, value.clone()))
        .collect();
    Edit::Bag { del, add }
}

fn diff_members(from: &Map<String, Value>, to: &Map<String, Value>) -> BTreeMap<String, Edit> {
    let mut out = BTreeMap::new();
    for (key, a) in from {
        match to.get(key) {
            None => {
                out.insert(key.clone(), Edit::Del);
            }
            Some(b) => {
                if let Some(edit) = diff(a, b) {
                    out.insert(key.clone(), edit);
                }
            }
        }
    }
    for (key, b) in to {
        if !from.contains_key(key) {
            out.insert(key.clone(), Edit::Set(b.clone()));
        }
    }
    out
}

/// The target array as runs over the source: a common prefix and suffix, and a
/// shortest edit script (Myers) over the middle, bounded by
/// [`MAX_ARRAY_EDITS`].
fn diff_array(from: &[Value], to: &[Value]) -> Vec<Run> {
    let mut prefix = 0;
    while prefix < from.len() && prefix < to.len() && same(&from[prefix], &to[prefix]) {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < from.len() - prefix
        && suffix < to.len() - prefix
        && same(&from[from.len() - 1 - suffix], &to[to.len() - 1 - suffix])
    {
        suffix += 1;
    }
    let a = &from[prefix..from.len() - suffix];
    let b = &to[prefix..to.len() - suffix];
    let matches = myers_matches(a, b).unwrap_or_default();

    let mut runs = RunBuilder::default();
    runs.keep(0, prefix);
    let (mut i, mut j) = (0, 0);
    for (mi, mj) in matches.into_iter().chain(std::iter::once((a.len(), b.len()))) {
        runs.hunk(&a[i..mi], prefix + i, &b[j..mj]);
        if mi < a.len() {
            runs.keep(prefix + mi, 1);
        }
        i = mi + 1;
        j = mj + 1;
    }
    runs.keep(from.len() - suffix, suffix);
    runs.finish()
}

/// Builds a run list, merging adjacent `Keep`s and `Put`s.
#[derive(Default)]
struct RunBuilder {
    runs: Vec<Run>,
}

impl RunBuilder {
    fn keep(&mut self, at: usize, len: usize) {
        if len == 0 {
            return;
        }
        if let Some(Run::Keep(start, n)) = self.runs.last_mut() {
            if *start + *n == at {
                *n += len;
                return;
            }
        }
        self.runs.push(Run::Keep(at, len));
    }

    fn put(&mut self, value: &Value) {
        if let Some(Run::Put(values)) = self.runs.last_mut() {
            values.push(value.clone());
            return;
        }
        self.runs.push(Run::Put(vec![value.clone()]));
    }

    /// A stretch where the source's `removed` (starting at `at`) became
    /// `added`. Equal lengths pair element by element - an edited element is a
    /// `Mod` when it is an object or array that changed in part - otherwise
    /// the added elements are literal.
    fn hunk(&mut self, removed: &[Value], at: usize, added: &[Value]) {
        if removed.len() == added.len() {
            for (k, (old, new)) in removed.iter().zip(added).enumerate() {
                match diff(old, new) {
                    Some(Edit::Set(_)) | None => self.put(new),
                    Some(edit) => self.runs.push(Run::Mod(at + k, Box::new(edit))),
                }
            }
            return;
        }
        for value in added {
            self.put(value);
        }
    }

    fn finish(self) -> Vec<Run> {
        self.runs
    }
}

/// Matched index pairs of a shortest edit script between `a` and `b`, in
/// ascending order, or `None` when it needs more than [`MAX_ARRAY_EDITS`]
/// edits (the caller then treats the whole middle as changed). The classic
/// greedy forward pass with a saved trace, walked back from the end.
fn myers_matches(a: &[Value], b: &[Value]) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 || m == 0 {
        return Some(Vec::new());
    }
    let ha: Vec<u64> = a.iter().map(hash_of).collect();
    let hb: Vec<u64> = b.iter().map(hash_of).collect();
    let equal = |x: isize, y: isize| {
        let (x, y) = (x as usize, y as usize);
        ha[x] == hb[y] && same(&a[x], &b[y])
    };
    let max = (n + m).min(MAX_ARRAY_EDITS as isize);
    let at = |k: isize| (k + max) as usize;
    let mut v = vec![0isize; 2 * max as usize + 2];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut done = false;
    for d in 0..=max {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && equal(x, y) {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                done = true;
                break;
            }
            k += 2;
        }
        if done {
            break;
        }
    }
    if !done {
        return None;
    }
    let mut matches = Vec::new();
    let (mut x, mut y) = (n, m);
    for (d, v) in trace.iter().enumerate().rev() {
        let d = d as isize;
        let k = x - y;
        let prev_k = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = v[at(prev_k)];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
            matches.push((x as usize, y as usize));
        }
        if d > 0 {
            x = prev_x;
            y = prev_y;
        }
    }
    matches.reverse();
    Some(matches)
}

// === Generic apply ===

fn apply_edit(from: &Value, edit: &Edit) -> Result<Value, DeltaError> {
    match edit {
        Edit::Set(value) => Ok(value.clone()),
        Edit::Del => Err("a delete edits an object member, not a value".to_string()),
        Edit::Obj(members) => match from {
            Value::Object(object) => Ok(Value::Object(apply_members(object, members, None)?)),
            _ => Err("an object edit met a value that is not an object".to_string()),
        },
        Edit::Arr(count, runs) => match from {
            Value::Array(items) if items.len() == *count => Ok(Value::Array(apply_runs(items, runs)?)),
            Value::Array(items) => Err(format!(
                "an array edit was recorded over {count} elements, the source holds {}",
                items.len()
            )),
            _ => Err("an array edit met a value that is not an array".to_string()),
        },
        Edit::Bag { del, add } => match from {
            Value::Array(items) => Ok(Value::Array(apply_bag(items, del, add))),
            _ => Err("a bag edit met a value that is not an array".to_string()),
        },
    }
}

/// `from` without one element equal to each of `del`, and with each of `add`
/// inserted at its recorded index (clamped). An element to remove that is not
/// there is skipped: a load or a save pruned it already.
fn apply_bag(from: &[Value], del: &[Value], add: &[(usize, Value)]) -> Vec<Value> {
    let mut by_hash: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, value) in from.iter().enumerate() {
        by_hash.entry(hash_of(value)).or_default().push(i);
    }
    let mut removed = vec![false; from.len()];
    for value in del {
        if let Some(candidates) = by_hash.get_mut(&hash_of(value)) {
            if let Some(k) = candidates.iter().position(|&i| same(&from[i], value)) {
                removed[candidates.swap_remove(k)] = true;
            }
        }
    }
    let mut out: Vec<Value> = from
        .iter()
        .zip(&removed)
        .filter(|(_, gone)| !**gone)
        .map(|(value, _)| value.clone())
        .collect();
    for (index, value) in add {
        out.insert((*index).min(out.len()), value.clone());
    }
    out
}

/// `from` with `edits` applied, leaving out the member named `skip`.
///
/// Tolerant of one thing only: deleting a member that is already absent is a
/// no-op. A save drops state the history never recorded (an orphan offset
/// key), and an entry that removes the same thing must still step through.
fn apply_members(
    from: &Map<String, Value>,
    edits: &BTreeMap<String, Edit>,
    skip: Option<&str>,
) -> Result<Map<String, Value>, DeltaError> {
    let mut out = Map::new();
    for (key, value) in from {
        if Some(key.as_str()) == skip {
            continue;
        }
        match edits.get(key) {
            None => {
                out.insert(key.clone(), value.clone());
            }
            Some(Edit::Del) => {}
            Some(edit) => {
                out.insert(key.clone(), apply_edit(value, edit)?);
            }
        }
    }
    for (key, edit) in edits {
        if from.contains_key(key) {
            continue;
        }
        match edit {
            Edit::Set(value) => {
                out.insert(key.clone(), value.clone());
            }
            Edit::Del => {}
            _ => return Err(format!("an edit names member {key:?}, which the source lacks")),
        }
    }
    Ok(out)
}

fn apply_runs(from: &[Value], runs: &[Run]) -> Result<Vec<Value>, DeltaError> {
    let mut out = Vec::with_capacity(from.len());
    for run in runs {
        match run {
            Run::Keep(at, len) => {
                let end = at.checked_add(*len).filter(|end| *end <= from.len());
                let Some(end) = end else {
                    return Err(format!(
                        "an array run keeps {len} elements from {at}, past the source's {}",
                        from.len()
                    ));
                };
                out.extend_from_slice(&from[*at..end]);
            }
            Run::Put(values) => out.extend(values.iter().cloned()),
            Run::Mod(at, edit) => {
                let Some(item) = from.get(*at) else {
                    return Err(format!("an array run edits element {at}, past the source"));
                };
                out.push(apply_edit(item, edit)?);
            }
        }
    }
    Ok(out)
}

// === Sections and shots ===

/// The source's shots, indexed by uuid, built on first use: only an edited or
/// new section needs it.
struct ShotPool<'a> {
    sections: &'a [Value],
    by_uuid: Option<HashMap<&'a str, Vec<(usize, usize)>>>,
}

impl<'a> ShotPool<'a> {
    fn new(sections: &'a [Value]) -> Self {
        Self {
            sections,
            by_uuid: None,
        }
    }

    fn shots(&self, section: usize) -> &'a [Value] {
        self.sections
            .get(section)
            .and_then(|s| s.get(SHOTS))
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }

    fn index(&mut self) -> &HashMap<&'a str, Vec<(usize, usize)>> {
        let sections = self.sections;
        self.by_uuid.get_or_insert_with(|| {
            let mut index: HashMap<&'a str, Vec<(usize, usize)>> = HashMap::new();
            for (p, section) in sections.iter().enumerate() {
                let shots = section
                    .get(SHOTS)
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice);
                for (i, shot) in shots.iter().enumerate() {
                    if let Some(uuid) = shot.get(UUID).and_then(Value::as_str) {
                        index.entry(uuid).or_default().push((p, i));
                    }
                }
            }
            index
        })
    }

    /// The target shots `target` as runs over the source's shots. `home` is
    /// the source position of the section being rebuilt, preferred when a
    /// uuid is held in several places.
    fn rebuild(&mut self, target: &[Value], home: Option<usize>) -> Vec<ShotRun> {
        let mut out = Vec::new();
        let mut open: Option<OpenCopy> = None;
        for (t, shot) in target.iter().enumerate() {
            let candidates: Vec<(usize, usize)> = shot
                .get(UUID)
                .and_then(Value::as_str)
                .and_then(|uuid| self.index().get(uuid))
                .cloned()
                .unwrap_or_default();
            let extended = open
                .as_ref()
                .and_then(|run| run.continuation(&candidates));
            let chosen = match extended {
                Some(next) => Some(next),
                None => {
                    if let Some(run) = open.take() {
                        out.push({
                        let shots = self.shots(run.sec);
                        run.finish(shots, target)
                    });
                    }
                    candidates
                        .iter()
                        .find(|(p, _)| Some(*p) == home)
                        .or_else(|| candidates.first())
                        .copied()
                        .map(|(p, i)| (p, i, None))
                }
            };
            let Some((p, i, rev)) = chosen else {
                push_put(&mut out, shot);
                continue;
            };
            let edit = self.shots(p).get(i).and_then(|source| diff(source, shot));
            if matches!(edit, Some(Edit::Set(_))) {
                // Not an object any more: nothing to share with the source.
                if let Some(run) = open.take() {
                    out.push({
                        let shots = self.shots(run.sec);
                        run.finish(shots, target)
                    });
                }
                push_put(&mut out, shot);
                continue;
            }
            match open.as_mut() {
                Some(run) if extended.is_some() => run.extend(rev),
                _ => {
                    open = Some(OpenCopy {
                        sec: p,
                        at: i,
                        n: 1,
                        rev: None,
                        t0: t,
                    });
                }
            }
        }
        if let Some(run) = open {
            out.push({
                        let shots = self.shots(run.sec);
                        run.finish(shots, target)
                    });
        }
        out
    }
}

fn push_put(out: &mut Vec<ShotRun>, shot: &Value) {
    if let Some(ShotRun::Put(values)) = out.last_mut() {
        values.push(shot.clone());
        return;
    }
    out.push(ShotRun::Put(vec![shot.clone()]));
}

/// A copy run being extended shot by shot; its edits are worked out when it
/// is finished.
struct OpenCopy {
    sec: usize,
    at: usize,
    n: usize,
    /// `None` until the second shot fixes the direction.
    rev: Option<bool>,
    /// Where the run starts in the target.
    t0: usize,
}

impl OpenCopy {
    /// The source shot that would continue this run, if it is a candidate,
    /// with the direction that continuing it fixes.
    fn continuation(&self, candidates: &[(usize, usize)]) -> Option<(usize, usize, Option<bool>)> {
        let up = self.at + self.n;
        let down = self.at.checked_sub(self.n);
        if self.rev != Some(true) && candidates.contains(&(self.sec, up)) {
            return Some((self.sec, up, Some(false)));
        }
        if self.rev != Some(false) {
            if let Some(down) = down.filter(|d| candidates.contains(&(self.sec, *d))) {
                return Some((self.sec, down, Some(true)));
            }
        }
        None
    }

    fn extend(&mut self, rev: Option<bool>) {
        if self.rev.is_none() {
            self.rev = rev;
        }
        self.n += 1;
    }

    /// The finished run over `source` (the copied section's shots) towards
    /// `target`. A run copied in reverse is a reversed section, so its copies
    /// are also tried turned around ([`flip_shot`], with and without the
    /// left-right swap): whichever leaves the fewest edits is kept.
    fn finish(self, source: &[Value], target: &[Value]) -> ShotRun {
        let rev = self.rev == Some(true);
        let mods_for = |flip: Option<bool>| -> Vec<(usize, Edit)> {
            (0..self.n)
                .filter_map(|k| {
                    let index = if rev { self.at.checked_sub(k)? } else { self.at + k };
                    let from = source.get(index)?;
                    let to = target.get(self.t0 + k)?;
                    let edit = match flip {
                        Some(swap_lr) => diff(&flip_shot(from, swap_lr), to),
                        None => diff(from, to),
                    };
                    Some((k, edit?))
                })
                .collect()
        };
        let candidates: &[Option<bool>] = if rev {
            &[None, Some(false), Some(true)]
        } else {
            &[None]
        };
        let (flip, m) = candidates
            .iter()
            .map(|flip| (*flip, mods_for(*flip)))
            .min_by_key(|(_, mods)| serde_json::to_string(mods).map_or(usize::MAX, |json| json.len()))
            .unwrap_or((None, Vec::new()));
        ShotRun::Copy {
            sec: self.sec,
            at: self.at,
            n: self.n,
            rev,
            flip,
            m,
        }
    }
}

fn shots_array(section: &Value) -> Option<&Vec<Value>> {
    section.get(SHOTS)?.as_array()
}

fn gauge_of(section: &Value) -> (Option<&str>, Option<&str>) {
    (
        section.get(GAUGE_SHOT).and_then(Value::as_str),
        section.get(GAUGE_NEIGHBOUR).and_then(Value::as_str),
    )
}

fn encode_sections(source: &[Value], target: &[Value]) -> Option<Vec<SectionRun>> {
    if source.len() == target.len() && source.iter().zip(target).all(|(a, b)| same(a, b)) {
        return None;
    }
    let mut by_id: HashMap<u64, usize> = HashMap::new();
    for (p, section) in source.iter().enumerate() {
        if let Some(id) = section_id_of(section) {
            by_id.entry(id).or_insert(p);
        }
    }
    let mut pool = ShotPool::new(source);
    let mut runs: Vec<SectionRun> = Vec::new();
    for section in target {
        let at = section_id_of(section).and_then(|id| by_id.get(&id).copied());
        match at {
            Some(at) if same(&source[at], section) => {
                if let Some(SectionRun::Keep(start, n)) = runs.last_mut() {
                    if *start + *n == at {
                        *n += 1;
                        continue;
                    }
                }
                runs.push(SectionRun::Keep(at, 1));
            }
            Some(at) => runs.push(edit_section(&source[at], section, at, &mut pool)),
            None => runs.push(new_section(section, &mut pool)),
        }
    }
    Some(runs)
}

fn edit_section(from: &Value, to: &Value, at: usize, pool: &mut ShotPool<'_>) -> SectionRun {
    let id = section_id_of(from)
        .and_then(|id| SectionId::try_from(id).ok())
        .unwrap_or_default();
    let (Value::Object(a), Value::Object(b)) = (from, to) else {
        // Unreachable for a serialized project; keep the result exact anyway.
        return SectionRun::New {
            fields: to.as_object().cloned().unwrap_or_default(),
            shots: None,
        };
    };
    let (Some(from_shots), Some(to_shots)) = (shots_array(from), shots_array(to)) else {
        return SectionRun::Edit {
            at,
            id,
            fields: diff_members(a, b),
            shots: None,
            ungauged: false,
        };
    };
    let mut fields = diff_members(a, b);
    fields.remove(SHOTS);
    let same_shots = from_shots.len() == to_shots.len()
        && from_shots.iter().zip(to_shots).all(|(x, y)| same(x, y));
    let shots = (!same_shots).then(|| pool.rebuild(to_shots, Some(at)));
    let gauge_untouched = !fields.contains_key(GAUGE_SHOT) && !fields.contains_key(GAUGE_NEIGHBOUR);
    let ungauged = shots.is_some() && gauge_untouched && gauge_of(from).0.is_none();
    SectionRun::Edit {
        at,
        id,
        fields,
        shots,
        ungauged,
    }
}

fn new_section(to: &Value, pool: &mut ShotPool<'_>) -> SectionRun {
    let mut fields = to.as_object().cloned().unwrap_or_default();
    let shots = match fields.get(SHOTS) {
        Some(Value::Array(shots)) => {
            let runs = pool.rebuild(shots, None);
            fields.remove(SHOTS);
            Some(runs)
        }
        _ => None,
    };
    SectionRun::New { fields, shots }
}

/// Where in the target a shot that the delta carried literally landed:
/// (section index, shot index).
type LiteralShot = (usize, usize);

fn build_sections(
    source: &[Value],
    runs: &[SectionRun],
    literal: &mut Vec<LiteralShot>,
) -> Result<Vec<Value>, DeltaError> {
    let mut out: Vec<Value> = Vec::with_capacity(source.len());
    for run in runs {
        match run {
            SectionRun::Keep(at, len) => {
                let end = at.checked_add(*len).filter(|end| *end <= source.len());
                let Some(end) = end else {
                    return Err(format!(
                        "a section run keeps {len} sections from {at}, past the source's {}",
                        source.len()
                    ));
                };
                out.extend_from_slice(&source[*at..end]);
            }
            SectionRun::Edit {
                at,
                id,
                fields,
                shots,
                ..
            } => {
                let Some(Value::Object(base)) = source.get(*at) else {
                    return Err(format!("a section run edits section {at}, past the source"));
                };
                if section_id_of_map(base) != Some(*id) {
                    return Err(format!(
                        "a section run edits section {id} at {at}, which holds another section"
                    ));
                }
                let skip = shots.as_ref().map(|_| SHOTS);
                let mut section = apply_members(base, fields, skip)?;
                if let Some(shot_runs) = shots {
                    let built = build_shots(source, shot_runs, out.len(), literal)?;
                    section.insert(SHOTS.to_string(), Value::Array(built));
                }
                out.push(Value::Object(section));
            }
            SectionRun::New { fields, shots } => {
                let mut section = fields.clone();
                if let Some(shot_runs) = shots {
                    let built = build_shots(source, shot_runs, out.len(), literal)?;
                    section.insert(SHOTS.to_string(), Value::Array(built));
                }
                out.push(Value::Object(section));
            }
        }
    }
    Ok(out)
}

fn build_shots(
    source: &[Value],
    runs: &[ShotRun],
    section_index: usize,
    literal: &mut Vec<LiteralShot>,
) -> Result<Vec<Value>, DeltaError> {
    let mut out = Vec::new();
    for run in runs {
        match run {
            ShotRun::Copy {
                sec,
                at,
                n,
                rev,
                flip,
                m,
            } => {
                let shots = source
                    .get(*sec)
                    .and_then(shots_array)
                    .ok_or_else(|| format!("a shot run copies from section {sec}, past the source"))?;
                let mut mods = m.iter().peekable();
                for k in 0..*n {
                    let index = if *rev { at.checked_sub(k) } else { at.checked_add(k) };
                    let Some(shot) = index.and_then(|i| shots.get(i)) else {
                        return Err(format!(
                            "a shot run copies {n} shots from {at} of section {sec}, past its {}",
                            shots.len()
                        ));
                    };
                    let flipped;
                    let base = match flip {
                        Some(swap_lr) => {
                            flipped = flip_shot(shot, *swap_lr);
                            &flipped
                        }
                        None => shot,
                    };
                    match mods.next_if(|(offset, _)| *offset == k) {
                        Some((_, edit)) => out.push(apply_edit(base, edit)?),
                        None => out.push(base.clone()),
                    }
                }
                if mods.next().is_some() {
                    return Err("a shot run edits a copy past its end".to_string());
                }
            }
            ShotRun::Put(values) => {
                for value in values {
                    literal.push((section_index, out.len()));
                    out.push(value.clone());
                }
            }
        }
    }
    Ok(out)
}

// === Settling ids ===

fn shot_id_of(shot: &Value) -> Option<u64> {
    shot.get(ID)?.as_u64()
}

/// Make every shot id in `sections` unique and every section's allocator
/// safe, after a rebuild that put `literal` shots back with their recorded
/// ids.
///
/// The loader renumbers every shot (`section_id * FEATURE_ID_STRIDE +
/// sequence_order`), so a shot recorded before a reopen can carry an id that a
/// renumbered live shot now holds. Copied shots keep their live ids; a literal
/// whose id is taken (or a second copy of one) takes a fresh one from its
/// section's allocator. Each allocator is then raised past every id in its
/// window that any section holds, so the next shot it hands out cannot collide
/// either. A state recorded in this session is already consistent, and then
/// nothing here changes a byte.
fn settle_ids(sections: &mut [Value], literal: &[LiteralShot]) -> Result<(), DeltaError> {
    let literal: HashSet<LiteralShot> = literal.iter().copied().collect();
    let mut taken: HashSet<u64> = HashSet::new();
    let mut clash: Vec<LiteralShot> = Vec::new();
    // Copies first, so a literal is what yields when both carry an id.
    for pass_literal in [false, true] {
        for (s, section) in sections.iter().enumerate() {
            let Some(shots) = shots_array(section) else {
                continue;
            };
            for (i, shot) in shots.iter().enumerate() {
                if literal.contains(&(s, i)) != pass_literal {
                    continue;
                }
                if let Some(id) = shot_id_of(shot) {
                    if !taken.insert(id) {
                        clash.push((s, i));
                    }
                }
            }
        }
    }
    let stride = u64::from(FEATURE_ID_STRIDE);
    let mut window_max: HashMap<u64, u64> = HashMap::new();
    for id in &taken {
        let slot = window_max.entry(id / stride).or_insert(*id);
        *slot = (*slot).max(*id);
    }
    for section in sections.iter_mut() {
        let Some(id) = section_id_of(section) else {
            continue;
        };
        let Some(max) = window_max.get(&id).copied() else {
            continue;
        };
        let Value::Object(members) = section else {
            continue;
        };
        if let Some(next) = members.get(NEXT_SHOT_ID).and_then(Value::as_u64) {
            if next <= max {
                members.insert(NEXT_SHOT_ID.to_string(), Value::from(max + 1));
            }
        }
    }
    for (s, i) in clash {
        let Some(Value::Object(section)) = sections.get_mut(s) else {
            continue;
        };
        let Some(section_id) = section.get(ID).and_then(Value::as_u64) else {
            return Err("a section without an id holds a shot whose id is taken".to_string());
        };
        let end = (section_id + 1) * stride;
        let mut next = section
            .get(NEXT_SHOT_ID)
            .and_then(Value::as_u64)
            .unwrap_or(section_id * stride);
        while taken.contains(&next) {
            next += 1;
        }
        if next >= end {
            return Err(format!("section {section_id} has no shot id left to give back"));
        }
        taken.insert(next);
        section.insert(NEXT_SHOT_ID.to_string(), Value::from(next + 1));
        if let Some(Value::Object(shot)) = section
            .get_mut(SHOTS)
            .and_then(Value::as_array_mut)
            .and_then(|shots| shots.get_mut(i))
        {
            shot.insert(ID.to_string(), Value::from(next));
        }
    }
    Ok(())
}

/// Raise the project's section allocator past every section it holds. The
/// loader recomputes it as `max + 1`, so after a reopen the recorded value of
/// a state that held a since-deleted section can be at or below that
/// section's id, and the next new section would take it again.
fn settle_section_counter(project: &mut Map<String, Value>, sections: &[Value]) {
    let Some(max) = sections.iter().filter_map(section_id_of).max() else {
        return;
    };
    if let Some(next) = project.get(NEXT_SECTION_ID).and_then(Value::as_u64) {
        if next <= max {
            project.insert(NEXT_SECTION_ID.to_string(), Value::from(max + 1));
        }
    }
}

#[cfg(test)]
#[path = "delta_tests.rs"]
mod tests;
