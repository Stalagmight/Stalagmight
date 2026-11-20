//! Streaming readers for the bulk members (`stations`, `lines`) and the
//! nested objects they hold (credits, azimuth references), plus the decode
//! context every reader shares.
//!
//! Single pass, no intermediate tree: each row is read straight into the
//! model type. Counts are checked as records arrive, so a hostile file cannot
//! make the reader allocate past a [`CountLimits`] cap.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;

use serde::de::{self, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

use crate::models::survey::{Line, Shot, Station, StationAlias};
use crate::models::{
    AzimuthReference, CorrectionSource, LineId, NorthReference, Person, ShotKind, StationId,
    SurveyCredit, SurveyDirection,
};

use super::error::DecodeError;
use super::scan::Key;
use super::tokens;
use super::types::{CountLimits, Limit};
use super::write::parse_date;

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// Where a reader is, for error messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Loc {
    Member(&'static str),
    Element(&'static str, usize),
    LineHeader {
        element: usize,
    },
    Shot {
        element: usize,
        line: LineId,
        index: usize,
    },
}

impl fmt::Display for Loc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Member(member) => write!(f, "the {member} member"),
            Self::Element(member, i) => write!(f, "{member}[{i}]"),
            Self::LineHeader { element } => write!(f, "the line header at lines[{element}]"),
            Self::Shot {
                element,
                line,
                index,
            } => write!(f, "shot {index} of line {line} (lines[{element}])"),
        }
    }
}

/// State shared by every reader of one decode.
pub(super) struct Ctx<'l> {
    limits: &'l CountLimits,
    counts: [usize; Limit::ALL.len()],
    pub(super) loc: Loc,
    /// The field being read, for error messages ("" between fields).
    pub(super) field: &'static str,
    /// A typed failure a reader raised; preferred over serde's error.
    pub(super) fail: Option<DecodeError>,
    /// Paths of unknown structural content (tier 1), the first few.
    pub(super) unknown: Vec<String>,
    pub(super) unknown_total: usize,
    credits: HashMap<SurveyCredit, Arc<SurveyCredit>>,
}

impl<'l> Ctx<'l> {
    pub(super) fn new(limits: &'l CountLimits) -> Self {
        Self {
            limits,
            counts: [0; Limit::ALL.len()],
            loc: Loc::Member("file"),
            field: "",
            fail: None,
            unknown: Vec::new(),
            unknown_total: 0,
            credits: HashMap::new(),
        }
    }

    /// Where the reader is, with the current field.
    pub(super) fn path(&self) -> String {
        if self.field.is_empty() {
            self.loc.to_string()
        } else {
            format!("{}, field {}", self.loc, self.field)
        }
    }

    /// Stash a typed failure and hand back a serde error to unwind with.
    pub(super) fn fail<E: de::Error>(&mut self, error: DecodeError) -> E {
        if self.fail.is_none() {
            self.fail = Some(error);
        }
        E::custom("stalx: typed failure")
    }

    pub(super) fn bad<E: de::Error>(&mut self, message: impl Into<String>) -> E {
        let at = self.path();
        self.fail(DecodeError::BadValue {
            at,
            message: message.into(),
        })
    }

    pub(super) fn missing<E: de::Error>(&mut self, field: &str) -> E {
        let at = self.loc.to_string();
        self.fail(DecodeError::Missing {
            at,
            field: field.to_string(),
        })
    }

    pub(super) fn duplicate<E: de::Error>(&mut self, key: &str) -> E {
        let at = self.loc.to_string();
        self.fail(DecodeError::DuplicateKey {
            at,
            key: key.to_string(),
        })
    }

    /// Count `n` more of `limit`, failing past its cap.
    pub(super) fn count<E: de::Error>(&mut self, limit: Limit, n: usize) -> Result<(), E> {
        let slot = &mut self.counts[limit as usize];
        *slot = slot.saturating_add(n);
        let cap = limit.cap(self.limits);
        if *slot > cap {
            return Err(self.fail(DecodeError::TooComplex { limit, max: cap }));
        }
        Ok(())
    }

