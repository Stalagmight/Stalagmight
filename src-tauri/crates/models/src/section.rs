use chrono::NaiveDate;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
use ts_rs::TS;

use super::{
    legacy_people, AzimuthReference, Coordinates, GpsCoordinates, Person, SectionCreditSummary,
    Shot, ShotData, ShotId, ShotKind, ShotUpdates, StationRef, SurveyCredit, FEATURE_ID_STRIDE,
};

/// Unique identifier for a section
pub type SectionId = u32;

/// Survey direction (In = going into cave, Out = returning)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum SurveyDirection {
    #[default]
    In,
    Out,
}

impl fmt::Display for SurveyDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SurveyDirection::In => write!(f, "in"),
            SurveyDirection::Out => write!(f, "out"),
        }
    }
}

impl FromStr for SurveyDirection {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "in" => Ok(SurveyDirection::In),
            "out" => Ok(SurveyDirection::Out),
            _ => Err(format!("Invalid direction: {s}")),
        }
    }
}

/// A group of consecutive shots collected during a single survey session
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Section {
    /// Internal immutable ID
    pub id: SectionId,
    /// Optional human-readable name
    pub name: Option<String>,
    /// LEGACY: the free-text surveyor a section carried before the survey
    /// credit moved onto its shots. Absent from the TS shape. An old project
    /// snapshot or undo payload that still carries `"surveyor"` deserializes
    /// into it, and [`Section::absorb_legacy_credit`] moves it onto the shots.
    ///
    /// A section WITH NO SHOTS keeps it: there is no shot to carry it, so it
    /// stays the section's own (written to `sections.surveyor`, serialized in
    /// undo snapshots) until the first shot arrives and takes it as its credit
    /// (`AppState::create_shot`). Only such a section ever holds one, so the key
    /// is on the wire only for a shot-less section. Read who surveyed a section
    /// with [`Section::credit_summary`].
    #[serde(default, rename = "surveyor", skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub legacy_surveyor: Option<String>,
    /// LEGACY: the section-level survey date, kept and absorbed like
    /// [`Section::legacy_surveyor`].
    #[serde(default, rename = "date", skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub legacy_date: Option<NaiveDate>,
    /// Display color (hex format, e.g., "#4ecdc4")
    pub color: String,
    /// Survey direction (In = entering cave, Out = exiting)
    pub direction: SurveyDirection,
    /// Quality factor (1.0 = default). Used for weighted adjustments.
    #[serde(default = "default_section_quality")]
    pub quality: f64,
    /// Shots in this section, in order
    pub shots: Vec<Shot>,
    /// Next shot ID to assign
    pub next_shot_id: ShotId,
    /// Flips every time this section's shot order is REVERSED (#24). Not a count and not
    /// meaningful on its own - only whether it still matches what something else recorded.
    ///
    /// Exists so the least-squares gauge can stay on one PHYSICAL station. That gauge has
    /// always been "station 0 of the lowest-id section in the component", and a reverse turns
    /// station 0 into station N - pinning the opposite end, which translates the whole adjusted
    /// component and breaks the reverse's "every station keeps its position" guarantee exactly
    /// where closure-adjusted geometry is forced (Plan/Export). Reading the end off this flag
    /// keeps the original choice: `false` means station 0, as every existing project has it.
    #[serde(default)]
    pub reverse_parity: bool,
    /// The least-squares gauge station for this section, as the pair of shots that identify it:
    /// the station is the end of `gauge_shot_uuid` FACING AWAY FROM `gauge_neighbour_uuid`
    /// (#24). `None` until the first save captures it.
    ///
    /// The gauge fixes the otherwise-free absolute translation of a datum-less component by
    /// pinning one station at its measured coordinates, so it has to name the SAME physical
    /// station for the life of the project - if it moves, the whole component translates by the
    /// adjustment residual and Plan/Export coordinates jump.
    ///
    /// An index cannot do that. "Station 0, or `shots.len()` when the section is reversed" is
    /// stable across the reverse itself but not across what comes next: appending to a reversed
    /// section grows `len` and walks the gauge onto the newly created far station. A pair is
    /// immune - neither shot moves relative to the other when the section is extended at either
    /// end, and a reverse carries both together, so "away from the neighbour" names the same
    /// junction throughout.
    #[serde(default)]
    pub gauge_shot_uuid: Option<String>,
    /// The shot on the far side of [`Self::gauge_shot_uuid`], giving the gauge station its
    /// direction. `None` for a single-shot section, which has no second shot to face away from.
    #[serde(default)]
    pub gauge_neighbour_uuid: Option<String>,
    /// How this section's stored azimuths relate to true north (#438).
    ///
    /// `None` = inherit [`Project::default_azimuth_reference`], which is itself
    /// `None` on every project that predates this field - and an absent
    /// reference means [`AzimuthReference::default`], "nothing recorded", whose
    /// correction is 0. So the field is additive in the strong sense: a project
    /// that never saw it computes exactly the geometry it always did.
    ///
    /// It lives on the SECTION, not the project, because that is the finest
    /// granularity the source formats have: one Compass file can declare a
    /// different declination per survey and can switch mode mid-file, and a
    /// project routinely mixes an Ariane import (magnetic) with a Compass one.
    ///
    /// [`Project::default_azimuth_reference`]: crate::models::Project::default_azimuth_reference
    ///
    /// `#[ts(optional)]`, like every other additive field the frontend does not
    /// construct: the payload always carries it, but a TS `Section` literal
    /// built before #438 stays valid.
    #[serde(default)]
    #[ts(optional)]
    pub azimuth_reference: Option<AzimuthReference>,
}

/// Everything about a section that [`SectionData`] cannot carry: the identity of
/// its shots, and the two flags that decide where its least-squares gauge sits.
///
/// `SectionData` is the CREATE shape - what the user typed - so it holds no
/// uuid, no [`Section::reverse_parity`] and neither gauge anchor. That makes it
/// lossy for RESTORE: rebuilding a section from it alone mints a fresh uuid per
/// shot (dangling every `StationKey::AfterShot` anchor and the Ariane export
/// identity) and resets the gauge pair a save captured (issue #413, finding 1).
/// An undo entry that re-creates a section carries this alongside the data, and
/// [`Section::restore_with_identity`] puts both back.
///
/// FORMAT EVOLUTION (additive, no schema bump): an entry written before this
/// existed deserializes to [`SectionIdentity::default`] - no uuids, parity
/// false, no anchors - which is exactly the pre-fix behaviour (mint fresh),
/// mirroring how `restore_shot`'s `uuid: None` is its own legacy fallback.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SectionIdentity {
    /// The shots' persistent UUIDs, in shot order. An entry per shot, `None`
    /// where the shot genuinely had none (a pre-v6 file loaded with a NULL uuid
    /// column). SHORTER than the section's shots - empty, for a legacy entry -
    /// means the rest mint fresh.
    #[serde(default)]
    pub shot_uuids: Vec<Option<String>>,
    #[serde(default)]
    pub reverse_parity: bool,
    #[serde(default)]
    pub gauge_shot_uuid: Option<String>,
    #[serde(default)]
    pub gauge_neighbour_uuid: Option<String>,
    /// The section's north reference (#438). [`SectionData`] cannot carry it, so
    /// without this an undo that re-creates a section would silently drop the
    /// declination and rotate that section by it. `None` on a legacy entry means
    /// "inherit the project default", which is what a restored section would
    /// have had anyway before the field existed.
    #[serde(default)]
    pub azimuth_reference: Option<AzimuthReference>,
}

