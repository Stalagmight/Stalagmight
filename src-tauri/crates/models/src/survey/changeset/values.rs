//! Shot fields, the values a changeset records for them, and the column form
//! both literal shot rows and field-level edits are written in.
//!
//! A column is the values one field takes over a stretch of shots, as spans:
//! `[at, [v0, v1, ...]]` (distinct values at consecutive offsets) or
//! `[at, n, v]` (`n` shots all taking `v`). A paste of one column is then a
//! value per cell, and a credit set on a 2,000-shot line is one span.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{ShotKind, StationId, SurveyCredit};

use super::super::Shot;
use super::bits::Same;

/// One recordable field of a shot (everything but its uuid, which a run
/// matches by, and its session id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShotField {
    To,
    Length,
    AzimuthIn,
    AzimuthOut,
    DepthIn,
    DepthOut,
    PitchIn,
    PitchOut,
    Left,
    Right,
    Up,
    Down,
    Comment,
    Kind,
    Marker,
    Temperature,
    Credit,
}

impl ShotField {
    /// Every field, in the fixed order columns are written in.
    pub const ALL: [ShotField; 17] = [
        Self::To,
        Self::Length,
        Self::AzimuthIn,
        Self::AzimuthOut,
        Self::DepthIn,
        Self::DepthOut,
        Self::PitchIn,
        Self::PitchOut,
        Self::Left,
        Self::Right,
        Self::Up,
        Self::Down,
        Self::Comment,
        Self::Kind,
        Self::Marker,
        Self::Temperature,
        Self::Credit,
    ];