    /// [`Self::count`] outside a reader: the typed error directly.
    pub(super) fn count_typed(&mut self, limit: Limit, n: usize) -> Result<(), DecodeError> {
        let slot = &mut self.counts[limit as usize];
        *slot = slot.saturating_add(n);
        let cap = limit.cap(self.limits);
        if *slot > cap {
            return Err(DecodeError::TooComplex { limit, max: cap });
        }
        Ok(())
    }

    /// Note unknown structural content (tier 1: the project opens read-only).
    pub(super) fn unknown(&mut self, what: impl FnOnce(&Self) -> String) {
        self.unknown_total += 1;
        if self.unknown.len() < 8 {
            let path = what(self);
            self.unknown.push(path);
        }
    }

    fn unknown_key(&mut self, key: &str) {
        self.unknown(|ctx| format!("{} key {key:?}", ctx.loc));
    }

    fn unknown_value(&mut self, field: &str, value: &str) {
        self.unknown(|ctx| format!("{} {field} {value:?}", ctx.loc));
    }

    /// One shared allocation per distinct credit.
    fn intern(&mut self, credit: SurveyCredit) -> Arc<SurveyCredit> {
        if let Some(shared) = self.credits.get(&credit) {
            return Arc::clone(shared);
        }
        let shared = Arc::new(credit.clone());
        self.credits.insert(credit, Arc::clone(&shared));
        shared
    }
}

/// Read the next element of a fixed-arity row, failing when it is missing.
fn cell<'de, T: de::Deserialize<'de>, A: SeqAccess<'de>>(
    seq: &mut A,
    ctx: &mut Ctx<'_>,
    field: &'static str,
) -> Result<T, A::Error> {
    ctx.field = field;
    let value = seq.next_element::<T>()?;
    let value = value.ok_or_else(|| ctx.missing(field))?;
    ctx.field = "";
    Ok(value)
}

/// Fail when a fixed-arity row has more elements.
fn end_of_row<'de, A: SeqAccess<'de>>(seq: &mut A, ctx: &mut Ctx<'_>) -> Result<(), A::Error> {
    ctx.field = "";
    if seq.next_element::<IgnoredAny>()?.is_some() {
        return Err(ctx.bad("has more cells than the format defines"));
    }
    Ok(())
}

/// A map value, with the key as the current field.
fn value<'de, T: de::Deserialize<'de>, A: MapAccess<'de>>(
    map: &mut A,
    ctx: &mut Ctx<'_>,
    field: &'static str,
) -> Result<T, A::Error> {
    ctx.field = field;
    let value = map.next_value::<T>()?;
    ctx.field = "";
    Ok(value)
}

/// Mark key `index` of a known-key object seen; a second time is a duplicate.
fn seen<E: de::Error>(ctx: &mut Ctx<'_>, mask: &mut u32, index: usize, key: &str) -> Result<(), E> {
    let bit = 1u32 << index;
    if *mask & bit != 0 {
        return Err(ctx.duplicate(key));
    }
    *mask |= bit;
    Ok(())
}

/// The index of `key` in `keys`.
fn known(keys: &[&str], key: &str) -> Option<usize> {
    keys.iter().position(|k| *k == key)
}

/// Skip an unknown key's value, refusing a repeated unknown key.
fn skip_unknown<'de, A: MapAccess<'de>>(
    map: &mut A,
    ctx: &mut Ctx<'_>,
    others: &mut Vec<String>,
    key: &str,
) -> Result<(), A::Error> {
    if others.iter().any(|k| k == key) {
        return Err(ctx.duplicate(key));
    }
    // Unknown keys count against the preserved-content cap, which also
    // bounds the duplicate scan above.
    ctx.count(Limit::Preserved, 1)?;
    others.push(key.to_string());
    ctx.unknown_key(key);
    map.next_value::<IgnoredAny>()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Stations
// ---------------------------------------------------------------------------

/// The `stations` member: `[id, "name"]` or `[id, "name", {extras}]` rows.
pub(super) struct StationsSeed<'c, 'l> {
    pub(super) ctx: &'c mut Ctx<'l>,
    pub(super) out: &'c mut Vec<Station>,
}

impl<'de> DeserializeSeed<'de> for StationsSeed<'_, '_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for StationsSeed<'_, '_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an array of station rows")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let mut element = 0;
        loop {
            self.ctx.loc = Loc::Element("stations", element);
            let Some(station) = seq.next_element_seed(StationSeed {
                ctx: &mut *self.ctx,
            })?
            else {
                return Ok(());
            };
            self.ctx.count(Limit::Stations, 1)?;
            self.out.push(station);
            element += 1;
        }
    }
}