impl SectionIdentity {
    /// The identity `section` carries right now.
    pub fn of(section: &Section) -> Self {
        Self {
            shot_uuids: section.shots.iter().map(|shot| shot.uuid.clone()).collect(),
            reverse_parity: section.reverse_parity,
            gauge_shot_uuid: section.gauge_shot_uuid.clone(),
            gauge_neighbour_uuid: section.gauge_neighbour_uuid.clone(),
            azimuth_reference: section.azimuth_reference,
        }
    }
}

/// Data for creating a new section
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SectionData {
    pub name: Option<String>,
    /// LEGACY template: free text split by [`legacy_people`] and given, with
    /// [`Self::date`], to every shot whose credit is empty when the section is
    /// built ([`Section::new`]). The section itself keeps no surveyor.
    pub surveyor: Option<String>,
    /// LEGACY template date, applied with [`Self::surveyor`].
    #[ts(type = "string | null")]
    pub date: Option<NaiveDate>,
    pub color: String,
    /// Survey direction (defaults to In if not specified)
    #[serde(default)]
    pub direction: Option<SurveyDirection>,
    /// Quality factor (1.0 = default). Used for weighted adjustments.
    #[serde(default = "default_section_quality")]
    pub quality: f64,
    pub shots: Vec<ShotData>,
}

/// Deserialize an `Option<Option<T>>` field so JSON `null` means "clear":
/// absent -> `None` (no update), `null` -> `Some(None)`, value -> `Some(Some(v))`.
/// With plain `#[serde(default)]`, `null` folds into the outer `None` and a
/// clear sent by the frontend becomes a silent no-op.
///
/// A local twin of the identical helper in `shot.rs`, which is private to that
/// module; duplicated rather than widening its visibility.
fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Deserialize::deserialize(de).map(Some)
}

/// Partial updates to a section
///
/// The three nullable fields use `double_option` + `skip_serializing_if` so the
/// JSON round-trip is lossless: absent <-> outer `None`, `null` <-> `Some(None)`.
/// Undo relies on this - previous values are stored as a serialized
/// `SectionUpdates` (`values_json`), so an un-set field must serialize as absent,
/// not `null`. Without it (issue #413 finding 5) a capture of `Some(None)` - "this
/// section had no surveyor, restore that" - came back as `None` and the undo left
/// the new value in place. `ShotUpdates` has carried the same pair since v7.
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
pub struct SectionUpdates {
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
    pub name: Option<Option<String>>,
    /// LEGACY: set on EVERY shot of the section - the surveyors (and, for the
    /// Ariane `"explorers / surveyors"` form, the explorers) parsed by
    /// [`legacy_people`]. `null` clears every shot's surveyors.
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
    pub surveyor: Option<Option<String>>,
    /// LEGACY: set on EVERY shot of the section; `null` clears every shot's date.
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(type = "string | null")]
    #[ts(optional)]
    pub date: Option<Option<NaiveDate>>,
    #[serde(default)]
    #[ts(optional)]
    pub color: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub direction: Option<SurveyDirection>,
    #[serde(default)]
    #[ts(optional)]
    pub quality: Option<f64>,
}

/// Section statistics (computed, not stored)
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SectionStats {
    pub shot_count: usize,
    /// Surveyed length: the sum of every shot's length except
    /// [`ShotKind::Reference`]. A Reference shot is a survey tie duplicating
    /// passage already surveyed, so counting it would double-count length
    /// (#689). [`ShotKind::Jump`] counts: it is a swum, surveyed shortcut
    /// (#27 q7). This is the one length every readout shows.
    pub total_length: f64,
    /// Count of [`ShotKind::Jump`] shots.
    pub jump_shot_count: usize,
    /// Count of [`ShotKind::Reference`] shots. Before #27 this counted every
    /// virtual shot; it is now the Reference half only, with jumps in
    /// [`jump_shot_count`](Self::jump_shot_count).
    pub reference_shot_count: usize,
}

/// Shot-line geometry for a single section within a segment.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct SegmentSectionGeometry {
    pub section_id: SectionId,
    pub color: String,
    /// Shot lines as pairs of [x, y] coords in meters, relative to the anchor section's station 0
    pub shot_lines: Vec<[[f64; 2]; 2]>,
}

/// Shot-line geometry for all sections in a segment, positioned relative to a given section.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct SegmentGeometry {
    pub sections: Vec<SegmentSectionGeometry>,
}

/// Configuration for a single section being imported via the batch import command.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ImportSectionConfig {
    /// Index of this section in the import array (used for fromImporting references)
    pub section_index: u32,
    pub name: Option<String>,
    /// LEGACY template, as [`SectionData::surveyor`]: given to every imported
    /// shot whose credit is empty.
    pub surveyor: Option<String>,
    /// LEGACY template date, as [`SectionData::date`].
    #[ts(type = "string | null")]
    pub date: Option<NaiveDate>,
    /// When set, REPLACES the surveyors of every imported shot (the import
    /// dialog's Surveyors field). `None` keeps what the file said.
    #[serde(default)]
    #[ts(optional)]
    pub surveyors: Option<Vec<Person>>,
    /// When set, replaces the explorers of every imported shot.
    #[serde(default)]
    #[ts(optional)]
    pub explorers: Option<Vec<Person>>,
    /// When set, replaces the survey date of every imported shot.
    #[serde(default)]
    #[ts(optional)]
    #[ts(type = "string")]
    pub override_date: Option<NaiveDate>,
    pub color: String,
    #[serde(default)]
    pub direction: Option<SurveyDirection>,
    /// Quality factor (1.0 = default). Used for weighted adjustments.
    #[serde(default = "default_section_quality")]
    pub quality: f64,
    pub shots: Vec<ShotData>,
    pub placement: ImportPlacement,
    /// Station comments to apply after section creation.
    /// Each tuple is (station_index, comment_text).
    #[serde(default)]
    pub station_comments: Vec<(u32, String)>,
    /// Station names to apply after section creation, as (station_index, name),
    /// such as the mNemo station numbers of a DMP import. Same index convention
    /// as `station_comments` (see `reverse` below).
    #[serde(default)]
    pub station_names: Vec<(u32, String)>,
    /// Reverse this section end-to-end as it is imported (#24), so the file's
    /// last station becomes station 0.
    ///
    /// A one-shot transform, not persisted state: the backend reverses the
    /// shots *before* creating the section.
    ///
    /// Index conventions, which differ by field:
    /// * `station_comments` and `station_names` indices are given in the FILE's
    ///   original order - the backend mirrors them itself
    ///   (`execute_import_sections`), so an importer never has to think about it.
    /// * `placement` indices must already address the final (reversed) order.
    ///   Placement is a graph of cross-section references the wizard resolves
    ///   while the user is looking at the original numbering, so only the caller
    ///   knows which side of a connection this batch renumbers; see
    ///   `src/utils/reversePlacement.ts`.
    ///
    /// Left/right passage dimensions are NOT swapped on the import path - the
    /// wizard has no dialog moment to ask, and mirroring LRUD without asking
    /// would silently rewrite measurements.
    #[serde(default)]
    pub reverse: bool,
    /// The north reference this section's readings were taken against (#438),
    /// as declared by the source file. `None` - what the frontend wizards send,
    /// since none of them asks the user - leaves the section inheriting the
    /// project default, i.e. no correction.
    ///
    /// It rides on the CONFIG rather than on `SectionData` because it is
    /// per-section provenance the importer knows and the create shape does not
    /// carry; `execute_import_sections` writes it onto the section as soon as
    /// the section exists.
    #[serde(default)]
    #[ts(optional)]
    pub azimuth_reference: Option<AzimuthReference>,
}

