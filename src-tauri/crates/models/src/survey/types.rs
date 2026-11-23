//! The entities of the station-graph model (plan 1.2).
//!
//! Measurement fields, credits, shot kinds, azimuth references, GPS
//! coordinates, survey directions, Sections and validation thresholds are the
//! crate's existing shared types; only the topology-bearing records are new.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::config::ValidationConfig;
use crate::shot::horizontal_length_from_slope;
use crate::{
    average_azimuth_of, AzimuthReference, GpsCoordinates, LineData, LineId, SectionId, ShotData,
    ShotId, ShotKind, StationId, SurveyCredit, SurveyDirection, FEATURE_ID_STRIDE,
};

use super::ClosureEnd;

/// A loop closure's id: project-unique, never reused.
pub type ClosureId = u64;

/// The bound of the closure id space: 2^53 - 1, the largest integer a
/// `.stalx` file holds (every JSON reader holds it exactly). Closure ids are
/// below it and the closure counter is at most it: at the bound the ids are
/// used up. Checked by `validate` (`IdOutOfRange`, `CounterOutOfRange`), so
/// a file at the bound opens and an edit that would mint past it is refused
/// instead of leaving a project no save can write (review finding
/// C-file-format-6).
pub const MAX_CLOSURE_ID: ClosureId = (1 << 53) - 1;

/// The longest shot uuid the model accepts, in characters (plan 1.1). Shot
/// uuids are opaque: minted v4 at creation, imported verbatim.
pub const MAX_SHOT_UUID_CHARS: usize = 128;

/// The most shots one line holds (99,999): session shot ids are
/// `line * FEATURE_ID_STRIDE + index`, so a longer line would hand out the
/// next line's ids (the legacy model's cap too). Checked by `validate`.
pub const MAX_SHOTS_PER_LINE: usize = (FEATURE_ID_STRIDE - 1) as usize;

/// One physical survey station: a node of the survey graph, with the
/// identity it shows (its own, unless a loop closure archives it, plan
/// 2026-11-23 RS 5.1) and the identities merges put away in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Station {
    /// The Station ID the user sees. A junction has one id.
    pub id: StationId,
    /// The name every line through the station shows it by (unless a loop
    /// closure archives it); unique among the names the native slots of each
    /// of its native lines show (invariant N8, RS-FX5: a foreign slot or a
    /// loop end counts for no uniqueness).
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// The identity an importer gave the station (a Compass or Walls name, an
    /// Ariane ID). An attribute only: nothing resolves through it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<String>,
    /// The stations merged into this one and not kept (RS 3), oldest first:
    /// each keeps its own identity and the lines whose own station it was,
    /// is shown nowhere, and comes back with Detach Line from Station. Each
    /// names the identity it was archived under ([`ArchivedStation::into`]),
    /// so the list is a forest under this station's own identity
    /// (`records.rs`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub archived: Vec<ArchivedStation>,
}

impl Station {
    /// A station with a name and nothing else.
    pub fn new(id: StationId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            comment: None,
            external: None,
            archived: Vec::new(),
        }
    }

    /// The record of this station that names `line`, if any (invariant A3:
    /// at most one does).
    pub fn record_for(&self, line: LineId) -> Option<&ArchivedStation> {
        self.archived.iter().find(|r| r.lines.contains(&line))
    }

    /// This station's own identity as a record naming `lines` (what a merge
    /// or a restore archives), archived under the identity of the station
    /// that will hold it (`into` unset).
    pub fn as_record(&self, lines: Vec<LineId>) -> ArchivedStation {
        ArchivedStation {
            id: self.id,
            name: self.name.clone(),
            comment: self.comment.clone(),
            external: self.external.clone(),
            into: None,
            lines,
        }
    }
}

/// A station a merge put away inside another one (RS 3): its identity, the
/// identity it was archived under, and the lines whose own station it was.
/// Shown nowhere, its name counts toward no uniqueness, and Detach Line from
/// Station on one of its lines (or of the records archived under it) brings
/// it back with the records archived under it. A record with no lines is
/// history (invariant A5): it reserves its id, owns no record, and nothing
/// restores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivedStation {
    pub id: StationId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<String>,
    /// The identity this record was archived under (RS-FX4): unset for the
    /// identity of the station holding it, else the Station ID of another
    /// record of that station, which held this one as its own record when a
    /// later merge archived it (invariant A6: never itself, never a history
    /// record, never a cycle). A record and the records under it are one
    /// former junction, which Detach Line from Station gives back whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub into: Option<StationId>,
    /// The lines whose own station this was; each passes the station that
    /// holds the record (invariant A2), and names at most one record there
    /// (A3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<LineId>,
}

