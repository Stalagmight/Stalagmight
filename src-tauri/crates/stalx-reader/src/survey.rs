//! The reader's public survey model and its conversion from the app's `Project`.
//!
//! These types are deliberately separate from `stalagmight_models`: the app's
//! model changes often, and every change there must not become a breaking
//! change for a consumer. Each struct is `#[non_exhaustive]` so fields can be
//! added in a minor release.

use std::collections::HashMap;

use stalagmight_models::station::{StationKey, StationRef};
use stalagmight_models::{NorthReference, Project, ShotKind as ModelShotKind, SurveyDirection};

/// One project, read from a `.stalx` file.
///
/// Conventions, for every field below:
/// - Lengths and depths are metres. Angles are degrees.
/// - Depth and pitch are positive DOWN.
/// - Azimuths are as stored; add the section's [`Section::azimuth_correction_deg`]
///   to get a true-north bearing.
///
/// # Identity
///
/// - [`Section::id`] is stable across edits for the life of the project.
/// - [`StationId::index`] is NOT stable: inserting or deleting a shot shifts the
///   indexes after it, and reversing a section renumbers station `i` to `N - i`.
///   Key anything persistent on `(section id, shot uuid)`, never on an index.
/// - [`Shot::uuid`] is stable across edits and unique across the whole survey.
///   A shot the file gives no usable UUID (files from before shot UUIDs, a second
///   shot repeating one in its section, or a section repeating an id an earlier
///   section holds - the same Ariane file imported twice, a merge of two copies)
///   gets one derived deterministically, the same one the app's open gives it,
///   and is flagged [`Shot::uuid_derived`].
///
/// # Ordering
///
/// Every list is sorted, so the same file content always reads to the same
/// value: sections by id, shots in
/// survey order, merges by `(a, b)`, zero datums, comments, names and station
/// positions by station, loop closures by id. [`Survey::modified_at`] changes
/// on every save; leave it out of any content hash.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Survey {
    /// The file's schema version as uploaded, read before the private copy is
    /// migrated forward (`PRAGMA user_version`, or the project row's own version
    /// for files that predate the pragma; 0 if neither is recorded).
    pub schema_version: i32,
    pub name: String,
    pub location: Option<String>,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub modified_at: String,
    pub sections: Vec<Section>,
    pub station_merges: Vec<StationMerge>,
    pub zero_datums: Vec<ZeroDatum>,
    pub loop_closures: Vec<LoopClosure>,
    pub station_comments: Vec<StationComment>,
    /// Station names, e.g. the mNemo station number a DMP import recorded. Not
    /// unique: mNemo numbers restart at 0 in every section.
    pub station_names: Vec<StationName>,
    /// The zero datum whose station is the georeference origin of the local frame
    /// (the project's first resolvable datum, as the app defines it). `None` when
    /// the project has no usable datum. See [`StationPosition`].
    pub frame_datum: Option<StationId>,
    /// Measured (unadjusted) station positions computed by the app's own geometry,
    /// the supported input for drawing a map; `None` unless requested in
    /// [`crate::ReadOptions`].
    pub stations: Option<Vec<StationPosition>>,
    /// Cross-references (merges, datums, closures, comments, names) whose station
    /// no longer exists in the file. They are dropped, and counted here.
    pub unresolved_references: usize,
}

/// A station: `index` 0 is the section's start, index `i` is the end of shot `i - 1`.
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
    pub surveyor: Option<String>,
    /// ISO 8601 date (`YYYY-MM-DD`).
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
    /// `true` when the file had no usable UUID for this shot (none, the same one
    /// as an earlier shot in the section, or one a lower section already holds)
    /// and `uuid` was derived: deterministic for the same file, but not a
    /// persisted identity until the app re-saves the file. A repeat across
    /// sections derives the id the app gives it, so it survives that save.
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

/// Two stations that are the same physical point.
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
    pub station: StationId,
    pub latitude: f64,
    pub longitude: f64,
    /// Depth of the datum station, metres, positive down.
    pub depth: f64,
    /// Horizontal disagreement, metres, between where this datum's station sits
    /// in the local frame and where its GPS says it should be relative to the
    /// frame datum. `Some(~0)` when consistent; a large value means the stored
    /// offsets no longer honour this datum (e.g. a datum removed and re-added).
    /// `None` unless station positions were requested and a frame datum exists.
    pub residual_m: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct LoopClosure {
    pub id: u64,
    pub a: StationId,
    pub b: StationId,
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
/// - Section and station offsets (manual moves) are applied.
/// - Horizontal georeference: the [`Survey::frame_datum`] station is the origin.
///   Another station's latitude/longitude is the frame datum's, moved by
///   `(x - frame.x)` metres east and `(y - frame.y)` metres north on the WGS84
///   ellipsoid's local east/north plane at the frame datum, which is how the app
///   placed every other datum relative to it.
/// - Only a station whose [`StationPosition::anchor`] is `Some` sits in a
///   component tied to a datum. A floating component (`anchor: None`) is placed
///   wherever the user's offsets put it, so its georeference is not meaningful.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct StationPosition {
    pub station: StationId,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// The zero-datum station anchoring this station's connected component
    /// (connected through shots and station merges): the frame datum when it is
    /// in the component, otherwise the component's lowest datum station. `None`
    /// when the component has no datum.
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
            section_id: r.section_id,
            index: r.station_index,
        }
    }
}