/// Update to apply to an existing section's shot during batch import.
/// Used for overlap interpolation: when an import section overlaps with an
/// existing project section, the overlap shots on the existing side get
/// updated with interpolated values as part of the same atomic transaction.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ExistingShotUpdate {
    pub section_id: SectionId,
    pub shot_id: ShotId,
    pub updates: ShotUpdates,
}

/// Placement strategy for an imported section.
///
/// `export_to` without `export`: the file itself is still written only by the
/// gen_ipc_types example (which takes the file name), but the path must use the
/// member-crate base so `stalagmight-parsers`' export test for
/// `GpxSectionPlacement` can compute the sibling `./ImportPlacement` import -
/// ts-rs diffs the two `EXPORT_TO` strings, and mixing the root-crate
/// `bindings/` base with the member `../../bindings/` base makes that diff
/// unrepresentable (the export test panics).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(
    rename_all = "snake_case",
    export_to = "../../bindings/ImportPlacement.ts"
)]
pub enum ImportPlacement {
    /// Place at origin (0, 0) - no additional operations needed.
    Origin,
    /// Set a GPS zero datum on a specific station.
    ZeroDatum {
        coordinates: GpsCoordinates,
        station_index: u32,
    },
    /// Merge a station with an existing project station.
    FromExisting {
        import_station: u32,
        existing_station: StationRef,
    },
    /// Merge a station with a station in another section being imported in the same batch.
    FromImporting {
        import_station: u32,
        target_section_index: u32,
        target_station_index: u32,
    },
    /// Place at a manual offset (pre-computed by the frontend).
    Manual {
        align_station_index: u32,
        #[serde(default)]
        offset: Option<Coordinates>,
    },
    /// Merge a station with an existing station (manual placement with snap).
    ManualSnap {
        align_station_index: u32,
        snapped_to_station: StationRef,
    },
}

/// A loop closure to create during import, referencing sections by their import index.
#[derive(Debug, Clone)]
pub struct ImportClosure {
    pub from_section_index: u32,
    pub from_station_index: u32,
    pub to_section_index: u32,
    pub to_station_index: u32,
}

impl Default for SectionData {
    fn default() -> Self {
        Self {
            name: None,
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            quality: default_section_quality(),
            shots: Vec::new(),
        }
    }
}

fn default_section_quality() -> f64 {
    1.0
}

impl Section {
    /// Create a new section from section data
    pub fn new(id: SectionId, data: SectionData) -> Self {
        let template = std::sync::Arc::new(SurveyCredit::from_legacy(
            data.surveyor.as_deref(),
            data.date,
        ));
        // A shot-less section keeps the legacy pair: no shot can carry it yet.
        let shotless = data.shots.is_empty();
        let mut section = Self {
            id,
            name: data.name,
            legacy_surveyor: data.surveyor.filter(|_| shotless),
            legacy_date: data.date.filter(|_| shotless),
            color: data.color,
            direction: data.direction.unwrap_or_default(),
            quality: if data.quality.is_finite() && data.quality > 0.0 {
                data.quality
            } else {
                default_section_quality()
            },
            shots: Vec::new(),
            next_shot_id: id * FEATURE_ID_STRIDE,
            reverse_parity: false,
            gauge_shot_uuid: None,
            gauge_neighbour_uuid: None,
            azimuth_reference: None,
        };

        // In-section UUID uniqueness, enforced where a `Vec<ShotData>` becomes
        // shots. This is the only door for a whole shot VECTOR - a TML import, a
        // section split, a project merge and a section-level undo restore all
        // come through here. It is NOT the only door into a section: the
        // single-shot inserts ([`Self::add_shot`],
        // [`Self::insert_shot_after_with_id`], [`Self::insert_shot_before_with_id`])
        // and the per-shot undo restore (`core::state::undo_restore`) build a
        // `Shot` and push it directly, bypassing this loop.
        //
        // What keeps those safe is a property of this function, not of them: a
        // FOREIGN id can only enter a section HERE, so every identity they can
        // introduce is either a fresh v4 (no collision possible) or one this
        // section already de-duped. The IPC `create_shot` clears `uuid` at the
        // boundary so a caller cannot smuggle one in, and `restore_shot` asserts
        // the identity it replays is absent before inserting it. A NEW path that
        // admits a foreign id one shot at a time would break that property and
        // needs its own answer - do not read this as blanket cover.
        //
        // `ShotData::uuid` can now carry a foreign id (#423), and a source file
        // is free to repeat one inside a section. A repeat makes `resolve_key`
        // mis-resolve: it takes the FIRST match, so a merge/closure/datum
        // anchored on the second station silently snaps to the first. Repairing
        // here rather than after the fact means the duplicate never exists in
        // the project, so no cross-reference can be built against it in the
        // window between creation and repair.
        //
        // Policy matches `Project::ensure_unique_shot_uuids` (the pre-save
        // backstop): the FIRST occurrence keeps the id, later ones are cleared
        // and get a fresh mint from `Shot::new`. A repeat ACROSS sections is not
        // visible from here; every door that creates sections from foreign ids
        // settles it once the sections exist
        // (`Project::reassign_repeated_shot_uuids`).
        let mut seen: HashSet<String> = HashSet::new();
        let mut repaired = 0usize;
        for mut shot_data in data.shots {
            if let Some(u) = shot_data.uuid.as_ref() {
                if u.is_empty() || !seen.insert(u.clone()) {
                    shot_data.uuid = None;
                    repaired += 1;
                }
            }
            // The legacy section-level surveyor/date is a template for the
            // shots that carry no credit of their own, SHARED by all of them.
            let inherits = shot_data.credit().is_empty();
            section.add_shot(shot_data);
            if inherits {
                if let Some(shot) = section.shots.last_mut() {
                    shot.set_shared_credit(&template);
                }
            }
        }
        if repaired > 0 {
            // Losing a foreign identity is exactly the kind of thing that is
            // invisible until someone asks why a round trip changed ids.
            tracing::warn!(
                section_id = id,
                repaired,
                purpose = "in-section shot uuid uniqueness (#423)",
                "Section carried repeated (or empty) shot UUIDs; kept the first \
                 occurrence and minted fresh identities for the rest"
            );
        }

        section
    }

    /// Create a new section with a specific ID (for undo restoration)
    pub fn new_with_id(id: SectionId, data: SectionData) -> Self {
        Self::new(id, data)
    }

    /// Restore a section with a specific ID *and* the identity
    /// [`SectionData`] cannot carry (issue #413, finding 1).
    ///
    /// [`Self::new`] mints a fresh uuid per shot and hardcodes
    /// `reverse_parity: false` with both gauge anchors `None`, which is right
    /// for a section being CREATED and wrong for one being PUT BACK: the uuid is
    /// what `StationKey::AfterShot` anchors and the Ariane export resolve
    /// through, and the gauge pair pins the component's absolute position.
    ///
    /// A shot with no recorded uuid (`identity.shot_uuids` shorter than the
    /// section, i.e. a legacy entry) keeps the fresh mint - the pre-fix
    /// behaviour, and the same fallback `restore_shot` takes.
    pub fn restore_with_identity(
        id: SectionId,
        data: SectionData,
        identity: &SectionIdentity,
    ) -> Self {
        let mut section = Self::new(id, data);
        for (shot, uuid) in section.shots.iter_mut().zip(identity.shot_uuids.iter()) {
            shot.uuid.clone_from(uuid);
        }
        section.reverse_parity = identity.reverse_parity;
        section
            .gauge_shot_uuid
            .clone_from(&identity.gauge_shot_uuid);
        section
            .gauge_neighbour_uuid
            .clone_from(&identity.gauge_neighbour_uuid);
        section.azimuth_reference = identity.azimuth_reference;
        section
    }