    /// The persisted key.
    pub fn code(self) -> &'static str {
        match self {
            Self::To => "to",
            Self::Length => "len",
            Self::AzimuthIn => "ai",
            Self::AzimuthOut => "ao",
            Self::DepthIn => "di",
            Self::DepthOut => "do",
            Self::PitchIn => "pi",
            Self::PitchOut => "po",
            Self::Left => "l",
            Self::Right => "r",
            Self::Up => "u",
            Self::Down => "d",
            Self::Comment => "c",
            Self::Kind => "k",
            Self::Marker => "m",
            Self::Temperature => "tp",
            Self::Credit => "cr",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.code() == code)
    }

    /// The name the history preview shows for the field.
    pub fn label(self) -> &'static str {
        match self {
            Self::To => "To station",
            Self::Length => "Length",
            Self::AzimuthIn => "Azimuth In",
            Self::AzimuthOut => "Azimuth Out",
            Self::DepthIn => "Depth In",
            Self::DepthOut => "Depth Out",
            Self::PitchIn => "Pitch In",
            Self::PitchOut => "Pitch Out",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Comment => "Comment",
            Self::Kind => "Kind",
            Self::Marker => "Marker",
            Self::Temperature => "Temperature",
            Self::Credit => "Credit",
        }
    }

    /// Whether `value` is a value this field can take (its JSON type).
    pub(crate) fn accepts(self, value: &Val) -> bool {
        match self {
            Self::To => matches!(value, Val::Int(v) if u32::try_from(*v).is_ok()),
            Self::Length | Self::AzimuthIn | Self::AzimuthOut | Self::DepthIn | Self::DepthOut => {
                matches!(value, Val::Num(_))
            }
            Self::PitchIn
            | Self::PitchOut
            | Self::Left
            | Self::Right
            | Self::Up
            | Self::Down
            | Self::Temperature => matches!(value, Val::Num(_) | Val::Null),
            Self::Comment => matches!(value, Val::Str(_) | Val::Null),
            Self::Kind => matches!(value, Val::Str(s) if kind_from_str(s).is_some()),
            Self::Marker => {
                matches!(value, Val::Null)
                    || matches!(value, Val::Int(v) if u32::try_from(*v).is_ok())
            }
            Self::Credit => matches!(value, Val::Int(_) | Val::Null),
        }
    }

    /// Whether `a` and `b` hold the same value of this field, to the bit.
    pub(crate) fn same(self, a: &Shot, b: &Shot) -> bool {
        match self {
            Self::To => a.to == b.to,
            Self::Length => a.length.same(&b.length),
            Self::AzimuthIn => a.azimuth_in.same(&b.azimuth_in),
            Self::AzimuthOut => a.azimuth_out.same(&b.azimuth_out),
            Self::DepthIn => a.depth_in.same(&b.depth_in),
            Self::DepthOut => a.depth_out.same(&b.depth_out),
            Self::PitchIn => a.pitch_in.same(&b.pitch_in),
            Self::PitchOut => a.pitch_out.same(&b.pitch_out),
            Self::Left => a.left.same(&b.left),
            Self::Right => a.right.same(&b.right),
            Self::Up => a.up.same(&b.up),
            Self::Down => a.down.same(&b.down),
            Self::Comment => a.comment == b.comment,
            Self::Kind => a.kind == b.kind,
            Self::Marker => a.marker_index == b.marker_index,
            Self::Temperature => a.temperature.same(&b.temperature),
            Self::Credit => a.credit.same(&b.credit),
        }
    }

    /// The value of this field on `shot` (a credit is interned into
    /// `credits`).
    pub(crate) fn get(self, shot: &Shot, credits: &mut Credits) -> Val {
        let num = |v: f64| Val::Num(v);
        let opt = |v: Option<f64>| v.map_or(Val::Null, Val::Num);
        match self {
            Self::To => Val::Int(u64::from(shot.to)),
            Self::Length => num(shot.length),
            Self::AzimuthIn => num(shot.azimuth_in),
            Self::AzimuthOut => num(shot.azimuth_out),
            Self::DepthIn => num(shot.depth_in),
            Self::DepthOut => num(shot.depth_out),
            Self::PitchIn => opt(shot.pitch_in),
            Self::PitchOut => opt(shot.pitch_out),
            Self::Left => opt(shot.left),
            Self::Right => opt(shot.right),
            Self::Up => opt(shot.up),
            Self::Down => opt(shot.down),
            Self::Comment => shot.comment.clone().map_or(Val::Null, Val::Str),
            Self::Kind => Val::Str(kind_str(shot.kind).to_string()),
            Self::Marker => shot
                .marker_index
                .map_or(Val::Null, |m| Val::Int(u64::from(m))),
            Self::Temperature => opt(shot.temperature),
            Self::Credit => shot
                .credit
                .as_ref()
                .map_or(Val::Null, |c| Val::Int(u64::from(credits.intern(c)))),
        }
    }

    /// Write `value` into this field of `shot`. `false` (and nothing
    /// written) when the value is not one the field takes or names a credit
    /// the table does not hold.
    pub(crate) fn set(self, shot: &mut Shot, value: &Val, credits: &[Arc<SurveyCredit>]) -> bool {
        if !self.accepts(value) {
            return false;
        }
        let num = |v: &Val| match v {
            Val::Num(n) => Some(*n),
            _ => None,
        };
        let int = |v: &Val| match v {
            Val::Int(n) => u32::try_from(*n).ok(),
            _ => None,
        };
        match self {
            Self::To => match int(value) {
                Some(to) => shot.to = to,
                None => return false,
            },
            Self::Length => shot.length = num(value).unwrap_or_default(),
            Self::AzimuthIn => shot.azimuth_in = num(value).unwrap_or_default(),
            Self::AzimuthOut => shot.azimuth_out = num(value).unwrap_or_default(),
            Self::DepthIn => shot.depth_in = num(value).unwrap_or_default(),
            Self::DepthOut => shot.depth_out = num(value).unwrap_or_default(),
            Self::PitchIn => shot.pitch_in = num(value),
            Self::PitchOut => shot.pitch_out = num(value),
            Self::Left => shot.left = num(value),
            Self::Right => shot.right = num(value),
            Self::Up => shot.up = num(value),
            Self::Down => shot.down = num(value),
            Self::Temperature => shot.temperature = num(value),
            Self::Comment => {
                shot.comment = match value {
                    Val::Str(s) => Some(s.clone()),
                    _ => None,
                };
            }
            Self::Kind => {
                if let Val::Str(s) = value {
                    shot.kind = kind_from_str(s).unwrap_or_default();
                }
            }
            Self::Marker => shot.marker_index = int(value),
            Self::Credit => {
                shot.credit = match value {
                    Val::Int(i) => match usize::try_from(*i).ok().and_then(|i| credits.get(i)) {
                        Some(credit) => Some(Arc::clone(credit)),
                        None => return false,
                    },
                    _ => None,
                };
            }
        }
        true
    }
}

