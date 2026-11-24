//! Stations in a changeset: whole images of the stations created or deleted,
//! and field-level columns for the stations on both sides, one column per
//! field: `[ids as runs, [before...], [after...]]`. Renaming a thousand
//! stations is then two names per station and a few id runs.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::StationId;

use super::super::{ArchivedStation, Station};
use super::bits::Same;
use super::values::IdRuns;
use super::{Direction, Refusal};

/// One field's changes over the stations on both sides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FieldCol<T>(pub(crate) IdRuns, pub(crate) Vec<T>, pub(crate) Vec<T>);

impl<T> FieldCol<T> {
    /// `(id, before, after)` per changed station, ascending id.
    pub(crate) fn rows(&self) -> impl Iterator<Item = (StationId, &T, &T)> + '_ {
        self.0
            .ids()
            .zip(self.1.iter().zip(&self.2))
            .map(|(id, (b, a))| (id, b, a))
    }

    pub(crate) fn shape_ok(&self) -> bool {
        let count = self.0.count();
        count == self.1.len() as u64 && count == self.2.len() as u64
    }
}

/// How the stations changed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct StationChanges {
    /// Stations of the AFTER state only, ascending id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) add: Vec<StationRow>,
    /// Stations of the BEFORE state only, ascending id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) del: Vec<StationRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<FieldCol<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) comment: Option<FieldCol<Option<String>>>,
    /// The archived records (RS 10), each a whole object row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) archived: Option<FieldCol<Vec<ArchivedStation>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) external: Option<FieldCol<Option<String>>>,
}

/// Builds one field column from stations visited in ascending id.
struct ColBuilder<T> {
    ids: Vec<StationId>,
    before: Vec<T>,
    after: Vec<T>,
}

impl<T: Same + Clone> ColBuilder<T> {
    fn new() -> Self {
        Self {
            ids: Vec::new(),
            before: Vec::new(),
            after: Vec::new(),
        }
    }

    fn visit(&mut self, id: StationId, before: &T, after: &T) {
        if !before.same(after) {
            self.ids.push(id);
            self.before.push(before.clone());
            self.after.push(after.clone());
        }
    }

    fn finish(self) -> Option<FieldCol<T>> {
        (!self.ids.is_empty()).then(|| FieldCol(IdRuns::of(&self.ids), self.before, self.after))
    }
}

impl StationChanges {
    pub(crate) fn between(
        before: &BTreeMap<StationId, Station>,
        after: &BTreeMap<StationId, Station>,
    ) -> Self {
        let mut out = Self::default();
        let mut name = ColBuilder::new();
        let mut comment = ColBuilder::new();
        let mut archived = ColBuilder::new();
        let mut external = ColBuilder::new();
        let (mut b, mut a) = (before.iter().peekable(), after.iter().peekable());
        loop {
            match (b.peek(), a.peek()) {
                (None, None) => break,
                (Some((bid, bs)), Some((aid, as_))) if bid == aid => {
                    if !bs.same(as_) {
                        let id = **bid;
                        name.visit(id, &bs.name, &as_.name);
                        comment.visit(id, &bs.comment, &as_.comment);
                        archived.visit(id, &bs.archived, &as_.archived);
                        external.visit(id, &bs.external, &as_.external);
                    }
                    b.next();
                    a.next();
                }
                (Some((bid, bs)), Some((aid, _))) if bid < aid => {
                    out.del.push(StationRow((*bs).clone()));
                    b.next();
                }
                (Some((_, bs)), None) => {
                    out.del.push(StationRow((*bs).clone()));
                    b.next();
                }
                (_, Some((_, as_))) => {
                    out.add.push(StationRow((*as_).clone()));
                    a.next();
                }
            }
        }
        out.name = name.finish();
        out.comment = comment.finish();
        out.archived = archived.finish();
        out.external = external.finish();
        out
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.add.is_empty()
            && self.del.is_empty()
            && self.name.is_none()
            && self.comment.is_none()
            && self.archived.is_none()
            && self.external.is_none()
    }

    /// The images a step in `direction` removes (the source side's own) and
    /// inserts (the target side's own).
    fn removed_inserted(&self, direction: Direction) -> (&[StationRow], &[StationRow]) {
        match direction {
            Direction::Undo => (&self.add, &self.del),
            Direction::Redo => (&self.del, &self.add),
        }
    }