    /// Convert to SectionData (for undo snapshots)
    pub fn to_section_data(&self) -> SectionData {
        SectionData {
            name: self.name.clone(),
            // The credit rides on the shots; only a shot-less section still
            // holds a legacy pair, which is then its template.
            surveyor: self.legacy_surveyor.clone(),
            date: self.legacy_date,
            color: self.color.clone(),
            direction: Some(self.direction),
            quality: self.quality,
            shots: self.shots.iter().map(Shot::to_shot_data).collect(),
        }
    }

    /// Add a shot to the end of this section
    pub fn add_shot(&mut self, data: ShotData) -> &Shot {
        let shot = Shot::new(self.next_shot_id, data);
        self.next_shot_id += 1;
        self.shots.push(shot);
        let idx = self.shots.len() - 1;
        &self.shots[idx]
    }

    /// Insert a shot after a specific shot ID, forcing the new shot's id when
    /// `forced_id` is `Some` (used by redo to preserve the station id).
    ///
    /// `forced_uuid` is the FALLBACK identity, not an override: the precedence
    /// is `data.uuid`, then `forced_uuid`, then a fresh mint - decided once in
    /// [`Shot::with_uuid`]. Redo passes the UUID its undo entry recorded, which
    /// wins whenever the recorded `ShotData` predates #423 and so carries none.
    pub fn insert_shot_after_with_id(
        &mut self,
        after_id: ShotId,
        data: ShotData,
        forced_id: Option<ShotId>,
        forced_uuid: Option<String>,
    ) -> Option<&Shot> {
        let pos = self.shots.iter().position(|s| s.id == after_id)?;
        let id = self.take_shot_id(forced_id);
        let shot = Shot::with_uuid(id, data, Some(Shot::uuid_or_mint(forced_uuid)));
        self.shots.insert(pos + 1, shot);
        Some(&self.shots[pos + 1])
    }

    /// Insert a shot before a specific shot ID, forcing the new shot's id when
    /// `forced_id` is `Some` (used by redo to preserve the station id).
    ///
    /// `forced_uuid` is the FALLBACK identity, not an override - see
    /// [`Self::insert_shot_after_with_id`].
    pub fn insert_shot_before_with_id(
        &mut self,
        before_id: ShotId,
        data: ShotData,
        forced_id: Option<ShotId>,
        forced_uuid: Option<String>,
    ) -> Option<&Shot> {
        let pos = self.shots.iter().position(|s| s.id == before_id)?;
        let id = self.take_shot_id(forced_id);
        let shot = Shot::with_uuid(id, data, Some(Shot::uuid_or_mint(forced_uuid)));
        self.shots.insert(pos, shot);
        Some(&self.shots[pos])
    }

    /// Resolve the id for a newly inserted shot: a forced id (keeping the counter
    /// ahead of it), or the next sequential id.
    fn take_shot_id(&mut self, forced_id: Option<ShotId>) -> ShotId {
        match forced_id {
            Some(id) => {
                // SATURATING: a forced id can arrive verbatim from a persisted
                // undo entry (the redo of a CreateShot), so ShotId::MAX must
                // park the counter rather than overflow (#413 finding 24's
                // class). The guard keeps monotonicity without a `max`.
                if id >= self.next_shot_id {
                    self.next_shot_id = id.saturating_add(1);
                }
                id
            }
            None => {
                let id = self.next_shot_id;
                self.next_shot_id = self.next_shot_id.saturating_add(1);
                id
            }
        }
    }

    /// Get a shot by ID
    pub fn get_shot(&self, shot_id: ShotId) -> Option<&Shot> {
        self.shots.iter().find(|s| s.id == shot_id)
    }

    /// Get a mutable reference to a shot by ID
    pub fn get_shot_mut(&mut self, shot_id: ShotId) -> Option<&mut Shot> {
        self.shots.iter_mut().find(|s| s.id == shot_id)
    }

    /// Remove a shot by ID, returns the removed shot if found
    pub fn remove_shot(&mut self, shot_id: ShotId) -> Option<Shot> {
        let pos = self.shots.iter().position(|s| s.id == shot_id)?;
        Some(self.shots.remove(pos))
    }

    /// Apply partial updates to this section
    pub fn apply_updates(&mut self, updates: &SectionUpdates) {
        if let Some(v) = &updates.name {
            self.name.clone_from(v);
        }
        if self.shots.is_empty() {
            // No shot to carry them: the section keeps the legacy pair.
            if let Some(v) = &updates.surveyor {
                self.legacy_surveyor = v.clone().filter(|t| !t.trim().is_empty());
            }
            if let Some(v) = &updates.date {
                self.legacy_date = *v;
            }
        }
        if let Some(v) = &updates.surveyor {
            self.set_legacy_surveyor_on_every_shot(v.as_deref());
        }
        if let Some(v) = &updates.date {
            for shot in &mut self.shots {
                let mut credit = shot.credit().clone();
                credit.date = *v;
                shot.set_credit(credit);
            }
        }
        if let Some(v) = &updates.color {
            self.color.clone_from(v);
        }
        if let Some(v) = updates.direction {
            self.direction = v;
        }
        if let Some(v) = updates.quality {
            if v.is_finite() && v > 0.0 {
                self.quality = v;
            } else {
                self.quality = default_section_quality();
            }
        }
    }

    /// [`SectionUpdates::surveyor`]: the legacy free text set on every shot.
    /// The surveyors are replaced by the text's; the explorers only when the
    /// text is in the Ariane `"explorers / surveyors"` form, so a plain name
    /// typed into a legacy Surveyor field leaves every shot's explorers alone.
    fn set_legacy_surveyor_on_every_shot(&mut self, text: Option<&str>) {
        let text = text.unwrap_or_default();
        let has_explorer_half = text.split(" / ").count() == 2;
        let (explorers, surveyors) = legacy_people(text);
        for shot in &mut self.shots {
            let mut credit = shot.credit().clone();
            credit.surveyors.clone_from(&surveyors);
            if has_explorer_half {
                credit.explorers.clone_from(&explorers);
            }
            shot.set_credit(credit);
        }
    }

    /// Who surveyed and explored this section and when, aggregated over its
    /// shots: each person with the number of shots that credit them, in
    /// first-appearance order, and the earliest and latest shot date.
    pub fn credit_summary(&self) -> SectionCreditSummary {
        SectionCreditSummary::from_credits(self.shots.iter().map(Shot::credit))
    }

    /// The distinct surveyor names of every shot, in first-appearance order,
    /// joined with `", "`; `None` when no shot names one. What the legacy
    /// `sections.surveyor` column and the plain-text consumers carry.
    pub fn surveyor_names(&self) -> Option<String> {
        super::join_names(&self.credit_summary().surveyor_people())
    }

    /// The legacy `sections.surveyor` text: [`Self::surveyor_names`] with each
    /// display name once ([`super::join_distinct_names`]), so two same-named
    /// people with different emails do not read as a duplicate in older
    /// builds and SpeleoDB. The people themselves live in the per-shot tables.
    pub fn legacy_surveyor_text(&self) -> Option<String> {
        super::join_distinct_names(&self.credit_summary().surveyor_people())
    }

    /// The earliest shot date; what the legacy `sections.date` column carries.
    pub fn first_survey_date(&self) -> Option<NaiveDate> {
        self.shots.iter().filter_map(|s| s.credit().date).min()
    }