impl ArchivedStation {
    /// Whether this record is history only (A5: it names no line).
    pub fn is_history(&self) -> bool {
        self.lines.is_empty()
    }

    /// The live station this record restores to (no records of its own:
    /// the caller moves the records under it, `records.rs`).
    pub fn to_station(&self) -> Station {
        Station {
            id: self.id,
            name: self.name.clone(),
            comment: self.comment.clone(),
            external: self.external.clone(),
            archived: Vec::new(),
        }
    }
}

/// One survey run: a simple chain of stations, `start` then each shot's `to`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub id: LineId,
    pub name: Option<String>,
    pub color: String,
    pub direction: SurveyDirection,
    /// Weight of this line's shots in the least squares (1.0 = default).
    pub quality: f64,
    /// `None` inherits [`ProjectSettings::default_azimuth_reference`].
    pub azimuth_reference: Option<AzimuthReference>,
    /// The cave Section this line belongs to, if any.
    pub section_id: Option<SectionId>,
    /// A shot-less line's legacy surveyor text (credit fallback), as today.
    pub legacy_surveyor: Option<String>,
    /// A shot-less line's legacy survey date, as today.
    pub legacy_date: Option<NaiveDate>,
    /// Station 0 of the chain.
    pub start: StationId,
    /// Shot `i` runs from chain station `i` to chain station `i + 1`.
    pub shots: Vec<Shot>,
    /// Legacy per-slot hand moves (plan 5.6), `(dx, dy, dz)` added to the chain
    /// position of a station on this line. No current command creates one.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hand_moves: BTreeMap<StationId, [f64; 3]>,
    /// Session only: the next [`ShotId`] to hand out on this line.
    #[serde(skip)]
    pub next_shot_id: ShotId,
}

impl Line {
    /// The chain: `start`, then each shot's `to`. Allocation-free; the cached
    /// form is [`super::Topology::chain`].
    pub fn stations(&self) -> impl Iterator<Item = StationId> + '_ {
        std::iter::once(self.start).chain(self.shots.iter().map(|shot| shot.to))
    }

    /// Station `index` of the chain (`0` is `start`).
    pub fn station(&self, index: usize) -> Option<StationId> {
        if index == 0 {
            Some(self.start)
        } else {
            self.shots.get(index - 1).map(|shot| shot.to)
        }
    }

    /// The station shot `index` starts from.
    pub fn shot_from(&self, index: usize) -> Option<StationId> {
        if index < self.shots.len() {
            self.station(index)
        } else {
            None
        }
    }

    /// The line's measured length, metres: its `Normal` and `Jump` shots'
    /// lengths summed in shot order (`Reference` shots excluded), exactly as
    /// line details and the survey statistics compute it. Rule K (RS 2)
    /// compares these as exact f64 values.
    pub fn measured_length(&self) -> f64 {
        let mut length = 0.0;
        for shot in &self.shots {
            match shot.kind {
                ShotKind::Normal | ShotKind::Jump => length += shot.length,
                ShotKind::Reference => {}
            }
        }
        length
    }

    /// Number of stations on the chain (`shots + 1`).
    pub fn station_count(&self) -> usize {
        self.shots.len() + 1
    }

    /// This line's hand move at `station`, zero when it has none.
    pub fn hand(&self, station: StationId) -> [f64; 3] {
        self.hand_moves
            .get(&station)
            .copied()
            .unwrap_or([0.0, 0.0, 0.0])
    }

    /// The line's metadata (everything but the chain and the hand moves).
    pub fn meta(&self) -> LineMeta {
        LineMeta {
            name: self.name.clone(),
            color: self.color.clone(),
            direction: self.direction,
            quality: self.quality,
            azimuth_reference: self.azimuth_reference,
            section_id: self.section_id,
            legacy_surveyor: self.legacy_surveyor.clone(),
            legacy_date: self.legacy_date,
        }
    }

    /// Hand out the next session [`ShotId`] for a new shot on this line.
    pub fn mint_shot_id(&mut self) -> ShotId {
        let id = self.next_shot_id;
        self.next_shot_id = self.next_shot_id.saturating_add(1);
        id
    }

    /// Re-number every shot's session id as `id * FEATURE_ID_STRIDE + index`,
    /// exactly as a load does today, and set `next_shot_id` past them.
    pub fn assign_session_shot_ids(&mut self) {
        let base = self.id.saturating_mul(FEATURE_ID_STRIDE);
        for (index, shot) in self.shots.iter_mut().enumerate() {
            shot.id = base.saturating_add(index as u32);
        }
        self.next_shot_id = base.saturating_add(self.shots.len() as u32);
    }
}