/// A shot with every field at its default: what a literal row starts from.
pub(crate) fn blank_shot(uuid: String) -> Shot {
    Shot {
        id: 0,
        uuid,
        to: 0,
        azimuth_in: 0.0,
        azimuth_out: 0.0,
        length: 0.0,
        depth_in: 0.0,
        depth_out: 0.0,
        pitch_in: None,
        pitch_out: None,
        left: None,
        right: None,
        up: None,
        down: None,
        comment: None,
        kind: ShotKind::Normal,
        marker_index: None,
        temperature: None,
        credit: None,
    }
}

fn kind_str(kind: ShotKind) -> &'static str {
    match kind {
        ShotKind::Normal => "Normal",
        ShotKind::Jump => "Jump",
        ShotKind::Reference => "Reference",
    }
}

fn kind_from_str(s: &str) -> Option<ShotKind> {
    match s {
        "Normal" => Some(ShotKind::Normal),
        "Jump" => Some(ShotKind::Jump),
        "Reference" => Some(ShotKind::Reference),
        _ => None,
    }
}

/// A recorded value: what one field holds on one shot.
#[derive(Debug, Clone)]
pub enum Val {
    Null,
    Int(u64),
    Num(f64),
    Str(String),
}

impl Same for Val {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Num(a), Self::Num(b)) => a.same(b),
            (Self::Str(a), Self::Str(b)) => a == b,
            _ => false,
        }
    }
}

impl Val {
    /// The value as the history preview prints it.
    pub fn display(&self) -> String {
        match self {
            Self::Null => String::new(),
            Self::Int(v) => v.to_string(),
            Self::Num(v) => format!("{v}"),
            Self::Str(s) => s.clone(),
        }
    }

    /// The number, when the value is one.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Num(v) => Some(*v),
            _ => None,
        }
    }

    /// The text, when the value is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn finite(&self) -> bool {
        !matches!(self, Self::Num(v) if !v.is_finite())
    }
}

impl Serialize for Val {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => s.serialize_unit(),
            Self::Int(v) => s.serialize_u64(*v),
            Self::Num(v) => finite_f64(*v, s),
            Self::Str(v) => s.serialize_str(v),
        }
    }
}

/// A float written only when finite: `serde_json` would write NaN as `null`.
pub(crate) fn finite_f64<S: Serializer>(v: f64, s: S) -> Result<S::Ok, S::Error> {
    if v.is_finite() {
        s.serialize_f64(v)
    } else {
        Err(serde::ser::Error::custom(
            "a non-finite number in a history entry",
        ))
    }
}