    /// Move a legacy section-level surveyor/date (read from an old snapshot
    /// or undo payload into [`Self::legacy_surveyor`]/[`Self::legacy_date`])
    /// onto every shot whose credit is empty, then clear the legacy fields. A
    /// section with no shots keeps them (see [`Self::legacy_surveyor`]).
    /// Returns whether any shot changed. Idempotent.
    pub fn absorb_legacy_credit(&mut self) -> bool {
        if self.shots.is_empty() {
            return false;
        }
        let template = std::sync::Arc::new(self.take_legacy_template());
        if template.is_empty() {
            return false;
        }
        let mut changed = false;
        for shot in &mut self.shots {
            if shot.credit().is_empty() {
                shot.set_shared_credit(&template);
                changed = true;
            }
        }
        changed
    }

    /// The legacy pair as a credit template, clearing it from the section.
    pub fn take_legacy_template(&mut self) -> SurveyCredit {
        SurveyCredit::from_legacy(
            self.legacy_surveyor.take().as_deref(),
            self.legacy_date.take(),
        )
    }

    /// Split shots at the given shot ID, excluding the target shot.
    /// Returns `(position, shots_before, shots_after)` where position is the index of the
    /// target shot. Returns `None` if the shot is first or last (edge shots don't split),
    /// or if the shot is not found.
    pub fn split_shots_at(&self, shot_id: ShotId) -> Option<(usize, Vec<ShotData>, Vec<ShotData>)> {
        let pos = self.shots.iter().position(|s| s.id == shot_id)?;

        // Edge shots cannot split
        if pos == 0 || pos == self.shots.len() - 1 {
            return None;
        }

        let shots_before: Vec<ShotData> =
            self.shots[..pos].iter().map(Shot::to_shot_data).collect();
        let shots_after: Vec<ShotData> = self.shots[pos + 1..]
            .iter()
            .map(Shot::to_shot_data)
            .collect();

        Some((pos, shots_before, shots_after))
    }

    /// Calculate statistics for this section
    ///
    /// Depth is deliberately absent: it is only meaningful as the ABSOLUTE
    /// station range, which needs the computed stations rather than this
    /// section's shots, so it lives on `ProjectStats` alone (#481).
    pub fn stats(&self) -> SectionStats {
        let mut total_length = 0.0;
        let mut jump_count = 0;
        let mut reference_count = 0;

        for shot in &self.shots {
            match shot.kind {
                ShotKind::Normal => total_length += shot.length,
                ShotKind::Jump => {
                    jump_count += 1;
                    // A jump IS surveyed passage, swum at a higher RMV (#27 q7).
                    total_length += shot.length;
                }
                // A reference tie repeats passage already surveyed, so its
                // length is never counted again (#689).
                ShotKind::Reference => reference_count += 1,
            }
        }

        SectionStats {
            shot_count: self.shots.len(),
            total_length,
            jump_shot_count: jump_count,
            reference_shot_count: reference_count,
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::{ShotData, ShotKind};

    // === SurveyDirection Tests ===

    #[test]
    fn test_survey_direction_default() {
        let direction: SurveyDirection = SurveyDirection::default();
        assert_eq!(direction, SurveyDirection::In);
    }

    #[test]
    fn test_survey_direction_display() {
        assert_eq!(format!("{}", SurveyDirection::In), "in");
        assert_eq!(format!("{}", SurveyDirection::Out), "out");
    }

    #[test]
    fn test_survey_direction_from_str_valid() {
        assert_eq!(
            SurveyDirection::from_str("in").unwrap(),
            SurveyDirection::In
        );
        assert_eq!(
            SurveyDirection::from_str("In").unwrap(),
            SurveyDirection::In
        );
        assert_eq!(
            SurveyDirection::from_str("IN").unwrap(),
            SurveyDirection::In
        );
        assert_eq!(
            SurveyDirection::from_str("out").unwrap(),
            SurveyDirection::Out
        );
        assert_eq!(
            SurveyDirection::from_str("Out").unwrap(),
            SurveyDirection::Out
        );
        assert_eq!(
            SurveyDirection::from_str("OUT").unwrap(),
            SurveyDirection::Out
        );
    }

    #[test]
    fn test_survey_direction_from_str_invalid() {
        let result = SurveyDirection::from_str("invalid");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid direction"));
    }

    #[test]
    fn test_survey_direction_serde_roundtrip() {
        let direction_in = SurveyDirection::In;
        let serialized = serde_json::to_string(&direction_in).unwrap();
        assert_eq!(serialized, "\"in\"");
        let deserialized: SurveyDirection = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, direction_in);

        let direction_out = SurveyDirection::Out;
        let serialized = serde_json::to_string(&direction_out).unwrap();
        assert_eq!(serialized, "\"out\"");
        let deserialized: SurveyDirection = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, direction_out);
    }

    // === SectionData Tests ===

    #[test]
    fn test_section_data_default() {
        let data = SectionData::default();
        assert!(data.name.is_none());
        assert!(data.surveyor.is_none());
        assert!(data.date.is_none());
        assert_eq!(data.color, "#4ecdc4");
        assert!(data.direction.is_none());
        assert!(data.shots.is_empty());
    }

    // === Helper Functions ===

    fn test_shot_data(length: f64) -> ShotData {
        ShotData {
            azimuth_in: 45.0,
            azimuth_out: 225.0,
            length,
            depth_in: 10.0,
            depth_out: 11.0,
            ..Default::default()
        }
    }

    fn test_shot_data_with_depths(length: f64, depth_in: f64, depth_out: f64) -> ShotData {
        ShotData {
            azimuth_in: 45.0,
            azimuth_out: 225.0,
            length,
            depth_in,
            depth_out,
            ..Default::default()
        }
    }

    fn test_shot_data_of_kind(length: f64, kind: ShotKind) -> ShotData {
        ShotData {
            azimuth_in: 0.0,
            azimuth_out: 180.0,
            length,
            depth_in: 5.0,
            depth_out: 5.0,
            kind,
            ..Default::default()
        }
    }

    fn test_reference_shot_data(length: f64) -> ShotData {
        test_shot_data_of_kind(length, ShotKind::Reference)
    }

    fn test_jump_shot_data(length: f64) -> ShotData {
        test_shot_data_of_kind(length, ShotKind::Jump)
    }

    // === Section Creation Tests ===

    #[test]
    fn test_section_new_empty() {
        let section = Section::new(1, SectionData::default());

        assert_eq!(section.id, 1);
        assert!(section.name.is_none());
        assert!(section.legacy_surveyor.is_none());
        assert!(section.legacy_date.is_none());
        assert_eq!(section.credit_summary().shot_count, 0);
        assert_eq!(section.color, "#4ecdc4");
        assert_eq!(section.direction, SurveyDirection::In);
        assert!(section.shots.is_empty());
        assert_eq!(section.next_shot_id, 1 * FEATURE_ID_STRIDE);
    }

    #[test]
    fn test_section_new_with_metadata() {
        let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
        let data = SectionData {
            name: Some("Test Section".to_string()),
            surveyor: Some("John Doe".to_string()),
            date: Some(date),
            color: "#ff5733".to_string(),
            direction: Some(SurveyDirection::Out),
            quality: 1.0,
            shots: vec![test_shot_data(3.0), test_shot_data(4.0)],
        };

        let section = Section::new(42, data);

        assert_eq!(section.id, 42);
        assert_eq!(section.name, Some("Test Section".to_string()));
        // The legacy template lands on the shots; the section keeps none.
        assert!(section.legacy_surveyor.is_none());
        for shot in &section.shots {
            assert_eq!(shot.credit().surveyor_names().as_deref(), Some("John Doe"));
            assert_eq!(shot.credit().date, Some(date));
        }
        let summary = section.credit_summary();
        assert_eq!(summary.surveyors.len(), 1);
        assert_eq!(summary.surveyors[0].shots, 2);
        assert_eq!(summary.date_min, Some(date));
        assert_eq!(section.color, "#ff5733");
        assert_eq!(section.direction, SurveyDirection::Out);
    }

