//! The reader's public survey model and its conversion from the app's
//! station-graph model (`stalagmight_models::survey::Project`).
//!
//! These types are deliberately separate from `stalagmight_models`: the app's
//! model changes often, and every change there must not become a breaking
//! change for a consumer. Each struct is `#[non_exhaustive]` so fields can be
//! added in a minor release.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use stalagmight_models::station::{GpsCoordinates, StationRef};
use stalagmight_models::survey::{ClosureStrategy, Project};
use stalagmight_models::{
    join_names, LineCreditSummary, LineId, NorthReference, ShotKind as ModelShotKind, SurveyCredit,
    SurveyDirection, EMPTY_CREDIT,
};

/// The app's station identity (the user-visible Station ID).
type ModelStationId = stalagmight_models::StationId;

/// One project, read from a `.stalx` file.
///
/// Conventions, for every field below:
/// - Lengths and depths are metres. Angles are degrees.
/// - Depth and pitch are positive DOWN.
/// - Azimuths are as stored; add the section's [`Section::azimuth_correction_deg`]
///   to get a true-north bearing.
///
/// # Stations and junctions
///
/// A section (survey line) is a chain of stations: [`StationId`] `{ section_id,
/// index }` names the `index`-th station of a section. That address is
/// positional. The station itself has a project-wide id, the Station ID the
/// app shows ([`Section::station_ids`], [`StationPosition::station_id`]): a
/// station on two or more sections (a **junction**, what the app used to call
/// a merge) has ONE Station ID, one name and one comment, and is listed at
/// every one of its positions. [`Survey::station_merges`] pairs the positions
/// of each junction.
///
/// # Identity
///
/// - [`Section::id`] is stable across edits for the life of the project.
/// - The Station ID ([`Section::station_ids`]) is stable across edits and never
///   reused; the junction of two sections keeps the id of one of them.
/// - [`StationId::index`] is NOT stable: inserting or deleting a shot shifts the
///   indexes after it, and reversing a section renumbers station `i` to `N - i`.
///   Key anything persistent on the Station ID or on `(section id, shot uuid)`,
///   never on an index.
/// - [`Shot::uuid`] is stable across edits and unique across the whole survey.
///   An SQLite file can hold a shot with no usable UUID (files from before shot
///   UUIDs, a second shot repeating one in its section, or a section repeating
///   an id an earlier section holds - the same Ariane file imported twice, a
///   merge of two copies): it gets one derived deterministically, the one the
///   app's own conversion of the file gives it, flagged [`Shot::uuid_derived`].
///   It persists once the app saves the file (as JSON). A JSON file holds a
///   UUID for every shot.
///
/// # Ordering
///
/// Every list is sorted, so the same file content always reads to the same
/// value: sections by id, shots in survey order, merges by `(a, b)`, zero
/// datums, comments, names and station positions by station, loop closures by
/// id. [`Survey::modified_at`] changes on every save; leave it out of any
/// content hash.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Survey {
    /// Which of the two `.stalx` formats the file is.
    pub format: FileFormat,
    /// The major format version: 1 for an SQLite file, the `version` member of
    /// a JSON file (2 for every file this crate reads).
    pub format_version: u64,
    /// An SQLite file's schema version as uploaded, read before the private
    /// copy is migrated forward (`PRAGMA user_version`, or the project row's
    /// own version for files that predate the pragma; 0 if neither is
    /// recorded). Always 0 for a JSON file (see [`Survey::format_version`]).
    pub schema_version: i32,
    pub name: String,
    pub location: Option<String>,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub modified_at: String,
    pub sections: Vec<Section>,
    /// Two positions of one junction station: for each station on two or more
    /// sections, its first position (lowest `(section_id, index)`) paired
    /// with each of the others. Both ends always have the same Station ID.
    pub station_merges: Vec<StationMerge>,
    /// Every zero datum: each component's placement datum and its GPS fixes
    /// alike. [`StationPosition::anchor`] names the placement datum.
    pub zero_datums: Vec<ZeroDatum>,
    pub loop_closures: Vec<LoopClosure>,
    /// One per station that has a comment, at the station's first position
    /// (lowest `(section_id, index)`): a junction's comment is listed once.
    pub station_comments: Vec<StationComment>,
    /// The station names (e.g. the MNemo station number a DMP import
    /// recorded), at every position: a junction's one name is listed once per
    /// section it is on. Unique among the stations of each section, but not
    /// across the survey: MNemo numbers restart at 0 in every section.
    pub station_names: Vec<StationName>,
    /// The zero datum whose station is the georeference origin of the local frame
    /// (the project's first resolvable datum, as the app defines it). `None` when
    /// the project has no usable datum. See [`StationPosition`].
    pub frame_datum: Option<StationId>,
    /// Measured (unadjusted) station positions computed by the app's own geometry,
    /// the supported input for drawing a map; `None` unless requested in
    /// [`crate::ReadOptions`].
    pub stations: Option<Vec<StationPosition>>,
    /// Cross-references (merges, datums, closures, comments, names) of an SQLite
    /// file whose station no longer exists in the file. They are dropped, and
    /// counted here. Always 0 for a JSON file, which is refused instead.
    pub unresolved_references: usize,
    /// A JSON file only: set when the file names `requires` features or holds
    /// survey fields this crate does not know (the app would open it
    /// read-only). Everything this crate knows is read, but the file may mean
    /// more than this value shows; upgrade the crate. The text names the
    /// unknown parts and can quote the file: treat it as untrusted.
    pub newer_content: Option<String>,
}