impl Survey {
    pub(crate) fn from_project(
        project: &Project,
        schema_version: i32,
        with_stations: bool,
        derived_uuids: &std::collections::HashSet<(u32, String)>,
    ) -> Self {
        let mut unresolved = 0;
        let mut resolve = |key: &StationKey| {
            let id = project.resolve_key(key).map(StationId::from);
            if id.is_none() {
                unresolved += 1;
            }
            id
        };

        let mut station_merges: Vec<StationMerge> = project
            .station_merges
            .iter()
            .filter_map(|m| {
                Some(StationMerge {
                    a: resolve(&m.station_a)?,
                    b: resolve(&m.station_b)?,
                })
            })
            .collect();
        station_merges.sort_by_key(|m| (m.a, m.b));
        let mut zero_datums: Vec<ZeroDatum> = project
            .zero_datums
            .iter()
            .filter_map(|z| {
                Some(ZeroDatum {
                    station: resolve(&z.station_ref)?,
                    latitude: z.gps.latitude,
                    longitude: z.gps.longitude,
                    depth: z.gps.depth_reference,
                    residual_m: None,
                })
            })
            .collect();
        zero_datums.sort_by_key(|z| z.station);
        let mut loop_closures: Vec<LoopClosure> = project
            .loop_closures
            .iter()
            .filter_map(|c| {
                Some(LoopClosure {
                    id: c.id,
                    a: resolve(&c.station_a)?,
                    b: resolve(&c.station_b)?,
                })
            })
            .collect();
        loop_closures.sort_by_key(|c| c.id);
        let mut station_comments: Vec<StationComment> = project
            .station_comments
            .iter()
            .filter_map(|(key, text)| {
                Some(StationComment {
                    station: resolve(key)?,
                    text: text.clone(),
                })
            })
            .collect();
        station_comments.sort_by_key(|c| c.station);
        let mut station_names: Vec<StationName> = project
            .station_names
            .iter()
            .filter_map(|(key, name)| {
                Some(StationName {
                    station: resolve(key)?,
                    name: name.clone(),
                })
            })
            .collect();
        station_names.sort_by_key(|n| n.station);
        let frame_datum = project
            .frame_datum_key()
            .and_then(|key| project.resolve_key(&key))
            .map(StationId::from);

        let frame_gps = project.frame_datum().map(|d| d.gps);
        let stations = with_stations.then(|| {
            let anchors = Anchors::new(project, &station_merges, &zero_datums, frame_datum);
            let mut positions: Vec<StationPosition> = project
                .compute_station_positions()
                .into_iter()
                .map(|(station_ref, coordinates)| {
                    let station = StationId::from(station_ref);
                    StationPosition {
                        station,
                        x: coordinates.x,
                        y: coordinates.y,
                        z: coordinates.z,
                        anchor: anchors.of(station),
                        latitude: None,
                        longitude: None,
                    }
                })
                .collect();
            positions.sort_by_key(|p| p.station);
            georeference(
                &mut positions,
                &mut zero_datums,
                frame_datum,
                frame_gps.as_ref(),
            );
            positions
        });

        let mut sections: Vec<&stalagmight_models::Section> = project.sections.iter().collect();
        sections.sort_by_key(|s| s.id);
        Self {
            schema_version,
            name: project.name.clone(),
            location: project.location.clone(),
            created_at: project.created_at.to_rfc3339(),
            modified_at: project.modified_at.to_rfc3339(),
            sections: sections
                .into_iter()
                .map(|section| {
                    let reference = project.azimuth_reference_for(section);
                    Section {
                        id: section.id,
                        name: section.name.clone(),
                        surveyor: section.surveyor.clone(),
                        date: section.date.map(|d| d.format("%Y-%m-%d").to_string()),
                        direction: match section.direction {
                            SurveyDirection::In => Direction::In,
                            SurveyDirection::Out => Direction::Out,
                        },
                        quality: section.quality,
                        color: section.color.clone(),
                        north: match reference.north {
                            NorthReference::Magnetic => North::Magnetic,
                            NorthReference::True => North::True,
                            NorthReference::Grid => North::Grid,
                        },
                        azimuth_correction_deg: reference.correction,
                        azimuth_correction_source: correction_source(project, section),
                        shots: section
                            .shots
                            .iter()
                            .map(|shot| {
                                let derived = shot.uuid.as_ref().is_some_and(|uuid| {
                                    derived_uuids.contains(&(section.id, uuid.clone()))
                                });
                                Shot::from_model(shot, derived)
                            })
                            .collect(),
                    }
                })
                .collect(),
            station_merges,
            zero_datums,
            loop_closures,
            station_comments,
            station_names,
            frame_datum,
            stations,
            unresolved_references: unresolved,
        }
    }
}