/// A line's metadata, for building lines ([`super::Project::add_line`]).
#[derive(Debug, Clone, PartialEq)]
pub struct LineMeta {
    pub name: Option<String>,
    pub color: String,
    pub direction: SurveyDirection,
    pub quality: f64,
    pub azimuth_reference: Option<AzimuthReference>,
    pub section_id: Option<SectionId>,
    pub legacy_surveyor: Option<String>,
    pub legacy_date: Option<NaiveDate>,
}

impl Default for LineMeta {
    fn default() -> Self {
        Self {
            name: None,
            color: String::new(),
            direction: SurveyDirection::In,
            quality: 1.0,
            azimuth_reference: None,
            section_id: None,
            legacy_surveyor: None,
            legacy_date: None,
        }
    }
}

impl LineMeta {
    /// The metadata half of a legacy [`LineData`] (its shots are not read).
    pub fn from_line_data(data: &LineData) -> Self {
        Self {
            name: data.name.clone(),
            color: data.color.clone(),
            direction: data.direction.unwrap_or_default(),
            quality: data.quality,
            azimuth_reference: None,
            section_id: None,
            legacy_surveyor: None,
            legacy_date: None,
        }
    }
}

/// One shot: an edge from the previous chain station to `to`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    /// Session handle (`line_id * FEATURE_ID_STRIDE + seq`), never persisted.
    #[serde(skip)]
    pub id: ShotId,
    /// Persistent identity, project-unique, at most [`MAX_SHOT_UUID_CHARS`].
    pub uuid: String,
    /// The station this shot ends at.
    pub to: StationId,
    pub azimuth_in: f64,
    pub azimuth_out: f64,
    /// Slope length in metres.
    pub length: f64,
    pub depth_in: f64,
    pub depth_out: f64,
    pub pitch_in: Option<f64>,
    pub pitch_out: Option<f64>,
    pub left: Option<f64>,
    pub right: Option<f64>,
    pub up: Option<f64>,
    pub down: Option<f64>,
    pub comment: Option<String>,
    pub kind: ShotKind,
    pub marker_index: Option<u32>,
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit: Option<Arc<SurveyCredit>>,
}

impl Shot {
    /// A shot from the legacy measurement record, ending at `to`.
    ///
    /// Identity and credit go through the crate's one decision point,
    /// [`crate::Shot::with_uuid`]: the data's own uuid wins, a missing one is
    /// minted (v4), and the credit is stored in its normalised form.
    pub fn from_data(id: ShotId, data: ShotData, to: StationId) -> Self {
        let legacy = crate::Shot::with_uuid(id, data, None);
        let uuid = crate::Shot::uuid_or_mint(legacy.uuid);
        Self {
            id,
            uuid,
            to,
            azimuth_in: legacy.azimuth_in,
            azimuth_out: legacy.azimuth_out,
            length: legacy.length,
            depth_in: legacy.depth_in,
            depth_out: legacy.depth_out,
            pitch_in: legacy.pitch_in,
            pitch_out: legacy.pitch_out,
            left: legacy.left,
            right: legacy.right,
            up: legacy.up,
            down: legacy.down,
            comment: legacy.comment,
            kind: legacy.kind,
            marker_index: legacy.marker_index,
            temperature: legacy.temperature,
            credit: legacy.credit,
        }
    }

    /// The measurement record of this shot, uuid included.
    pub fn to_shot_data(&self) -> ShotData {
        ShotData {
            uuid: Some(self.uuid.clone()),
            azimuth_in: self.azimuth_in,
            azimuth_out: self.azimuth_out,
            length: self.length,
            depth_in: self.depth_in,
            depth_out: self.depth_out,
            pitch_in: self.pitch_in,
            pitch_out: self.pitch_out,
            left: self.left,
            right: self.right,
            up: self.up,
            down: self.down,
            comment: self.comment.clone(),
            kind: self.kind,
            marker_index: self.marker_index,
            temperature: self.temperature,
            credit: self.credit.clone().filter(|credit| !credit.is_empty()),
        }
    }

    /// Horizontal projection of the slope length, from the shot's own depth
    /// pair: today's formula ([`crate::Shot::horizontal_length`]).
    pub fn horizontal_length(&self) -> f64 {
        horizontal_length_from_slope(self.length, self.depth_out - self.depth_in)
    }