/// The two `.stalx` file formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileFormat {
    /// The SQLite database the app wrote before format version 2. Read through
    /// the app's conversion of old files.
    Sqlite,
    /// Canonical JSON, format version 2 and later.
    Json,
}

/// A station position: `index` 0 is the section's start, index `i` is the end
/// of shot `i - 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct StationId {
    pub section_id: u32,
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Section {
    pub id: u32,
    pub name: Option<String>,
    /// The distinct surveyor names of its shots joined with `", "` (each shot
    /// carries its own credit: [`Shot::surveyors`]).
    pub surveyor: Option<String>,
    /// The earliest shot date, ISO 8601 (`YYYY-MM-DD`).
    pub date: Option<String>,
    pub direction: Direction,
    pub quality: f64,
    /// `#rrggbb`.
    pub color: String,
    /// What the stored azimuths are measured against.
    pub north: North,
    /// Degrees to ADD to a stored azimuth for a true-north bearing (east positive).
    pub azimuth_correction_deg: f64,
    /// Where [`Section::azimuth_correction_deg`] came from. `NotRecorded` means no
    /// reference was ever set for this section or the project: the 0 is not a
    /// verified zero, and the survey is in uncorrected (usually magnetic) north.
    pub azimuth_correction_source: CorrectionSource,
    pub shots: Vec<Shot>,
    /// The Station ID of each station of the section, by index
    /// (`station_ids[i]` is the id of `StationId { section_id: id, index: i }`,
    /// so there is one more than there are shots). A junction station has the
    /// same id on every section through it.
    pub station_ids: Vec<u32>,
}

/// Provenance of a section's azimuth correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CorrectionSource {
    /// No azimuth reference is recorded at all.
    NotRecorded,
    /// A reference is recorded but where its correction came from is not.
    Unknown,
    /// Declared by the imported file (e.g. Compass `DECLINATION`, Walls `DECL=`).
    FileDeclared,
    /// Computed from a geomagnetic model for `epoch_year`.
    Model { epoch_year: i32 },
    /// Entered by hand.
    Manual,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Shot {
    /// Stable identity across revisions. See [`Shot::uuid_derived`].
    pub uuid: String,
    /// `true` when an SQLite file had no usable UUID for this shot (none, the
    /// same one as an earlier shot in the section, or one a lower section
    /// already holds) and `uuid` was derived: deterministic for the same file,
    /// the id the app's conversion of the file gives it, but not a persisted
    /// identity until the app re-saves the file. Never set for a JSON file.
    pub uuid_derived: bool,
    pub kind: ShotKind,
    pub length: f64,
    pub azimuth_in: f64,
    pub azimuth_out: f64,
    pub depth_in: f64,
    pub depth_out: f64,
    pub pitch_in: Option<f64>,
    pub pitch_out: Option<f64>,
    pub left: Option<f64>,
    pub right: Option<f64>,
    pub up: Option<f64>,
    pub down: Option<f64>,
    pub temperature: Option<f64>,
    pub marker_index: Option<u32>,
    pub comment: Option<String>,
    /// Who surveyed this shot, in order. Shared (`Arc`) between the shots that
    /// carry the same credit - typically a whole section's.
    pub surveyors: Arc<[Person]>,
    /// Who explored (first laid line through) this shot, in order. Shared like
    /// [`Shot::surveyors`].
    pub explorers: Arc<[Person]>,
    /// The day this shot was surveyed, ISO 8601 (`YYYY-MM-DD`).
    pub date: Option<String>,
}