struct StationSeed<'c, 'l> {
    ctx: &'c mut Ctx<'l>,
}

impl<'de> DeserializeSeed<'de> for StationSeed<'_, '_> {
    type Value = Station;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Station, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for StationSeed<'_, '_> {
    type Value = Station;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a station row [id, name, {extras}?]")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Station, A::Error> {
        let ctx = self.ctx;
        let id: StationId = cell(&mut seq, ctx, "id")?;
        let name: String = cell(&mut seq, ctx, "name")?;
        let mut station = Station::new(id, name);
        if let Some(()) = seq.next_element_seed(StationExtrasSeed {
            ctx: &mut *ctx,
            station: &mut station,
        })? {
            end_of_row(&mut seq, ctx)?;
        }
        Ok(station)
    }
}

struct StationExtrasSeed<'c, 'l> {
    ctx: &'c mut Ctx<'l>,
    station: &'c mut Station,
}

impl<'de> DeserializeSeed<'de> for StationExtrasSeed<'_, '_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for StationExtrasSeed<'_, '_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a station's extras object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        const KEYS: [&str; 3] = ["comment", "aliases", "external"];
        let ctx = self.ctx;
        let mut mask = 0;
        let mut others = Vec::new();
        while let Some(key) = map.next_key::<Key<'de>>()? {
            let key = key.0;
            let Some(index) = known(&KEYS, &key) else {
                skip_unknown(&mut map, ctx, &mut others, &key)?;
                continue;
            };
            seen(ctx, &mut mask, index, &key)?;
            match index {
                0 => self.station.comment = Some(value(&mut map, ctx, "comment")?),
                1 => {
                    ctx.field = "aliases";
                    map.next_value_seed(AliasesSeed {
                        ctx: &mut *ctx,
                        out: &mut self.station.aliases,
                    })?;
                    ctx.field = "";
                }
                _ => self.station.external = Some(value(&mut map, ctx, "external")?),
            }
        }
        Ok(())
    }
}

struct AliasesSeed<'c, 'l> {
    ctx: &'c mut Ctx<'l>,
    out: &'c mut Vec<StationAlias>,
}

impl<'de> DeserializeSeed<'de> for AliasesSeed<'_, '_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for AliasesSeed<'_, '_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an array of aliases [id, name?]")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(alias) = seq.next_element_seed(AliasSeed {
            ctx: &mut *self.ctx,
        })? {
            self.ctx.count(Limit::Aliases, 1)?;
            self.out.push(alias);
        }
        Ok(())
    }
}

struct AliasSeed<'c, 'l> {
    ctx: &'c mut Ctx<'l>,
}

impl<'de> DeserializeSeed<'de> for AliasSeed<'_, '_> {
    type Value = StationAlias;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<StationAlias, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for AliasSeed<'_, '_> {
    type Value = StationAlias;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an alias [id, name?]")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<StationAlias, A::Error> {
        let id: StationId = cell(&mut seq, self.ctx, "alias id")?;
        self.ctx.field = "alias name";
        let name = seq.next_element::<String>()?;
        if name.is_some() {
            end_of_row(&mut seq, self.ctx)?;
        }
        self.ctx.field = "";
        Ok(StationAlias { id, name })
    }
}

// ---------------------------------------------------------------------------
// Lines: headers and shot rows, interleaved
// ---------------------------------------------------------------------------

/// A line being read, with the credit its header declares for its shots.
pub(super) struct LineBuild {
    pub(super) line: Line,
    credit: Option<Arc<SurveyCredit>>,
}

/// The `lines` member: a line header object, then that line's shot rows,
/// then the next header.
pub(super) struct LinesSeed<'c, 'l> {
    pub(super) ctx: &'c mut Ctx<'l>,
    pub(super) out: &'c mut Vec<LineBuild>,
}

