//! The station-keyed members, with their keys written as positions.
//!
//! Four top-level members hold `[station key, value]` pairs, the key a
//! [`StationKey`](crate::models::StationKey) - `{"line_id": L, "anchor":
//! "LineStart" | {"AfterShot": uuid}}`: `station_offsets`, `station_comments`,
//! `station_names` and `station_ids`. Since every station has a name and a
//! Station ID, the last two hold an entry for EVERY station, so an edit that
//! re-keys a line's stations (a reverse, a delete, a split) or brings a line
//! back (the undo of a delete) moves one pair per station per member, each
//! spelling out a 36-character uuid. Written as they are, those pairs were
//! most of such an entry.
//!
//! A pair whose key names a station of the state it belongs to - the SOURCE
//! for an element the bag removes, the TARGET for one it adds - is written
//! instead as `(line id, station index, value)`, and its key is rebuilt from
//! that state's shots when the delta is applied: `LineStart` for station 0,
//! `AfterShot(uuid of shot k - 1)` for station k. The encoder only does so
//! when the rebuilt key is the recorded one, value for value, so the bag
//! restores exactly what the literal pair did; any other pair stays literal.
//!
//! A position names no uuid, so the load's uuid remap has nothing to rewrite
//! in one: the key is rebuilt from the uuids the state holds when the step
//! runs, which the remap has already brought up to date.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Map, Value};

use super::json::same;
use super::{DeltaError, Edit, ID, SHOTS, UUID};
use crate::models::LineId;

/// The top-level members whose elements are `[station key, value]` pairs.
pub(super) const STATION_KEYED: [&str; 4] = [
    "station_offsets",
    "station_comments",
    "station_names",
    "station_ids",
];

const LINE_ID: &str = "line_id";
const ANCHOR: &str = "anchor";
const LINE_START: &str = "LineStart";
const AFTER_SHOT: &str = "AfterShot";

/// One line of a state, as the delta engine holds it: raw text, or a value it
/// built.
#[derive(Clone, Copy)]
pub(super) enum LineText<'s> {
    Raw(&'s str),
    Built(&'s Value),
}

/// The shot uuids of one line, by position.
type Uuids = Vec<Option<String>>;

/// The lines of one state, by id, with each line's shot uuids read only when
/// a station key asks for them.
pub(super) struct Stations<'s> {
    lines: HashMap<LineId, LineText<'s>>,
    uuids: HashMap<LineId, Option<Uuids>>,
}