    #[test]
    fn test_section_new_with_shots() {
        let data = SectionData {
            shots: vec![
                test_shot_data(5.0),
                test_shot_data(7.0),
                test_shot_data(3.0),
            ],
            ..Default::default()
        };

        let section = Section::new(0, data);

        assert_eq!(section.shots.len(), 3);
        assert_eq!(section.shots[0].id, 0);
        assert_eq!(section.shots[1].id, 1);
        assert_eq!(section.shots[2].id, 2);
        assert_eq!(section.next_shot_id, 3);

        assert_eq!(section.shots[0].length, 5.0);
        assert_eq!(section.shots[1].length, 7.0);
        assert_eq!(section.shots[2].length, 3.0);
    }

    #[test]
    fn test_section_new_with_id() {
        let data = SectionData {
            name: Some("Restored Section".to_string()),
            ..Default::default()
        };

        let section = Section::new_with_id(100, data);

        assert_eq!(section.id, 100);
        assert_eq!(section.name, Some("Restored Section".to_string()));
    }

    #[test]
    fn test_section_direction_defaults_to_in() {
        let data = SectionData {
            direction: None,
            ..Default::default()
        };

        let section = Section::new(0, data);
        assert_eq!(section.direction, SurveyDirection::In);
    }

    // === Shot Management Tests ===

    #[test]
    fn test_add_shot() {
        let mut section = Section::new(0, SectionData::default());

        let shot = section.add_shot(test_shot_data(10.0));

        assert_eq!(shot.id, 0);
        assert_eq!(shot.length, 10.0);
        assert_eq!(section.shots.len(), 1);
        assert_eq!(section.next_shot_id, 1);
    }

    #[test]
    fn test_add_multiple_shots() {
        let mut section = Section::new(0, SectionData::default());

        section.add_shot(test_shot_data(5.0));
        section.add_shot(test_shot_data(7.0));
        section.add_shot(test_shot_data(3.0));

        assert_eq!(section.shots.len(), 3);
        assert_eq!(section.next_shot_id, 3);
        assert_eq!(section.shots[0].id, 0);
        assert_eq!(section.shots[1].id, 1);
        assert_eq!(section.shots[2].id, 2);
    }

    #[test]
    fn test_insert_shot_after() {
        let mut section = Section::new(
            0,
            SectionData {
                shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
                ..Default::default()
            },
        );

        // Insert after shot 0
        let inserted = section.insert_shot_after_with_id(0, test_shot_data(10.0), None, None);

        assert!(inserted.is_some());
        let inserted = inserted.unwrap();
        assert_eq!(inserted.id, 2); // IDs continue from next_shot_id
        assert_eq!(inserted.length, 10.0);

        assert_eq!(section.shots.len(), 3);
        assert_eq!(section.shots[0].length, 5.0);
        assert_eq!(section.shots[1].length, 10.0); // New shot
        assert_eq!(section.shots[2].length, 7.0);
    }

    #[test]
    fn test_insert_shot_after_nonexistent() {
        let mut section = Section::new(
            0,
            SectionData {
                shots: vec![test_shot_data(5.0)],
                ..Default::default()
            },
        );

        let result = section.insert_shot_after_with_id(999, test_shot_data(10.0), None, None);

        assert!(result.is_none());
        assert_eq!(section.shots.len(), 1); // Unchanged
    }

    #[test]
    fn test_get_shot() {
        let section = Section::new(
            0,
            SectionData {
                shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
                ..Default::default()
            },
        );

        let shot = section.get_shot(1);
        assert!(shot.is_some());
        assert_eq!(shot.unwrap().length, 7.0);

        let nonexistent = section.get_shot(999);
        assert!(nonexistent.is_none());
    }

    #[test]
    fn test_get_shot_mut() {
        let mut section = Section::new(
            0,
            SectionData {
                shots: vec![test_shot_data(5.0)],
                ..Default::default()
            },
        );

        {
            let shot = section.get_shot_mut(0);
            assert!(shot.is_some());
            shot.unwrap().length = 15.0;
        }

        assert_eq!(section.shots[0].length, 15.0);
    }

    #[test]
    fn test_remove_shot() {
        let mut section = Section::new(
            0,
            SectionData {
                shots: vec![
                    test_shot_data(5.0),
                    test_shot_data(7.0),
                    test_shot_data(3.0),
                ],
                ..Default::default()
            },
        );

        let removed = section.remove_shot(1);

        assert!(removed.is_some());
        assert_eq!(removed.unwrap().length, 7.0);
        assert_eq!(section.shots.len(), 2);
        assert_eq!(section.shots[0].length, 5.0);
        assert_eq!(section.shots[1].length, 3.0);
    }

    #[test]
    fn test_remove_shot_nonexistent() {
        let mut section = Section::new(
            0,
            SectionData {
                shots: vec![test_shot_data(5.0)],
                ..Default::default()
            },
        );

        let result = section.remove_shot(999);

        assert!(result.is_none());
        assert_eq!(section.shots.len(), 1);
    }

    // === Section Update Tests ===

    #[test]
    fn test_apply_updates_name() {
        let mut section = Section::new(0, SectionData::default());

        section.apply_updates(&SectionUpdates {
            name: Some(Some("New Name".to_string())),
            ..Default::default()
        });

        assert_eq!(section.name, Some("New Name".to_string()));
    }

    #[test]
    fn test_apply_updates_clear_name() {
        let mut section = Section::new(
            0,
            SectionData {
                name: Some("Original".to_string()),
                ..Default::default()
            },
        );

        section.apply_updates(&SectionUpdates {
            name: Some(None), // Set to None
            ..Default::default()
        });

        assert!(section.name.is_none());
    }

    #[test]
    fn test_apply_updates_all_fields() {
        let mut section = Section::new(
            0,
            SectionData {
                shots: vec![test_shot_data(2.0)],
                ..Default::default()
            },
        );
        let new_date = NaiveDate::from_ymd_opt(2024, 12, 25).unwrap();

        section.apply_updates(&SectionUpdates {
            name: Some(Some("Updated".to_string())),
            surveyor: Some(Some("Jane".to_string())),
            date: Some(Some(new_date)),
            color: Some("#abc123".to_string()),
            direction: Some(SurveyDirection::Out),
            quality: Some(1.5),
        });

        assert_eq!(section.name, Some("Updated".to_string()));
        assert_eq!(
            section.shots[0].credit().surveyor_names().as_deref(),
            Some("Jane")
        );
        assert_eq!(section.shots[0].credit().date, Some(new_date));
        assert_eq!(section.color, "#abc123");
        assert_eq!(section.direction, SurveyDirection::Out);
        assert_eq!(section.quality, 1.5);
    }

    #[test]
    fn test_apply_updates_partial() {
        let mut section = Section::new(
            0,
            SectionData {
                name: Some("Original".to_string()),
                surveyor: Some("Original Surveyor".to_string()),
                color: "#000000".to_string(),
                shots: vec![test_shot_data(2.0)],
                ..Default::default()
            },
        );

        // Only update color
        section.apply_updates(&SectionUpdates {
            color: Some("#ffffff".to_string()),
            ..Default::default()
        });

        assert_eq!(section.name, Some("Original".to_string())); // Unchanged
        assert_eq!(
            section.shots[0].credit().surveyor_names().as_deref(),
            Some("Original Surveyor")
        ); // Unchanged
        assert_eq!(section.color, "#ffffff"); // Changed
    }