impl<'de> DeserializeSeed<'de> for LinesSeed<'_, '_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for LinesSeed<'_, '_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an array of line headers and shot rows")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let mut element = 0;
        while seq
            .next_element_seed(ElementSeed {
                ctx: &mut *self.ctx,
                out: &mut *self.out,
                element,
            })?
            .is_some()
        {
            element += 1;
        }
        Ok(())
    }
}

struct ElementSeed<'c, 'l> {
    ctx: &'c mut Ctx<'l>,
    out: &'c mut Vec<LineBuild>,
    element: usize,
}

impl<'de> DeserializeSeed<'de> for ElementSeed<'_, '_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for ElementSeed<'_, '_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a line header object or a shot row array")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<(), A::Error> {
        self.ctx.loc = Loc::LineHeader {
            element: self.element,
        };
        let build = read_line_header(map, self.ctx)?;
        self.ctx.count(Limit::Lines, 1)?;
        self.out.push(build);
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<(), A::Error> {
        let Some(build) = self.out.last_mut() else {
            self.ctx.loc = Loc::Element("lines", self.element);
            return Err(self.ctx.bad("a shot row comes before any line header"));
        };
        self.ctx.loc = Loc::Shot {
            element: self.element,
            line: build.line.id,
            index: build.line.shots.len(),
        };
        let shot = read_shot(seq, self.ctx, build.credit.as_ref())?;
        self.ctx.count(Limit::Shots, 1)?;
        build.line.shots.push(shot);
        Ok(())
    }
}

fn read_line_header<'de, A: MapAccess<'de>>(
    mut map: A,
    ctx: &mut Ctx<'_>,
) -> Result<LineBuild, A::Error> {
    const KEYS: [&str; 12] = [
        "id",
        "name",
        "color",
        "direction",
        "quality",
        "section",
        "azimuth",
        "surveyor",
        "date",
        "credit",
        "start",
        "hand",
    ];
    let mut mask = 0;
    let mut others = Vec::new();
    let mut id: Option<LineId> = None;
    let mut name = None;
    let mut color: Option<String> = None;
    let mut direction = SurveyDirection::In;
    let mut quality = 1.0;
    let mut section = None;
    let mut azimuth = None;
    let mut surveyor = None;
    let mut date = None;
    let mut credit = None;
    let mut start: Option<StationId> = None;
    let mut hand = BTreeMap::new();
    while let Some(key) = map.next_key::<Key<'de>>()? {
        let key = key.0;
        let Some(index) = known(&KEYS, &key) else {
            skip_unknown(&mut map, ctx, &mut others, &key)?;
            continue;
        };
        seen(ctx, &mut mask, index, &key)?;
        match index {
            0 => id = Some(value(&mut map, ctx, "id")?),
            1 => name = Some(value(&mut map, ctx, "name")?),
            2 => color = Some(value(&mut map, ctx, "color")?),
            3 => {
                let token: String = value(&mut map, ctx, "direction")?;
                direction = tokens::parse_direction(&token).unwrap_or_else(|| {
                    ctx.unknown_value("direction", &token);
                    SurveyDirection::In
                });
            }
            4 => quality = value(&mut map, ctx, "quality")?,
            5 => section = Some(value(&mut map, ctx, "section")?),
            6 => {
                ctx.field = "azimuth";
                azimuth = Some(map.next_value_seed(AzimuthSeed { ctx: &mut *ctx })?);
                ctx.field = "";
            }
            7 => surveyor = Some(value(&mut map, ctx, "surveyor")?),
            8 => {
                let text: String = value(&mut map, ctx, "date")?;
                ctx.field = "date";
                date = Some(
                    parse_date(&text).ok_or_else(|| ctx.bad(format!("{text:?} is not a date")))?,
                );
                ctx.field = "";
            }
            9 => {
                ctx.field = "credit";
                let read = map.next_value_seed(OptCreditSeed { ctx: &mut *ctx })?;
                ctx.field = "";
                credit = read.map(|c| ctx.intern(c));
            }
            10 => start = Some(value(&mut map, ctx, "start")?),
            _ => {
                ctx.field = "hand";
                map.next_value_seed(HandSeed {
                    ctx: &mut *ctx,
                    out: &mut hand,
                })?;
                ctx.field = "";
            }
        }
    }
    let id = id.ok_or_else(|| ctx.missing("id"))?;
    let color = color.ok_or_else(|| ctx.missing("color"))?;
    let start = start.ok_or_else(|| ctx.missing("start"))?;
    let line = Line {
        id,
        name,
        color,
        direction,
        quality,
        azimuth_reference: azimuth,
        section_id: section,
        legacy_surveyor: surveyor,
        legacy_date: date,
        start,
        shots: Vec::new(),
        hand_moves: hand,
        next_shot_id: 0,
    };
    Ok(LineBuild { line, credit })
}