/// One person credited on a shot. Names are compared lowercased with
/// whitespace collapsed, emails lowercased; a person without an email is a
/// different person from one with.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Person {
    pub name: String,
    pub email: Option<String>,
}

impl Person {
    fn from_model(person: &stalagmight_models::Person) -> Self {
        Self {
            name: person.name.clone(),
            email: person.email.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShotKind {
    /// A surveyed leg of the cave line.
    Normal,
    /// A swum shortcut between two lines.
    Jump,
    /// A non-swimmable survey tie.
    Reference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Direction {
    In,
    Out,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum North {
    Magnetic,
    True,
    Grid,
}

/// Two positions of one junction station (the same physical point, with one
/// Station ID).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct StationMerge {
    pub a: StationId,
    pub b: StationId,
}

/// A station with a known GPS position.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct ZeroDatum {
    /// The datum's station, at the position of the section it was set on (a
    /// junction's first position when that is not recorded).
    pub station: StationId,
    pub latitude: f64,
    pub longitude: f64,
    /// Depth of the datum station, metres, positive down.
    pub depth: f64,
    /// Horizontal disagreement, metres, between where this datum's station sits
    /// in the local frame and where its GPS says it should be relative to the
    /// frame datum. For a datum that places its piece, `Some(~0)` when
    /// consistent; a large value means the stored placement no longer honours
    /// this datum (e.g. a datum removed and re-added). For a GPS fix (a later
    /// datum in a connected piece that another datum places, #741) it is the
    /// fix's gap to its GPS, which is expected: a fix does not move the
    /// measured survey. The first datum of a connected piece in the file's
    /// datum order places it. `None` unless station positions were requested
    /// and a frame datum exists.
    pub residual_m: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct LoopClosure {
    pub id: u64,
    /// Each end at the position of the section it was made on (a junction's
    /// first position when that is not recorded).
    pub a: StationId,
    pub b: StationId,
    /// A rigid tie the app's conversion made from an old file: the two ends
    /// are one physical point (a merge that closed a loop, joined a section to
    /// itself, or an aligned legacy closure), kept as a closure because one
    /// station cannot represent it.
    pub tie: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StationComment {
    pub station: StationId,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StationName {
    pub station: StationId,
    pub name: String,
}

/// A station's measured (not loop-closure-adjusted) position, computed by the
/// app's own geometry, so a map drawn from these agrees with the app's measured
/// view.
///
/// # Frame
///
/// - Metres. `x` east, `y` north (true north where the section records an
///   azimuth correction, otherwise as surveyed), `z` depth below the surface,
///   positive down.
/// - Every position of a junction has the same `x` and `y`. Legacy per-section
///   manual moves of an old file are applied (`z` can then differ per
///   section).
/// - Horizontal georeference: the [`Survey::frame_datum`] station is the origin.
///   Another station's latitude/longitude is the frame datum's, moved by
///   `(x - frame.x)` metres east and `(y - frame.y)` metres north on the WGS84
///   ellipsoid's local east/north plane at the frame datum, which is how the app
///   placed every other datum relative to it.
/// - Only a station whose [`StationPosition::anchor`] is `Some` sits in a
///   component tied to a datum. A floating component (`anchor: None`) is placed
///   wherever the user put it, so its georeference is not meaningful.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct StationPosition {
    pub station: StationId,
    /// The station's Station ID (shared by every position of a junction).
    pub station_id: u32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// The zero-datum station anchoring this station's connected component
    /// (connected through shots and junctions): the component's placement
    /// datum, its first datum in the project's list order, as the app defines
    /// it. A later datum in the same component is a GPS fix, which never
    /// anchors. The frame datum is the placement datum of its own component.
    /// `None` when the component has no datum.
    pub anchor: Option<StationId>,
    /// WGS84 latitude/longitude in degrees, by the exact inverse of the app's own
    /// datum placement (see "Frame"). `None` for a floating station or when the
    /// project has no frame datum.
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

impl From<StationRef> for StationId {
    fn from(r: StationRef) -> Self {
        Self {
            section_id: r.line_id,
            index: r.station_index,
        }
    }
}

/// What the format path knows about the file besides its survey.
pub(crate) struct ReadMeta<'a> {
    pub(crate) format: FileFormat,
    pub(crate) format_version: u64,
    pub(crate) schema_version: i32,
    /// `(section id, uuid)` of every shot whose uuid the file did not give.
    pub(crate) derived_uuids: &'a HashSet<(u32, String)>,
    pub(crate) unresolved_references: usize,
    pub(crate) newer_content: Option<String>,
}

/// A station's positions, sorted by `(section_id, index)`.
fn positions_of(project: &Project, station: ModelStationId) -> Vec<StationId> {
    let mut out: Vec<StationId> = project
        .topology()
        .slots_of(station)
        .iter()
        .copied()
        .map(StationId::from)
        .collect();
    out.sort_unstable();
    out
}

/// The position the reader reports for something attached to `station` (a
/// datum, a closure end): its position on the `via` section when that section
/// passes the station, else its first position.
fn attached_position(
    project: &Project,
    station: ModelStationId,
    via: Option<LineId>,
) -> Option<StationId> {
    via.and_then(|line| project.topology().slot_on(station, line))
        .map(StationId::from)
        .or_else(|| positions_of(project, station).first().copied())
}

/// How many [`StationMerge`]s a project reads to (what the count cap checks).
pub(crate) fn junction_pair_count(project: &Project) -> usize {
    let topology = project.topology();
    topology
        .stations()
        .iter()
        .map(|&s| topology.slots_of(s).len().saturating_sub(1))
        .sum()
}

fn junction_pairs(project: &Project) -> Vec<StationMerge> {
    let mut out = Vec::new();
    for &station in project.topology().stations() {
        let positions = positions_of(project, station);
        if let Some((&first, rest)) = positions.split_first() {
            out.extend(rest.iter().map(|&b| StationMerge { a: first, b }));
        }
    }
    out.sort_by_key(|m| (m.a, m.b));
    out
}

impl Survey {
    pub(crate) fn from_model(project: &Project, meta: &ReadMeta<'_>, with_stations: bool) -> Self {
        let datum_at = |index: usize| -> Option<StationId> {
            let d = project.datums.get(index)?;
            attached_position(project, d.station, d.via)
        };
        let mut zero_datums: Vec<ZeroDatum> = project
            .datums
            .iter()
            .filter_map(|d| {
                Some(ZeroDatum {
                    station: attached_position(project, d.station, d.via)?,
                    latitude: d.gps.latitude,
                    longitude: d.gps.longitude,
                    depth: d.gps.depth_reference,
                    residual_m: None,
                })
            })
            .collect();
        zero_datums.sort_by_key(|z| z.station);
        let mut loop_closures: Vec<LoopClosure> = project
            .closures
            .iter()
            .filter_map(|c| {
                Some(LoopClosure {
                    id: c.id,
                    a: attached_position(project, c.a, c.a_via)?,
                    b: attached_position(project, c.b, c.b_via)?,
                    tie: c.strategy == ClosureStrategy::Tie,
                })
            })
            .collect();
        loop_closures.sort_by_key(|c| c.id);
        let mut station_comments: Vec<StationComment> = project
            .stations
            .values()
            .filter_map(|station| {
                let text = station.comment.as_ref().filter(|t| !t.is_empty())?;
                Some(StationComment {
                    station: attached_position(project, station.id, None)?,
                    text: text.clone(),
                })
            })
            .collect();
        station_comments.sort_by_key(|c| c.station);
        let mut station_names: Vec<StationName> = Vec::new();
        for line in project.lines() {
            for (index, id) in line.stations().enumerate() {
                let Some(station) = project.station(id).filter(|s| !s.name.is_empty()) else {
                    continue;
                };
                station_names.push(StationName {
                    station: StationId {
                        section_id: line.id,
                        index: index as u32,
                    },
                    name: station.name.clone(),
                });
            }
        }
        station_names.sort_by_key(|n| n.station);
        let frame = project.frame_datum();
        let frame_datum = frame.and_then(datum_at);

        let stations = with_stations.then(|| {
            let raw = project.raw_positions();
            let topology = project.topology();
            let placements: Vec<Option<StationId>> = project
                .placement_datums()
                .into_iter()
                .map(|d| d.and_then(datum_at))
                .collect();
            let mut positions: Vec<StationPosition> = raw
                .slots(project)
                .into_iter()
                .map(|slot| StationPosition {
                    station: StationId::from(slot.slot),
                    station_id: slot.station,
                    x: slot.x,
                    y: slot.y,
                    z: slot.z,
                    anchor: topology
                        .component_of(slot.station)
                        .and_then(|c| placements.get(c).copied().flatten()),
                    latitude: None,
                    longitude: None,
                })
                .collect();
            positions.sort_by_key(|p| p.station);
            let frame_gps = frame.and_then(|i| project.datums.get(i)).map(|d| d.gps);
            georeference(
                &mut positions,
                &mut zero_datums,
                frame_datum,
                frame_gps.as_ref(),
            );
            positions
        });

        let mut people = PeopleCache::new();
        let mut lines: Vec<&stalagmight_models::survey::Line> = project.lines().iter().collect();
        lines.sort_by_key(|l| l.id);
        Self {
            format: meta.format,
            format_version: meta.format_version,
            schema_version: meta.schema_version,
            name: project.name.clone(),
            location: project.location.clone(),
            created_at: project.created_at.to_rfc3339(),
            modified_at: project.modified_at.to_rfc3339(),
            sections: lines
                .into_iter()
                .map(|line| Section::from_model(project, line, meta, &mut people))
                .collect(),
            station_merges: junction_pairs(project),
            zero_datums,
            loop_closures,
            station_comments,
            station_names,
            frame_datum,
            stations,
            unresolved_references: meta.unresolved_references,
            newer_content: meta.newer_content.clone(),
        }
    }
}

/// A shot's credit, empty when it has none.
fn credit_of(shot: &stalagmight_models::survey::Shot) -> &SurveyCredit {
    shot.credit.as_deref().unwrap_or(&EMPTY_CREDIT)
}

impl Section {
    fn from_model(
        project: &Project,
        line: &stalagmight_models::survey::Line,
        meta: &ReadMeta<'_>,
        people: &mut PeopleCache,
    ) -> Self {
        let reference = project.azimuth_reference_for(line);
        let summary = LineCreditSummary::from_credits(line.shots.iter().map(credit_of));
        Self {
            id: line.id,
            name: line.name.clone(),
            surveyor: join_names(&summary.surveyor_people()),
            date: line
                .shots
                .iter()
                .filter_map(|s| credit_of(s).date)
                .min()
                .map(|d| d.format("%Y-%m-%d").to_string()),
            direction: match line.direction {
                SurveyDirection::In => Direction::In,
                SurveyDirection::Out => Direction::Out,
            },
            quality: line.quality,
            color: line.color.clone(),
            north: match reference.north {
                NorthReference::Magnetic => North::Magnetic,
                NorthReference::True => North::True,
                NorthReference::Grid => North::Grid,
            },
            azimuth_correction_deg: reference.correction,
            azimuth_correction_source: correction_source(project, line),
            shots: line
                .shots
                .iter()
                .map(|shot| {
                    let derived = meta.derived_uuids.contains(&(line.id, shot.uuid.clone()));
                    Shot::from_model(shot, derived, people)
                })
                .collect(),
            station_ids: line.stations().collect(),
        }
    }
}

/// Fill each anchored station's latitude/longitude and each datum's residual,
/// both relative to the frame datum. Positions must be sorted by station.
fn georeference(
    positions: &mut [StationPosition],
    datums: &mut [ZeroDatum],
    frame_datum: Option<StationId>,
    frame_gps: Option<&GpsCoordinates>,
) {
    let (Some(frame), Some(frame_gps)) = (frame_datum, frame_gps) else {
        return;
    };
    let at = |positions: &[StationPosition], station: StationId| {
        positions
            .binary_search_by_key(&station, |p| p.station)
            .ok()
            .map(|i| (positions[i].x, positions[i].y))
    };
    let Some((fx, fy)) = at(positions, frame) else {
        return;
    };
    for p in positions.iter_mut() {
        if p.anchor.is_none() {
            continue;
        }
        if let Some((lat, lon)) =
            stalagmight_models::geo::local_to_gps(p.x - fx, p.y - fy, frame_gps)
        {
            p.latitude = Some(lat);
            p.longitude = Some(lon);
        }
    }
    for d in datums.iter_mut() {
        let (Some((x, y)), Ok(gps)) = (
            at(positions, d.station),
            GpsCoordinates::new(d.latitude, d.longitude, d.depth),
        ) else {
            continue;
        };
        let (east, north) = stalagmight_models::geo::gps_to_local(&gps, frame_gps);
        d.residual_m = Some((x - fx - east).hypot(y - fy - north));
    }
}

fn correction_source(
    project: &Project,
    line: &stalagmight_models::survey::Line,
) -> CorrectionSource {
    use stalagmight_models::CorrectionSource as Model;
    if line.azimuth_reference.is_none() && project.settings.default_azimuth_reference.is_none() {
        return CorrectionSource::NotRecorded;
    }
    match project.azimuth_reference_for(line).source {
        Model::Unknown => CorrectionSource::Unknown,
        Model::FileDeclared => CorrectionSource::FileDeclared,
        Model::IgrfAuto { epoch_year } => CorrectionSource::Model { epoch_year },
        Model::Manual => CorrectionSource::Manual,
    }
}

/// The converted people of each model credit, by the credit's allocation: the
/// shots that share one model credit (a section's legacy fallback) share one
/// converted list instead of a copy each.
type PeopleCache = HashMap<usize, (Arc<[Person]>, Arc<[Person]>)>;

fn people_of(
    shot: &stalagmight_models::survey::Shot,
    cache: &mut PeopleCache,
) -> (Arc<[Person]>, Arc<[Person]>) {
    let convert = |list: &[stalagmight_models::Person]| -> Arc<[Person]> {
        list.iter().map(Person::from_model).collect()
    };
    match &shot.credit {
        None => (Arc::from([]), Arc::from([])),
        Some(credit) => {
            let key = Arc::as_ptr(credit) as usize;
            let (surveyors, explorers) = cache
                .entry(key)
                .or_insert_with(|| (convert(&credit.surveyors), convert(&credit.explorers)));
            (Arc::clone(surveyors), Arc::clone(explorers))
        }
    }
}

impl Shot {
    fn from_model(
        shot: &stalagmight_models::survey::Shot,
        uuid_derived: bool,
        people: &mut PeopleCache,
    ) -> Self {
        let (surveyors, explorers) = people_of(shot, people);
        Self {
            uuid: shot.uuid.clone(),
            uuid_derived,
            kind: match shot.kind {
                ModelShotKind::Normal => ShotKind::Normal,
                ModelShotKind::Jump => ShotKind::Jump,
                ModelShotKind::Reference => ShotKind::Reference,
            },
            length: shot.length,
            azimuth_in: shot.azimuth_in,
            azimuth_out: shot.azimuth_out,
            depth_in: shot.depth_in,
            depth_out: shot.depth_out,
            pitch_in: shot.pitch_in,
            pitch_out: shot.pitch_out,
            left: shot.left,
            right: shot.right,
            up: shot.up,
            down: shot.down,
            temperature: shot.temperature,
            marker_index: shot.marker_index,
            comment: shot.comment.clone(),
            surveyors,
            explorers,
            date: credit_of(shot)
                .date
                .map(|d| d.format("%Y-%m-%d").to_string()),
        }
    }
}