impl<'s> Stations<'s> {
    /// The state whose lines are `lines`, each with its id (a line without
    /// one is never named).
    pub(super) fn new(lines: impl IntoIterator<Item = (Option<LineId>, LineText<'s>)>) -> Self {
        Self {
            lines: lines
                .into_iter()
                .filter_map(|(id, line)| Some((id?, line)))
                .collect(),
            uuids: HashMap::new(),
        }
    }

    fn uuids(&mut self, line: LineId) -> Option<&Uuids> {
        if !self.uuids.contains_key(&line) {
            let read = self.lines.get(&line).and_then(|text| read_uuids(*text));
            self.uuids.insert(line, read);
        }
        self.uuids.get(&line)?.as_ref()
    }

    /// The key of station `index` of line `line` in this state, when the line
    /// holds that station and the shot before it has a uuid.
    pub(super) fn key_at(&mut self, line: LineId, index: u32) -> Option<Value> {
        let anchor = if index == 0 {
            self.lines.get(&line)?;
            Value::String(LINE_START.to_string())
        } else {
            let shot = usize::try_from(index - 1).ok()?;
            let uuid = self.uuids(line)?.get(shot)?.clone()?;
            let mut after = Map::new();
            after.insert(AFTER_SHOT.to_string(), Value::String(uuid));
            Value::Object(after)
        };
        let mut key = Map::new();
        key.insert(LINE_ID.to_string(), Value::from(line));
        key.insert(ANCHOR.to_string(), anchor);
        Some(Value::Object(key))
    }

    /// The station `key` names in this state, by position, when it names one
    /// and [`Self::key_at`] rebuilds exactly `key` from it.
    pub(super) fn position_of(&mut self, key: &Value) -> Option<(LineId, u32)> {
        let object = key.as_object()?;
        if object.len() != 2 {
            return None;
        }
        let line = LineId::try_from(object.get(LINE_ID)?.as_u64()?).ok()?;
        let index = match object.get(ANCHOR)? {
            Value::String(anchor) if anchor == LINE_START => 0,
            Value::Object(anchor) if anchor.len() == 1 => {
                let uuid = anchor.get(AFTER_SHOT)?.as_str()?;
                let shot = self
                    .uuids(line)?
                    .iter()
                    .position(|held| held.as_deref() == Some(uuid))?;
                u32::try_from(shot).ok()?.checked_add(1)?
            }
            _ => return None,
        };
        let rebuilt = self.key_at(line, index)?;
        same(&rebuilt, key).then_some((line, index))
    }
}

/// The shot uuids of a line, or `None` when it cannot be read.
fn read_uuids(line: LineText<'_>) -> Option<Uuids> {
    match line {
        LineText::Built(value) => Some(
            value
                .get(SHOTS)?
                .as_array()?
                .iter()
                .map(|shot| shot.get(UUID).and_then(Value::as_str).map(str::to_string))
                .collect(),
        ),
        LineText::Raw(raw) => {
            #[derive(Deserialize)]
            struct Shot {
                #[serde(default)]
                uuid: Option<String>,
            }
            #[derive(Deserialize)]
            struct Line {
                #[serde(default)]
                shots: Vec<Shot>,
            }
            let line: Line = serde_json::from_str(raw).ok()?;
            Some(line.shots.into_iter().map(|shot| shot.uuid).collect())
        }
    }
}

/// The id of a line from its raw text, without parsing the whole line when it
/// starts the way this build writes one (`{"id":N,`).
pub(super) fn raw_line_id(raw: &str) -> Option<LineId> {
    if let Some(rest) = raw.strip_prefix("{\"id\":") {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && rest[digits..].starts_with([',', '}']) {
            return rest[..digits].parse().ok();
        }
    }
    #[derive(Deserialize)]
    struct Head {
        #[serde(default)]
        id: Option<u64>,
    }
    let head: Head = serde_json::from_str(raw).ok()?;
    LineId::try_from(head.id?).ok()
}

/// The id of a built line.
pub(super) fn built_line_id(line: &Value) -> Option<LineId> {
    LineId::try_from(line.get(ID)?.as_u64()?).ok()
}

/// `pair` as `(key, value)` when it is a `[station key, value]` pair.
fn split_pair(pair: &Value) -> Option<(&Value, &Value)> {
    match pair.as_array()?.as_slice() {
        [key, value] => Some((key, value)),
        _ => None,
    }
}

fn pair(key: Value, value: Value) -> Value {
    Value::Array(vec![key, value])
}

/// A station-keyed member's bag edit with every pair whose key names a
/// station of its state written as a position (see the module docs).
pub(super) fn positional(edit: Edit, source: &mut Stations<'_>, target: &mut Stations<'_>) -> Edit {
    let Edit::Bag {
        del,
        add,
        station_del,
        station_add,
    } = edit
    else {
        return edit;
    };
    let mut literal_del = Vec::new();
    let mut station_del = station_del;
    for element in del {
        match split_pair(&element).and_then(|(key, value)| {
            let (line, index) = source.position_of(key)?;
            Some((line, index, value.clone()))
        }) {
            Some(positional) => station_del.push(positional),
            None => literal_del.push(element),
        }
    }
    let mut literal_add = Vec::new();
    let mut station_add = station_add;
    for (at, element) in add {
        match split_pair(&element).and_then(|(key, value)| {
            let (line, index) = target.position_of(key)?;
            Some((at, line, index, value.clone()))
        }) {
            Some(positional) => station_add.push(positional),
            None => literal_add.push((at, element)),
        }
    }
    Edit::Bag {
        del: literal_del,
        add: literal_add,
        station_del,
        station_add,
    }
}

/// `edit` with every positional pair written out as the literal pair it
/// stands for: a removal's key rebuilt from the SOURCE, an addition's from the
/// TARGET. A removal whose station the source does not hold is dropped, as a
/// literal removal of an element that is not there is skipped; an addition
/// whose station the target does not hold refuses the step.
pub(super) fn literal(
    edit: &Edit,
    source: &mut Stations<'_>,
    target: &mut Stations<'_>,
) -> Result<Edit, DeltaError> {
    let Edit::Bag {
        del,
        add,
        station_del,
        station_add,
    } = edit
    else {
        return Ok(edit.clone());
    };
    if station_del.is_empty() && station_add.is_empty() {
        return Ok(edit.clone());
    }
    let mut out_del = del.clone();
    for (line, index, value) in station_del {
        if let Some(key) = source.key_at(*line, *index) {
            out_del.push(pair(key, value.clone()));
        }
    }
    let mut out_add = add.clone();
    for (at, line, index, value) in station_add {
        let key = target.key_at(*line, *index).ok_or_else(|| {
            format!("a station-keyed edit adds a pair at station {index} of line {line}, which the target does not hold")
        })?;
        out_add.push((*at, pair(key, value.clone())));
    }
    // The additions go in by target index, ascending, as the bag recorded them.
    out_add.sort_by_key(|(at, _)| *at);
    Ok(Edit::Bag {
        del: out_del,
        add: out_add,
        station_del: Vec::new(),
        station_add: Vec::new(),
    })
}