impl<'de> Deserialize<'de> for Val {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Val;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("null, a number or a string")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Val, E> {
                Ok(Val::Null)
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Val, E> {
                Ok(Val::Int(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Val, E> {
                u64::try_from(v)
                    .map(Val::Int)
                    .map_err(|_| E::custom("a negative integer"))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Val, E> {
                Ok(Val::Num(v))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Val, E> {
                Ok(Val::Str(v.to_string()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Val, E> {
                Ok(Val::Str(v))
            }
        }
        d.deserialize_any(V)
    }
}

/// The credits a changeset names, interned once each (plan 8.1): a row or an
/// edit names a credit by its index in this table.
#[derive(Debug, Clone, Default)]
pub(crate) struct Credits {
    pub(crate) table: Vec<Arc<SurveyCredit>>,
    index: HashMap<Arc<SurveyCredit>, u32>,
}

impl Credits {
    pub(crate) fn intern(&mut self, credit: &Arc<SurveyCredit>) -> u32 {
        if let Some(&i) = self.index.get(credit) {
            return i;
        }
        let i = self.table.len() as u32;
        self.table.push(Arc::clone(credit));
        self.index.insert(Arc::clone(credit), i);
        i
    }
}

/// A stretch of one column.
#[derive(Debug, Clone)]
pub(crate) enum Span {
    /// `values[k]` at offset `at + k`.
    Values(u32, Vec<Val>),
    /// `n` offsets from `at`, all taking one value.
    Repeat(u32, u32, Val),
}

impl Span {
    pub(crate) fn start(&self) -> u32 {
        match self {
            Self::Values(at, _) | Self::Repeat(at, _, _) => *at,
        }
    }

    /// How many offsets the span covers.
    pub(crate) fn len(&self) -> u32 {
        match self {
            Self::Values(_, values) => values.len() as u32,
            Self::Repeat(_, n, _) => *n,
        }
    }

    /// Every `(offset, value)` of the span.
    pub(crate) fn cells(&self) -> impl Iterator<Item = (u32, &Val)> + '_ {
        let (at, n) = (self.start(), self.len());
        (0..n).map(move |k| {
            let value = match self {
                Self::Values(_, values) => &values[k as usize],
                Self::Repeat(_, _, value) => value,
            };
            (at + k, value)
        })
    }

    /// The value at `offset`, when the span covers it.
    pub(crate) fn at(&self, offset: u32) -> Option<&Val> {
        let start = self.start();
        if offset < start || offset - start >= self.len() {
            return None;
        }
        match self {
            Self::Values(_, values) => values.get((offset - start) as usize),
            Self::Repeat(_, _, value) => Some(value),
        }
    }
}

impl Serialize for Span {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Values(at, values) => {
                let mut seq = s.serialize_seq(Some(2))?;
                seq.serialize_element(at)?;
                seq.serialize_element(values)?;
                seq.end()
            }
            Self::Repeat(at, n, value) => {
                let mut seq = s.serialize_seq(Some(3))?;
                seq.serialize_element(at)?;
                seq.serialize_element(n)?;
                seq.serialize_element(value)?;
                seq.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Span {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Second {
            Values(Vec<Val>),
            Count(u32),
        }
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Span;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("[at, [values]] or [at, n, value]")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Span, A::Error> {
                let at: u32 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("a span without its offset"))?;
                let second: Second = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("a span without its values"))?;
                let span = match second {
                    Second::Values(values) => Span::Values(at, values),
                    Second::Count(n) => {
                        let value: Val = seq
                            .next_element()?
                            .ok_or_else(|| de::Error::custom("a repeat span without its value"))?;
                        Span::Repeat(at, n, value)
                    }
                };
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("a span with extra elements"));
                }
                Ok(span)
            }
        }
        d.deserialize_seq(V)
    }
}

/// The columns of a stretch of shots: per field (in [`ShotField::ALL`]
/// order), its spans in offset order.
#[derive(Debug, Clone, Default)]
pub(crate) struct Cols(pub(crate) Vec<(ShotField, Vec<Span>)>);

impl Cols {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every cell, as `(field, offset, value)`.
    pub(crate) fn cells(&self) -> impl Iterator<Item = (ShotField, u32, &Val)> + '_ {
        self.0.iter().flat_map(|(field, spans)| {
            spans
                .iter()
                .flat_map(move |span| span.cells().map(move |(at, v)| (*field, at, v)))
        })
    }

    /// The value of `field` at `offset`, when the columns hold one.
    pub(crate) fn value(&self, field: ShotField, offset: u32) -> Option<&Val> {
        let spans = &self.0.iter().find(|(f, _)| *f == field)?.1;
        let i = spans.partition_point(|span| span.start() <= offset);
        spans.get(i.checked_sub(1)?)?.at(offset)
    }