    /// The true-north bearing this shot walks, in radians: today's
    /// [`crate::Shot::average_azimuth_true_rad`].
    pub fn average_azimuth_true_rad(&self, reference: AzimuthReference) -> f64 {
        reference
            .to_true(average_azimuth_of(self.azimuth_in, self.azimuth_out))
            .to_radians()
    }

    /// The horizontal vector from the shot's start to its end, `(dx, dy)` in
    /// local ENU metres: `h * (sin az, cos az)`, exactly as today's walk.
    pub fn vector(&self, reference: AzimuthReference) -> [f64; 2] {
        let azimuth = self.average_azimuth_true_rad(reference);
        let horizontal = self.horizontal_length();
        [horizontal * azimuth.sin(), horizontal * azimuth.cos()]
    }

    /// Every number of the shot, named, for the finiteness check.
    pub(super) fn numbers(&self) -> [(&'static str, Option<f64>); 12] {
        [
            ("azimuth_in", Some(self.azimuth_in)),
            ("azimuth_out", Some(self.azimuth_out)),
            ("length", Some(self.length)),
            ("depth_in", Some(self.depth_in)),
            ("depth_out", Some(self.depth_out)),
            ("pitch_in", self.pitch_in),
            ("pitch_out", self.pitch_out),
            ("left", self.left),
            ("right", self.right),
            ("up", self.up),
            ("down", self.down),
            ("temperature", self.temperature),
        ]
    }
}

/// A zero datum: a station tied to a GPS position.
///
/// Its role is derived: the first datum of a component (list order) places
/// it, every later one is a GPS fix (plan 3.8).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ZeroDatum {
    pub station: StationId,
    /// Attachment hint: the line whose slot the datum was set on (plan 5.2).
    /// When set, the line must pass through `station`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<LineId>,
    pub gps: GpsCoordinates,
    /// `None` is the default accuracy (a hard pin).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy_m: Option<f64>,
    /// The entrance override; `None` is the default rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrance: Option<bool>,
    /// Horizontal displacement (local metres) between where this placement
    /// datum's piece sits and where its GPS says, relative to the current
    /// frame. Maintained ("measure, do not re-derive", plan 4.3): set by frame
    /// changes and Add .stalx so nothing moves; cleared by a GPS edit of this
    /// datum and when it is demoted to a GPS fix. `None` for a datum that sits
    /// exactly on its GPS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correction: Option<[f64; 2]>,
}

impl ZeroDatum {
    /// A datum with no hint, default accuracy, default entrance rule and no
    /// correction.
    pub fn new(station: StationId, gps: GpsCoordinates) -> Self {
        Self {
            station,
            via: None,
            gps,
            accuracy_m: None,
            entrance: None,
            correction: None,
        }
    }

    /// The correction as a vector, zero when there is none.
    pub fn correction_or_zero(&self) -> [f64; 2] {
        self.correction.unwrap_or([0.0, 0.0])
    }

    /// Whether `other` has bit-for-bit the same GPS position.
    pub fn same_gps_bits(&self, other: &GpsCoordinates) -> bool {
        gps_bits(&self.gps) == gps_bits(other)
    }
}

/// The bit patterns of a GPS position, for exact comparison.
pub(super) fn gps_bits(gps: &GpsCoordinates) -> [u64; 3] {
    [
        gps.latitude.to_bits(),
        gps.longitude.to_bits(),
        gps.depth_reference.to_bits(),
    ]
}

impl PartialEq for ZeroDatum {
    /// Field-wise, with the GPS compared by value (the shared
    /// [`GpsCoordinates`] has no `PartialEq` of its own).
    fn eq(&self, other: &Self) -> bool {
        self.station == other.station
            && self.via == other.via
            && self.gps.latitude == other.gps.latitude
            && self.gps.longitude == other.gps.longitude
            && self.gps.depth_reference == other.gps.depth_reference
            && self.accuracy_m == other.accuracy_m
            && self.entrance == other.entrance
            && self.correction == other.correction
    }
}

/// The anchor of a component that holds no datum: `station` sits at local
/// `(x, y)`. Exactly one per such component (invariant 6).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub station: StationId,
    pub x: f64,
    pub y: f64,
}

/// A loop closure between two stations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopClosure {
    pub id: ClosureId,
    pub a: StationId,
    /// Attachment hint for end `a` (plan 5.2); the line must pass through `a`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a_via: Option<LineId>,
    pub b: StationId,
    /// Attachment hint for end `b`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b_via: Option<LineId>,
    pub strategy: ClosureStrategy,
    /// The end this closure archives (RS 5): that end shows the other end's
    /// identity (`Project::shown`) while it stays a node of its own. `None`
    /// archives nothing (both ends show their own names).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<ClosureEnd>,
}