struct HandSeed<'c, 'l> {
    ctx: &'c mut Ctx<'l>,
    out: &'c mut BTreeMap<StationId, [f64; 3]>,
}

impl<'de> DeserializeSeed<'de> for HandSeed<'_, '_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for HandSeed<'_, '_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an array of hand moves [station, dx, dy, dz]")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some((station, dx, dy, dz)) = seq.next_element::<(StationId, f64, f64, f64)>()? {
            self.ctx.count(Limit::HandMoves, 1)?;
            if self.out.insert(station, [dx, dy, dz]).is_some() {
                return Err(self
                    .ctx
                    .bad(format!("has two hand moves at station {station}")));
            }
        }
        Ok(())
    }
}

/// A cell after the seven required ones: an LRUD value (a number or null) or
/// the row's extras object.
enum Tail {
    Lrud(Option<f64>),
    Extras(RowExtras),
}

#[derive(Default)]
struct RowExtras {
    kind: ShotKind,
    pitch_in: Option<f64>,
    pitch_out: Option<f64>,
    temperature: Option<f64>,
    marker: Option<u32>,
    comment: Option<String>,
    credit: RowCredit,
}

/// A row's credit: the line header's, none (`"credit":null`), or its own.
#[derive(Default)]
enum RowCredit {
    #[default]
    Inherit,
    Nobody,
    Own(SurveyCredit),
}

fn read_shot<'de, A: SeqAccess<'de>>(
    mut seq: A,
    ctx: &mut Ctx<'_>,
    line_credit: Option<&Arc<SurveyCredit>>,
) -> Result<Shot, A::Error> {
    let uuid: String = cell(&mut seq, ctx, "uuid")?;
    let to: StationId = cell(&mut seq, ctx, "to")?;
    let length: f64 = cell(&mut seq, ctx, "length")?;
    let azimuth_in: f64 = cell(&mut seq, ctx, "azimuth_in")?;
    let azimuth_out: f64 = cell(&mut seq, ctx, "azimuth_out")?;
    let depth_in: f64 = cell(&mut seq, ctx, "depth_in")?;
    let depth_out: f64 = cell(&mut seq, ctx, "depth_out")?;
    const LRUD: [&str; 4] = ["left", "right", "up", "down"];
    let mut lrud = [None; 4];
    let mut extras = RowExtras::default();
    let mut cells = 0;
    loop {
        ctx.field = LRUD.get(cells).copied().unwrap_or("extras");
        match seq.next_element_seed(TailSeed { ctx: &mut *ctx })? {
            None => break,
            Some(Tail::Lrud(v)) => {
                let Some(slot) = lrud.get_mut(cells) else {
                    return Err(ctx.bad("has more cells than the format defines"));
                };
                *slot = v;
                cells += 1;
            }
            Some(Tail::Extras(e)) => {
                extras = e;
                end_of_row(&mut seq, ctx)?;
                break;
            }
        }
    }
    ctx.field = "";
    let credit = match extras.credit {
        RowCredit::Nobody => None,
        RowCredit::Own(own) => Some(ctx.intern(own)),
        RowCredit::Inherit => line_credit.map(Arc::clone),
    };
    let [left, right, up, down] = lrud;
    Ok(Shot {
        id: 0,
        uuid,
        to,
        azimuth_in,
        azimuth_out,
        length,
        depth_in,
        depth_out,
        pitch_in: extras.pitch_in,
        pitch_out: extras.pitch_out,
        left,
        right,
        up,
        down,
        comment: extras.comment,
        kind: extras.kind,
        marker_index: extras.marker,
        temperature: extras.temperature,
        credit,
    })
}