    // === Section Statistics Tests ===

    #[test]
    fn test_stats_empty_section() {
        let section = Section::new(0, SectionData::default());
        let stats = section.stats();

        assert_eq!(stats.shot_count, 0);
        assert_eq!(stats.total_length, 0.0);
        assert_eq!(stats.reference_shot_count, 0);
    }

    #[test]
    fn test_stats_single_shot() {
        let section = Section::new(
            0,
            SectionData {
                shots: vec![test_shot_data_with_depths(10.0, 5.0, 8.0)],
                ..Default::default()
            },
        );

        let stats = section.stats();

        assert_eq!(stats.shot_count, 1);
        assert_eq!(stats.total_length, 10.0);
        assert_eq!(stats.reference_shot_count, 0);
    }

    #[test]
    fn test_stats_multiple_shots() {
        let section = Section::new(
            0,
            SectionData {
                shots: vec![
                    test_shot_data_with_depths(5.0, 10.0, 12.0),
                    test_shot_data_with_depths(7.0, 12.0, 15.0),
                    test_shot_data_with_depths(3.0, 15.0, 8.0),
                ],
                ..Default::default()
            },
        );

        let stats = section.stats();

        assert_eq!(stats.shot_count, 3);
        assert_eq!(stats.total_length, 15.0); // 5 + 7 + 3
    }

    #[test]
    fn test_stats_with_reference_shots() {
        let section = Section::new(
            0,
            SectionData {
                shots: vec![
                    test_shot_data(5.0),
                    test_reference_shot_data(3.0),
                    test_shot_data(7.0),
                    test_reference_shot_data(2.0),
                ],
                ..Default::default()
            },
        );

        let stats = section.stats();

        assert_eq!(stats.shot_count, 4);
        // A Reference repeats passage already surveyed, so its length drops
        // out of the total (#689) while it still counts as a shot.
        assert_eq!(stats.total_length, 12.0); // 5 + 7
        assert_eq!(stats.reference_shot_count, 2);
        assert_eq!(stats.jump_shot_count, 0);
    }

    #[test]
    fn test_stats_only_reference_shots_has_zero_length() {
        let section = Section::new(
            0,
            SectionData {
                shots: vec![test_reference_shot_data(4.0), test_reference_shot_data(6.0)],
                ..Default::default()
            },
        );

        let stats = section.stats();

        assert_eq!(stats.shot_count, 2);
        assert_eq!(stats.total_length, 0.0);
        assert_eq!(stats.reference_shot_count, 2);
    }

    #[test]
    fn test_stats_counts_jumps_separately_and_in_length() {
        let section = Section::new(
            0,
            SectionData {
                shots: vec![
                    test_shot_data(5.0),
                    test_jump_shot_data(3.0),
                    test_reference_shot_data(2.0),
                ],
                ..Default::default()
            },
        );

        let stats = section.stats();

        assert_eq!(stats.shot_count, 3);
        // A jump IS surveyed passage (#27 q7), so only the Reference is excluded.
        assert_eq!(stats.total_length, 8.0); // 5 + 3
        assert_eq!(stats.jump_shot_count, 1);
        assert_eq!(stats.reference_shot_count, 1);
    }

    // === Section Data Conversion Tests ===

    #[test]
    fn test_to_section_data() {
        let date = NaiveDate::from_ymd_opt(2024, 6, 15).unwrap();
        let original_data = SectionData {
            name: Some("Test".to_string()),
            surveyor: Some("John".to_string()),
            date: Some(date),
            color: "#ff0000".to_string(),
            direction: Some(SurveyDirection::Out),
            quality: 1.0,
            shots: vec![test_shot_data(5.0), test_shot_data(7.0)],
        };

        let section = Section::new(1, original_data);
        let converted = section.to_section_data();

        assert_eq!(converted.name, Some("Test".to_string()));
        // The template went onto the shots, so the data carries it there.
        assert_eq!(converted.surveyor, None);
        assert_eq!(converted.date, None);
        assert_eq!(
            converted.shots[0].credit().surveyor_names().as_deref(),
            Some("John")
        );
        assert_eq!(converted.shots[1].credit().date, Some(date));
        assert_eq!(converted.color, "#ff0000");
        assert_eq!(converted.direction, Some(SurveyDirection::Out));
        assert_eq!(converted.shots.len(), 2);
        assert_eq!(converted.shots[0].length, 5.0);
        assert_eq!(converted.shots[1].length, 7.0);
    }

    #[test]
    fn test_to_section_data_roundtrip() {
        let date = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let original_data = SectionData {
            name: Some("Roundtrip Test".to_string()),
            surveyor: Some("Tester".to_string()),
            date: Some(date),
            color: "#123456".to_string(),
            direction: Some(SurveyDirection::In),
            quality: 1.0,
            shots: vec![ShotData {
                azimuth_in: 90.0,
                azimuth_out: 270.0,
                length: 10.0,
                depth_in: 5.0,
                depth_out: 8.0,
                left: Some(1.0),
                right: Some(2.0),
                up: Some(3.0),
                down: Some(4.0),
                comment: Some("Test comment".to_string()),
                ..Default::default()
            }],
        };

        let section = Section::new(0, original_data);
        let converted = section.to_section_data();
        let restored = Section::new(0, converted);

        assert_eq!(restored.name, section.name);
        assert_eq!(restored.shots[0].credit(), section.shots[0].credit());
        assert_eq!(restored.shots[0].credit().date, Some(date));
        assert_eq!(restored.color, section.color);
        assert_eq!(restored.direction, section.direction);
        assert_eq!(restored.shots.len(), section.shots.len());
        assert_eq!(restored.shots[0].length, section.shots[0].length);
        assert_eq!(restored.shots[0].comment, section.shots[0].comment);
    }

    // === SectionUpdates Serde Tests ===

    #[test]
    fn test_section_updates_default() {
        let updates = SectionUpdates::default();

        assert!(updates.name.is_none());
        assert!(updates.surveyor.is_none());
        assert!(updates.date.is_none());
        assert!(updates.color.is_none());
        assert!(updates.direction.is_none());
    }