    /// Write every cell into `shots` (offset 0 is `shots[0]`). `false` when
    /// a cell is out of range or not a value its field takes.
    pub(crate) fn apply(&self, shots: &mut [Shot], credits: &[Arc<SurveyCredit>]) -> bool {
        for (field, offset, value) in self.cells() {
            let Some(shot) = shots.get_mut(offset as usize) else {
                return false;
            };
            if !field.set(shot, value, credits) {
                return false;
            }
        }
        true
    }
}

/// Builds [`Cols`] from cells given field by field, offsets ascending.
#[derive(Default)]
pub(crate) struct ColsBuilder {
    cols: Vec<(ShotField, Vec<(u32, Val)>)>,
}

/// A run of at least this many equal values at consecutive offsets is
/// written once (`[at, n, v]`).
const REPEAT_MIN: usize = 3;

impl ColsBuilder {
    /// Add `value` for `field` at `offset` (offsets per field ascending).
    pub(crate) fn push(&mut self, field: ShotField, offset: u32, value: Val) {
        match self.cols.iter_mut().find(|(f, _)| *f == field) {
            Some((_, cells)) => cells.push((offset, value)),
            None => self.cols.push((field, vec![(offset, value)])),
        }
    }

    pub(crate) fn finish(mut self) -> Cols {
        self.cols.sort_by_key(|(field, _)| *field);
        Cols(
            self.cols
                .into_iter()
                .map(|(field, cells)| (field, spans_of(cells)))
                .collect(),
        )
    }
}

/// `cells` (offsets ascending) as spans: consecutive offsets join one span,
/// and a stretch of [`REPEAT_MIN`] or more equal values becomes a repeat.
fn spans_of(cells: Vec<(u32, Val)>) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        let (at, ref value) = cells[i];
        // The stretch of equal values at consecutive offsets from `i`.
        let mut run = 1;
        while i + run < cells.len()
            && cells[i + run].0 == at + run as u32
            && cells[i + run].1.same(value)
        {
            run += 1;
        }
        if run >= REPEAT_MIN {
            spans.push(Span::Repeat(at, run as u32, value.clone()));
            i += run;
            continue;
        }
        match spans.last_mut() {
            Some(Span::Values(start, values)) if *start + values.len() as u32 == at => {
                values.push(value.clone());
            }
            _ => spans.push(Span::Values(at, vec![value.clone()])),
        }
        i += 1;
    }
    spans
}

impl Serialize for Cols {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (field, spans) in &self.0 {
            map.serialize_entry(field.code(), spans)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Cols {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Cols;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object of shot-field columns")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Cols, A::Error> {
                let mut cols: Vec<(ShotField, Vec<Span>)> = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    let field = ShotField::from_code(&key)
                        .ok_or_else(|| de::Error::custom(format!("unknown shot field {key:?}")))?;
                    if cols.iter().any(|(f, _)| *f == field) {
                        return Err(de::Error::custom(format!("shot field {key:?} twice")));
                    }
                    cols.push((field, map.next_value()?));
                }
                cols.sort_by_key(|(field, _)| *field);
                Ok(Cols(cols))
            }
        }
        d.deserialize_map(V)
    }
}

/// Station ids as ascending runs `[[start, len], ...]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct IdRuns(pub(crate) Vec<(StationId, u32)>);

impl IdRuns {
    pub(crate) fn of(ids: &[StationId]) -> Self {
        let mut runs: Vec<(StationId, u32)> = Vec::new();
        for &id in ids {
            match runs.last_mut() {
                Some((start, len)) if u64::from(*start) + u64::from(*len) == u64::from(id) => {
                    *len += 1;
                }
                _ => runs.push((id, 1)),
            }
        }
        Self(runs)
    }

    /// How many ids the runs name (saturating: a hostile count is refused by
    /// the shape check before anything expands it).
    pub(crate) fn count(&self) -> u64 {
        self.0
            .iter()
            .map(|(_, len)| u64::from(*len))
            .fold(0, u64::saturating_add)
    }

    pub(crate) fn ids(&self) -> impl Iterator<Item = StationId> + '_ {
        self.0
            .iter()
            .flat_map(|&(start, len)| (0..len).map(move |k| start.saturating_add(k)))
    }
}