    /// Whether `stations` is where a step in `direction` starts, for every
    /// station this change touches.
    pub(crate) fn check(
        &self,
        stations: &BTreeMap<StationId, Station>,
        direction: Direction,
    ) -> Result<(), Refusal> {
        let (removed, inserted) = self.removed_inserted(direction);
        for StationRow(image) in removed {
            if !stations.get(&image.id).is_some_and(|live| live.same(image)) {
                return Err(Refusal::Station(image.id));
            }
        }
        for StationRow(image) in inserted {
            if stations.contains_key(&image.id) {
                return Err(Refusal::Station(image.id));
            }
        }
        fn col<T: Same>(
            col: Option<&FieldCol<T>>,
            stations: &BTreeMap<StationId, Station>,
            direction: Direction,
            field: impl Fn(&Station) -> &T,
        ) -> Result<(), Refusal> {
            for (id, before, after) in col.into_iter().flat_map(FieldCol::rows) {
                let from = match direction {
                    Direction::Undo => after,
                    Direction::Redo => before,
                };
                if !stations.get(&id).is_some_and(|s| field(s).same(from)) {
                    return Err(Refusal::Station(id));
                }
            }
            Ok(())
        }
        col(self.name.as_ref(), stations, direction, |s| &s.name)?;
        col(self.comment.as_ref(), stations, direction, |s| &s.comment)?;
        col(self.archived.as_ref(), stations, direction, |s| &s.archived)?;
        col(self.external.as_ref(), stations, direction, |s| &s.external)?;
        Ok(())
    }

    /// Step `stations` in `direction` ([`Self::check`] passed).
    pub(crate) fn apply(&self, stations: &mut BTreeMap<StationId, Station>, direction: Direction) {
        let (removed, inserted) = self.removed_inserted(direction);
        for StationRow(image) in removed {
            stations.remove(&image.id);
        }
        for StationRow(image) in inserted {
            stations.insert(image.id, image.clone());
        }
        fn col<T: Clone>(
            col: Option<&FieldCol<T>>,
            stations: &mut BTreeMap<StationId, Station>,
            direction: Direction,
            field: impl Fn(&mut Station) -> &mut T,
        ) {
            for (id, before, after) in col.into_iter().flat_map(FieldCol::rows) {
                let to = match direction {
                    Direction::Undo => before,
                    Direction::Redo => after,
                };
                if let Some(station) = stations.get_mut(&id) {
                    *field(station) = to.clone();
                }
            }
        }
        col(self.name.as_ref(), stations, direction, |s| &mut s.name);
        col(self.comment.as_ref(), stations, direction, |s| {
            &mut s.comment
        });
        col(self.archived.as_ref(), stations, direction, |s| {
            &mut s.archived
        });
        col(self.external.as_ref(), stations, direction, |s| {
            &mut s.external
        });
    }
}

/// A station image, written as `[id, name]` or `[id, name, {comment,
/// external, archived}]` (each archived record an object `{id, name,
/// comment?, external?, lines?}`, every field carried, RS 10).
#[derive(Debug, Clone)]
pub(crate) struct StationRow(pub(crate) Station);

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct StationExtras {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    external: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    archived: Vec<ArchivedStation>,
}

impl Serialize for StationRow {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let station = &self.0;
        let extras =
            station.comment.is_some() || station.external.is_some() || !station.archived.is_empty();
        let mut seq = s.serialize_seq(Some(if extras { 3 } else { 2 }))?;
        seq.serialize_element(&station.id)?;
        seq.serialize_element(&station.name)?;
        if extras {
            seq.serialize_element(&ExtrasRef(station))?;
        }
        seq.end()
    }
}

struct ExtrasRef<'a>(&'a Station);

impl Serialize for ExtrasRef<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let station = self.0;
        let mut map = s.serialize_map(None)?;
        if let Some(comment) = &station.comment {
            map.serialize_entry("comment", comment)?;
        }
        if let Some(external) = &station.external {
            map.serialize_entry("external", external)?;
        }
        if !station.archived.is_empty() {
            map.serialize_entry("archived", &station.archived)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for StationRow {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = StationRow;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("[id, name] or [id, name, {extras}]")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<StationRow, A::Error> {
                let id: StationId = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("a station without its id"))?;
                let name: String = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("a station without its name"))?;
                let extras: StationExtras = seq.next_element()?.unwrap_or_default();
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("a station with extra elements"));
                }
                Ok(StationRow(Station {
                    id,
                    name,
                    comment: extras.comment,
                    external: extras.external,
                    archived: extras.archived,
                }))
            }
        }
        d.deserialize_seq(V)
    }
}