/// Fill each anchored station's latitude/longitude and each datum's residual,
/// both relative to the frame datum. Positions must be sorted by station.
fn georeference(
    positions: &mut [StationPosition],
    datums: &mut [ZeroDatum],
    frame_datum: Option<StationId>,
    frame_gps: Option<&stalagmight_models::station::GpsCoordinates>,
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
            stalagmight_models::station::GpsCoordinates::new(d.latitude, d.longitude, d.depth),
        ) else {
            continue;
        };
        let (east, north) = stalagmight_models::geo::gps_to_local(&gps, frame_gps);
        d.residual_m = Some((x - fx - east).hypot(y - fy - north));
    }
}

fn correction_source(project: &Project, section: &stalagmight_models::Section) -> CorrectionSource {
    use stalagmight_models::CorrectionSource as Model;
    if section.azimuth_reference.is_none() && project.default_azimuth_reference.is_none() {
        return CorrectionSource::NotRecorded;
    }
    match project.azimuth_reference_for(section).source {
        Model::Unknown => CorrectionSource::Unknown,
        Model::FileDeclared => CorrectionSource::FileDeclared,
        Model::IgrfAuto { epoch_year } => CorrectionSource::Model { epoch_year },
        Model::Manual => CorrectionSource::Manual,
    }
}

/// Which zero datum anchors each station's component (connected through shots
/// and station merges). Union-find with path halving and union by size.
struct Anchors {
    index: HashMap<StationId, usize>,
    root: Vec<usize>,
    anchor_of_root: HashMap<usize, StationId>,
}

impl Anchors {
    fn new(
        project: &Project,
        merges: &[StationMerge],
        datums: &[ZeroDatum],
        frame_datum: Option<StationId>,
    ) -> Self {
        let mut index = HashMap::new();
        for section in &project.sections {
            for i in 0..=section.shots.len() as u32 {
                let next = index.len();
                index.insert(
                    StationId {
                        section_id: section.id,
                        index: i,
                    },
                    next,
                );
            }
        }
        let mut uf = UnionFind::new(index.len());
        for section in &project.sections {
            for i in 0..section.shots.len() as u32 {
                let at = |index_: u32| {
                    index.get(&StationId {
                        section_id: section.id,
                        index: index_,
                    })
                };
                if let (Some(&a), Some(&b)) = (at(i), at(i + 1)) {
                    uf.union(a, b);
                }
            }
        }
        for m in merges {
            if let (Some(&a), Some(&b)) = (index.get(&m.a), index.get(&m.b)) {
                uf.union(a, b);
            }
        }
        // `datums` is sorted by station, so the first datum seen in a component is
        // its lowest; the frame datum then overrides it in its own component.
        let mut anchor_of_root = HashMap::new();
        for d in datums {
            if let Some(&i) = index.get(&d.station) {
                anchor_of_root.entry(uf.find(i)).or_insert(d.station);
            }
        }
        if let Some(frame) = frame_datum {
            if let Some(&i) = index.get(&frame) {
                anchor_of_root.insert(uf.find(i), frame);
            }
        }
        let root = (0..index.len()).map(|i| uf.find(i)).collect();
        Self {
            index,
            root,
            anchor_of_root,
        }
    }

    fn of(&self, station: StationId) -> Option<StationId> {
        let i = *self.index.get(&station)?;
        self.anchor_of_root.get(&self.root[i]).copied()
    }
}

struct UnionFind {
    parent: Vec<usize>,
    size: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            size: vec![1; n],
        }
    }

    fn find(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }

    fn union(&mut self, a: usize, b: usize) {
        let (mut a, mut b) = (self.find(a), self.find(b));
        if a == b {
            return;
        }
        if self.size[a] < self.size[b] {
            std::mem::swap(&mut a, &mut b);
        }
        self.parent[b] = a;
        self.size[a] += self.size[b];
    }
}

impl Shot {
    fn from_model(shot: &stalagmight_models::Shot, uuid_derived: bool) -> Self {
        Self {
            // The reader fills every missing UUID before converting.
            uuid: shot.uuid.clone().unwrap_or_default(),
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
        }
    }
}