struct TailSeed<'c, 'l> {
    ctx: &'c mut Ctx<'l>,
}

impl<'de> DeserializeSeed<'de> for TailSeed<'_, '_> {
    type Value = Tail;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Tail, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for TailSeed<'_, '_> {
    type Value = Tail;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a number, null, or the shot's extras object")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Tail, E> {
        Ok(Tail::Lrud(None))
    }

    fn visit_none<E: de::Error>(self) -> Result<Tail, E> {
        Ok(Tail::Lrud(None))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Tail, E> {
        Ok(Tail::Lrud(Some(v)))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Tail, E> {
        Ok(Tail::Lrud(Some(v as f64)))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Tail, E> {
        Ok(Tail::Lrud(Some(v as f64)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Tail, A::Error> {
        const KEYS: [&str; 7] = [
            "kind",
            "pitch_in",
            "pitch_out",
            "temperature",
            "marker",
            "comment",
            "credit",
        ];
        let ctx = self.ctx;
        let mut extras = RowExtras::default();
        let mut mask = 0;
        let mut others = Vec::new();
        while let Some(key) = map.next_key::<Key<'de>>()? {
            let key = key.0;
            let Some(index) = known(&KEYS, &key) else {
                skip_unknown(&mut map, ctx, &mut others, &key)?;
                continue;
            };
            seen(ctx, &mut mask, index, &key)?;
            match index {
                0 => {
                    let token: String = value(&mut map, ctx, "kind")?;
                    extras.kind = tokens::parse_kind(&token).unwrap_or_else(|| {
                        // Fail closed: an unknown kind must not become a
                        // swimmable leg (#27).
                        ctx.unknown_value("kind", &token);
                        ShotKind::Reference
                    });
                }
                1 => extras.pitch_in = Some(value(&mut map, ctx, "pitch_in")?),
                2 => extras.pitch_out = Some(value(&mut map, ctx, "pitch_out")?),
                3 => extras.temperature = Some(value(&mut map, ctx, "temperature")?),
                4 => extras.marker = Some(value(&mut map, ctx, "marker")?),
                5 => extras.comment = Some(value(&mut map, ctx, "comment")?),
                _ => {
                    ctx.field = "credit";
                    extras.credit = match map.next_value_seed(OptCreditSeed { ctx: &mut *ctx })? {
                        Some(own) => RowCredit::Own(own),
                        None => RowCredit::Nobody,
                    };
                    ctx.field = "";
                }
            }
        }
        Ok(Tail::Extras(extras))
    }
}

// ---------------------------------------------------------------------------
// Credits and azimuth references
// ---------------------------------------------------------------------------

/// A credit object, or `null` for "no credit".
pub(super) struct OptCreditSeed<'c, 'l> {
    pub(super) ctx: &'c mut Ctx<'l>,
}

impl<'de> DeserializeSeed<'de> for OptCreditSeed<'_, '_> {
    type Value = Option<SurveyCredit>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for OptCreditSeed<'_, '_> {
    type Value = Option<SurveyCredit>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a credit object or null")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        const KEYS: [&str; 3] = ["surveyors", "explorers", "date"];
        let ctx = self.ctx;
        let mut credit = SurveyCredit::default();
        let mut mask = 0;
        let mut others = Vec::new();
        while let Some(key) = map.next_key::<Key<'de>>()? {
            let key = key.0;
            let Some(index) = known(&KEYS, &key) else {
                skip_unknown(&mut map, ctx, &mut others, &key)?;
                continue;
            };
            seen(ctx, &mut mask, index, &key)?;
            match index {
                0 | 1 => {
                    let out = if index == 0 {
                        &mut credit.surveyors
                    } else {
                        &mut credit.explorers
                    };
                    ctx.field = KEYS[index];
                    map.next_value_seed(PeopleSeed {
                        ctx: &mut *ctx,
                        out,
                    })?;
                    ctx.field = "";
                }
                _ => {
                    let text: String = value(&mut map, ctx, "date")?;
                    ctx.field = "date";
                    credit.date = Some(
                        parse_date(&text)
                            .ok_or_else(|| ctx.bad(format!("{text:?} is not a date")))?,
                    );
                    ctx.field = "";
                }
            }
        }
        Ok(Some(credit))
    }
}

struct PeopleSeed<'c, 'l> {
    ctx: &'c mut Ctx<'l>,
    out: &'c mut Vec<Person>,
}

impl<'de> DeserializeSeed<'de> for PeopleSeed<'_, '_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for PeopleSeed<'_, '_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an array of people")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(person) = seq.next_element_seed(PersonSeed {
            ctx: &mut *self.ctx,
        })? {
            self.ctx.count(Limit::CreditPeople, 1)?;
            self.out.push(person);
        }
        Ok(())
    }
}