impl LoopClosure {
    /// The archived end's station and the kept end's, when this closure
    /// archives one and its ends are two stations.
    pub fn archive(&self) -> Option<(StationId, StationId)> {
        if self.a == self.b {
            return None;
        }
        match self.archived? {
            ClosureEnd::A => Some((self.a, self.b)),
            ClosureEnd::B => Some((self.b, self.a)),
        }
    }

    /// The station at `end`.
    pub fn end(&self, end: ClosureEnd) -> StationId {
        match end {
            ClosureEnd::A => self.a,
            ClosureEnd::B => self.b,
        }
    }
}

/// How a loop closure is treated (plan 3.6).
///
/// The shared [`crate::ClosureStrategy`] plus [`Self::Tie`], with the same
/// serde names and legacy aliases for the shared variants. It is a separate
/// enum in this stream so the generated TypeScript of the shared one does not
/// change before the wire layer switches (plan "Deviations", D1).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq, Hash)]
pub enum ClosureStrategy {
    /// Legacy only; refused at creation.
    ProportionalByLength,
    /// Legacy only; refused at creation.
    EqualDistribution,
    /// A soft loop closure, relaxed only by the Closures Computed View.
    #[serde(alias = "KeepRaw", alias = "ManualAdjustment")]
    #[default]
    Batch,
    /// Legacy name of a soft closure (read as eligible, like `Batch`).
    #[serde(alias = "Sequential")]
    LeastSquaresWeighted,
    /// A snap: rigid while its ends are within [`super::ALIGNMENT_TOLERANCE_M`]
    /// and the tie would not double-anchor; inert when torn.
    Move,
    /// A legacy rigid tie (migrated cycle-closing / same-line / self merges and
    /// legacy aligned non-`Move` closures). Always aligned; never derived;
    /// never created by new commands.
    Tie,
}

impl ClosureStrategy {
    /// The stored spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProportionalByLength => "ProportionalByLength",
            Self::EqualDistribution => "EqualDistribution",
            Self::Batch => "Batch",
            Self::LeastSquaresWeighted => "LeastSquaresWeighted",
            Self::Move => "Move",
            Self::Tie => "Tie",
        }
    }

    /// Parse a stored spelling (legacy aliases included), defaulting to
    /// `Batch` like the shared enum.
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "Tie" => Self::Tie,
            other => crate::ClosureStrategy::from_str_lossy(other).into(),
        }
    }

    /// The shared enum's variant, or `None` for [`Self::Tie`], which has none.
    pub fn to_shared(self) -> Option<crate::ClosureStrategy> {
        match self {
            Self::ProportionalByLength => Some(crate::ClosureStrategy::ProportionalByLength),
            Self::EqualDistribution => Some(crate::ClosureStrategy::EqualDistribution),
            Self::Batch => Some(crate::ClosureStrategy::Batch),
            Self::LeastSquaresWeighted => Some(crate::ClosureStrategy::LeastSquaresWeighted),
            Self::Move => Some(crate::ClosureStrategy::Move),
            Self::Tie => None,
        }
    }
}

impl From<crate::ClosureStrategy> for ClosureStrategy {
    fn from(strategy: crate::ClosureStrategy) -> Self {
        match strategy {
            crate::ClosureStrategy::ProportionalByLength => Self::ProportionalByLength,
            crate::ClosureStrategy::EqualDistribution => Self::EqualDistribution,
            crate::ClosureStrategy::Batch => Self::Batch,
            crate::ClosureStrategy::LeastSquaresWeighted => Self::LeastSquaresWeighted,
            crate::ClosureStrategy::Move => Self::Move,
        }
    }
}

/// Project-wide settings that are survey content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ProjectSettings {
    pub validation: ValidationConfig,
    /// Entrance elevation, +up, metres above mean sea level (#431).
    pub surface_elevation: Option<f64>,
    /// The azimuth reference a line inherits when it declares none (#438).
    pub default_azimuth_reference: Option<AzimuthReference>,
}

/// The stored id allocators (invariant 9). Each is above every id of its kind
/// ever issued in this timeline; ids are never reused. Line ids are not
/// counted: `next_line_id` is derived as `max + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    pub next_station_id: StationId,
    pub next_closure_id: ClosureId,
    pub next_section_id: SectionId,
}

impl Default for Counters {
    /// Station and Section ids start at 0, closure ids at 1 (as today).
    fn default() -> Self {
        Self {
            next_station_id: 0,
            next_closure_id: 1,
            next_section_id: 0,
        }
    }
}