    #[test]
    fn test_section_updates_serde() {
        let updates = SectionUpdates {
            name: Some(Some("Updated Name".to_string())),
            color: Some("#aabbcc".to_string()),
            ..Default::default()
        };

        let json = serde_json::to_string(&updates).unwrap();
        let deserialized: SectionUpdates = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, Some(Some("Updated Name".to_string())));
        assert_eq!(deserialized.color, Some("#aabbcc".to_string()));
        assert!(deserialized.surveyor.is_none());
    }

    // === SectionStats Tests ===

    #[test]
    fn test_section_stats_clone() {
        let section = Section::new(
            0,
            SectionData {
                shots: vec![test_shot_data(10.0)],
                ..Default::default()
            },
        );

        let stats = section.stats();
        let cloned = stats.clone();

        assert_eq!(cloned.shot_count, stats.shot_count);
        assert_eq!(cloned.total_length, stats.total_length);
    }

    #[test]
    fn test_section_stats_serde() {
        let stats = SectionStats {
            shot_count: 5,
            total_length: 42.5,
            jump_shot_count: 2,
            reference_shot_count: 1,
        };

        let json = serde_json::to_string(&stats).unwrap();
        let deserialized: SectionStats = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.shot_count, 5);
        assert_eq!(deserialized.total_length, 42.5);
        assert_eq!(deserialized.jump_shot_count, 2);
        assert_eq!(deserialized.reference_shot_count, 1);
    }

    // === Split Shots Tests ===

    #[test]
    fn test_split_shots_at_middle() {
        // 5-shot section, split at index 2 -> A has 2 shots, B has 2 shots
        let section = Section::new(
            0,
            SectionData {
                shots: vec![
                    test_shot_data(5.0),
                    test_shot_data(7.0),
                    test_shot_data(3.0), // shot to delete (index 2)
                    test_shot_data(4.0),
                    test_shot_data(6.0),
                ],
                ..Default::default()
            },
        );

        let shot_id = section.shots[2].id; // middle shot
        let result = section.split_shots_at(shot_id);
        assert!(result.is_some());

        let (pos, before, after) = result.unwrap();
        assert_eq!(pos, 2);
        assert_eq!(before.len(), 2);
        assert_eq!(after.len(), 2);
        assert_eq!(before[0].length, 5.0);
        assert_eq!(before[1].length, 7.0);
        assert_eq!(after[0].length, 4.0);
        assert_eq!(after[1].length, 6.0);
    }

    #[test]
    fn test_split_shots_rejects_first_shot() {
        let section = Section::new(
            0,
            SectionData {
                shots: vec![
                    test_shot_data(5.0),
                    test_shot_data(7.0),
                    test_shot_data(3.0),
                ],
                ..Default::default()
            },
        );

        let first_shot_id = section.shots[0].id;
        assert!(section.split_shots_at(first_shot_id).is_none());
    }

    #[test]
    fn test_split_shots_rejects_last_shot() {
        let section = Section::new(
            0,
            SectionData {
                shots: vec![
                    test_shot_data(5.0),
                    test_shot_data(7.0),
                    test_shot_data(3.0),
                ],
                ..Default::default()
            },
        );

        let last_shot_id = section.shots[2].id;
        assert!(section.split_shots_at(last_shot_id).is_none());
    }

    #[test]
    fn test_split_shots_rejects_nonexistent() {
        let section = Section::new(
            0,
            SectionData {
                shots: vec![test_shot_data(5.0)],
                ..Default::default()
            },
        );

        assert!(section.split_shots_at(999).is_none());
    }

    #[test]
    fn test_split_shots_at_index_1_of_3() {
        // 3-shot section, split at index 1 -> A has 1 shot, B has 1 shot
        let section = Section::new(
            0,
            SectionData {
                shots: vec![
                    test_shot_data(5.0),
                    test_shot_data(7.0), // shot to delete
                    test_shot_data(3.0),
                ],
                ..Default::default()
            },
        );

        let shot_id = section.shots[1].id;
        let result = section.split_shots_at(shot_id);
        assert!(result.is_some());

        let (pos, before, after) = result.unwrap();
        assert_eq!(pos, 1);
        assert_eq!(before.len(), 1);
        assert_eq!(after.len(), 1);
        assert_eq!(before[0].length, 5.0);
        assert_eq!(after[0].length, 3.0);
    }

    // === Survey credit ===

    fn credit(surveyors: &[&str], explorers: &[&str]) -> SurveyCredit {
        SurveyCredit {
            surveyors: surveyors.iter().filter_map(|n| Person::named(n)).collect(),
            explorers: explorers.iter().filter_map(|n| Person::named(n)).collect(),
            date: None,
        }
    }

    #[test]
    fn the_template_only_fills_shots_with_no_credit() {
        let own = credit(&["Own"], &[]);
        let section = Section::new(
            1,
            SectionData {
                surveyor: Some("nh / ds".to_string()),
                shots: vec![
                    ShotData {
                        credit: Some(own.clone()),
                        ..test_shot_data(1.0)
                    },
                    test_shot_data(2.0),
                ],
                ..Default::default()
            },
        );
        assert_eq!(section.shots[0].credit(), &own);
        assert_eq!(section.shots[1].credit(), &credit(&["ds"], &["nh"]));
    }

    #[test]
    fn legacy_surveyor_update_keeps_explorers_unless_the_text_names_them() {
        let mut section = Section::new(
            1,
            SectionData {
                shots: vec![ShotData {
                    credit: Some(credit(&["Old"], &["Ex"])),
                    ..test_shot_data(1.0)
                }],
                ..Default::default()
            },
        );
        section.apply_updates(&SectionUpdates {
            surveyor: Some(Some("New".to_string())),
            ..Default::default()
        });
        assert_eq!(section.shots[0].credit(), &credit(&["New"], &["Ex"]));
        section.apply_updates(&SectionUpdates {
            surveyor: Some(Some("E2 / S2".to_string())),
            ..Default::default()
        });
        assert_eq!(section.shots[0].credit(), &credit(&["S2"], &["E2"]));
        section.apply_updates(&SectionUpdates {
            surveyor: Some(None),
            ..Default::default()
        });
        assert_eq!(section.shots[0].credit(), &credit(&[], &["E2"]));
    }

    #[test]
    fn an_old_section_json_with_a_surveyor_is_absorbed_onto_its_shots() {
        let mut section = Section::new(
            3,
            SectionData {
                shots: vec![test_shot_data(1.0), test_shot_data(2.0)],
                ..Default::default()
            },
        );
        section.shots[1].set_credit(credit(&["Kept"], &[]));
        let mut json = serde_json::to_value(&section).unwrap();
        assert!(json.get("surveyor").is_none(), "never serialized: {json}");
        assert!(json.get("date").is_none(), "never serialized: {json}");
        json["surveyor"] = serde_json::json!("nh / ds");
        json["date"] = serde_json::json!("2021-04-05");

        let mut old: Section = serde_json::from_value(json).unwrap();
        assert_eq!(old.legacy_surveyor.as_deref(), Some("nh / ds"));
        assert!(old.absorb_legacy_credit());
        assert!(old.legacy_surveyor.is_none() && old.legacy_date.is_none());
        let mut expected = credit(&["ds"], &["nh"]);
        expected.date = NaiveDate::from_ymd_opt(2021, 4, 5);
        assert_eq!(old.shots[0].credit(), &expected);
        assert_eq!(old.shots[1].credit(), &credit(&["Kept"], &[]));
        // Idempotent.
        assert!(!old.absorb_legacy_credit());
    }

    #[test]
    fn section_ts_shape_has_no_surveyor_or_date() {
        use ts_rs::TS;
        let decl = Section::decl();
        assert!(!decl.contains("surveyor"), "{decl}");
        assert!(!decl.contains(" date"), "{decl}");
    }

    #[test]
    fn a_shotless_section_keeps_its_legacy_pair_on_the_wire() {
        let date = NaiveDate::from_ymd_opt(2022, 3, 4);
        let section = Section::new(
            7,
            SectionData {
                surveyor: Some("Lone Surveyor".to_string()),
                date,
                ..Default::default()
            },
        );
        assert_eq!(section.legacy_surveyor.as_deref(), Some("Lone Surveyor"));
        assert_eq!(section.legacy_date, date);
        let data = section.to_section_data();
        assert_eq!(data.surveyor.as_deref(), Some("Lone Surveyor"));
        let json = serde_json::to_value(&section).unwrap();
        assert_eq!(json["surveyor"], "Lone Surveyor");
        let mut back: Section = serde_json::from_value(json).unwrap();
        assert!(!back.absorb_legacy_credit(), "nothing to absorb into");
        assert_eq!(back.legacy_surveyor.as_deref(), Some("Lone Surveyor"));
    }
}