struct PersonSeed<'c, 'l> {
    ctx: &'c mut Ctx<'l>,
}

impl<'de> DeserializeSeed<'de> for PersonSeed<'_, '_> {
    type Value = Person;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Person, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for PersonSeed<'_, '_> {
    type Value = Person;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a person {name, email?}")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Person, A::Error> {
        const KEYS: [&str; 2] = ["name", "email"];
        let ctx = self.ctx;
        let mut mask = 0;
        let mut others = Vec::new();
        let mut name = None;
        let mut email = None;
        while let Some(key) = map.next_key::<Key<'de>>()? {
            let key = key.0;
            let Some(index) = known(&KEYS, &key) else {
                skip_unknown(&mut map, ctx, &mut others, &key)?;
                continue;
            };
            seen(ctx, &mut mask, index, &key)?;
            if index == 0 {
                name = Some(value(&mut map, ctx, "name")?);
            } else {
                email = Some(value(&mut map, ctx, "email")?);
            }
        }
        let name = name.ok_or_else(|| ctx.missing("name"))?;
        Ok(Person { name, email })
    }
}

/// An azimuth reference `{north, correction, source, epoch?}`, kept exactly
/// as written (load never sanitises; geometry applies the type's rules).
pub(super) struct AzimuthSeed<'c, 'l> {
    pub(super) ctx: &'c mut Ctx<'l>,
}

impl<'de> DeserializeSeed<'de> for AzimuthSeed<'_, '_> {
    type Value = AzimuthReference;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<AzimuthReference, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for AzimuthSeed<'_, '_> {
    type Value = AzimuthReference;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an azimuth reference {north, correction, source}")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<AzimuthReference, A::Error> {
        const KEYS: [&str; 4] = ["north", "correction", "source", "epoch"];
        let ctx = self.ctx;
        let mut mask = 0;
        let mut others = Vec::new();
        let mut north = None;
        let mut correction = 0.0;
        let mut source = None;
        let mut epoch = None;
        while let Some(key) = map.next_key::<Key<'de>>()? {
            let key = key.0;
            let Some(index) = known(&KEYS, &key) else {
                skip_unknown(&mut map, ctx, &mut others, &key)?;
                continue;
            };
            seen(ctx, &mut mask, index, &key)?;
            match index {
                0 => north = Some(value::<String, _>(&mut map, ctx, "north")?),
                1 => correction = value(&mut map, ctx, "correction")?,
                2 => source = Some(value::<String, _>(&mut map, ctx, "source")?),
                _ => epoch = Some(value::<i32, _>(&mut map, ctx, "epoch")?),
            }
        }
        let north = match north.as_deref() {
            None => NorthReference::Magnetic,
            Some(token) => tokens::parse_north(token).unwrap_or_else(|| {
                ctx.unknown_value("north", token);
                NorthReference::Magnetic
            }),
        };
        let source = match source.as_deref() {
            None if epoch.is_none() => CorrectionSource::Unknown,
            token => tokens::parse_source(token.unwrap_or("unknown"), epoch).unwrap_or_else(|| {
                ctx.unknown_value("source", token.unwrap_or(""));
                CorrectionSource::Unknown
            }),
        };
        Ok(AzimuthReference {
            north,
            correction,
            source,
        })
    }
}
