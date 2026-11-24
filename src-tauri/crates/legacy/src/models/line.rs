use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use super::{
    legacy_people, AzimuthReference, Coordinates, GpsCoordinates, LineCreditSummary, Person,
    SectionId, Shot, ShotData, ShotId, ShotKind, ShotUpdates, StationRef, SurveyCredit,
    FEATURE_ID_STRIDE,
};

/// Unique identifier for a line
pub type LineId = u32;

/// What the user reads when an edit addresses a line by a handle the project
/// no longer holds (deleted since, a stale view): never the internal id,
/// which the user never sees (decision 2 of the line-identity contract).
pub const LINE_GONE: &str = "That line no longer exists.";

/// [`LINE_GONE`] for a shot.
pub const SHOT_GONE: &str = "That shot no longer exists.";

/// Survey direction (In = going into cave, Out = returning)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    /// Internal immutable ID
    pub id: LineId,
    /// Optional human-readable name
    pub name: Option<String>,
    /// LEGACY: the free-text surveyor a line carried before the survey
    /// credit moved onto its shots. Absent from the TS shape. An old project
    /// snapshot or undo payload that still carries `"surveyor"` deserializes
    /// into it, and [`Line::absorb_legacy_credit`] moves it onto the shots.
    ///
    /// A line WITH NO SHOTS keeps it: there is no shot to carry it, so it
    /// stays the line's own (written to `sections.surveyor`, serialized in
    /// undo snapshots) until the first shot arrives and takes it as its credit
    /// (`AppState::create_shot`). Only such a line ever holds one, so the key
    /// is on the wire only for a shot-less line. Read who surveyed a line
    /// with [`Line::credit_summary`].
    #[serde(default, rename = "surveyor", skip_serializing_if = "Option::is_none")]
    pub legacy_surveyor: Option<String>,
    /// LEGACY: the line-level survey date, kept and absorbed like
    /// [`Line::legacy_surveyor`].
    #[serde(default, rename = "date", skip_serializing_if = "Option::is_none")]
    pub legacy_date: Option<NaiveDate>,
    /// Display color (hex format, e.g., "#4ecdc4")
    pub color: String,
    /// Survey direction (In = entering cave, Out = exiting)
    pub direction: SurveyDirection,
    /// Quality factor (1.0 = default). Used for weighted adjustments.
    #[serde(default = "default_line_quality")]
    pub quality: f64,
    /// Shots in this line, in order
    pub shots: Vec<Shot>,
    /// Next shot ID to assign
    pub next_shot_id: ShotId,
    /// Flips every time this line's shot order is REVERSED (#24). Not a count and not
    /// meaningful on its own - only whether it still matches what something else recorded.
    ///
    /// Exists so the least-squares gauge can stay on one PHYSICAL station. That gauge has
    /// always been "station 0 of the lowest-id line in the component", and a reverse turns
    /// station 0 into station N - pinning the opposite end, which translates the whole adjusted
    /// component and breaks the reverse's "every station keeps its position" guarantee exactly
    /// where closure-adjusted geometry is forced (Plan/Export). Reading the end off this flag
    /// keeps the original choice: `false` means station 0, as every existing project has it.
    #[serde(default)]
    pub reverse_parity: bool,
    /// The least-squares gauge station for this line, as the pair of shots that identify it:
    /// the station is the end of `gauge_shot_uuid` FACING AWAY FROM `gauge_neighbour_uuid`
    /// (#24). `None` until the first save captures it.
    ///
    /// The gauge fixes the otherwise-free absolute translation of a datum-less component by
    /// pinning one station at its measured coordinates, so it has to name the SAME physical
    /// station for the life of the project - if it moves, the whole component translates by the
    /// adjustment residual and Plan/Export coordinates jump.
    ///
    /// An index cannot do that. "Station 0, or `shots.len()` when the line is reversed" is
    /// stable across the reverse itself but not across what comes next: appending to a reversed
    /// line grows `len` and walks the gauge onto the newly created far station. A pair is
    /// immune - neither shot moves relative to the other when the line is extended at either
    /// end, and a reverse carries both together, so "away from the neighbour" names the same
    /// junction throughout.
    #[serde(default)]
    pub gauge_shot_uuid: Option<String>,
    /// The shot on the far side of [`Self::gauge_shot_uuid`], giving the gauge station its
    /// direction. `None` for a single-shot line, which has no second shot to face away from.
    #[serde(default)]
    pub gauge_neighbour_uuid: Option<String>,
    /// How this line's stored azimuths relate to true north (#438).
    ///
    /// `None` = inherit [`Project::default_azimuth_reference`], which is itself
    /// `None` on every project that predates this field - and an absent
    /// reference means [`AzimuthReference::default`], "nothing recorded", whose
    /// correction is 0. So the field is additive in the strong sense: a project
    /// that never saw it computes exactly the geometry it always did.
    ///
    /// It lives on the LINE, not the project, because that is the finest
    /// granularity the source formats have: one Compass file can declare a
    /// different declination per survey and can switch mode mid-file, and a
    /// project routinely mixes an Ariane import (magnetic) with a Compass one.
    ///
    /// [`Project::default_azimuth_reference`]: crate::models::models::Project::default_azimuth_reference
    ///
    /// `#[ts(optional)]`, like every other additive field the frontend does not
    /// construct: the payload always carries it, but a TS `Line` literal
    /// built before #438 stays valid.
    #[serde(default)]
    pub azimuth_reference: Option<AzimuthReference>,
    /// The [`crate::models::Section`] (named grouping of lines) this line belongs
    /// to; `None` is ungrouped. Serialized as `cave_section_id`, never as the
    /// disk name `section_id` (a LINE id, see `disk_names`), and absent when
    /// `None` so a project without Sections serializes as before.
    #[serde(
        default,
        rename = "cave_section_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub section_id: Option<SectionId>,
    /// The legacy `sections.surveyor` text this line was READ with, when it
    /// differs from what this build would write for the same people
    /// ([`Self::legacy_surveyor_text`]): a list longer than a role keeps, a
    /// name past the length clamp, another spelling (`A;B`). While nobody's
    /// credit in the line has changed since, a save writes that text back
    /// verbatim ([`Self::legacy_surveyor_to_write`]), so older builds and
    /// SpeleoDB keep seeing exactly what they wrote; the shots themselves hold
    /// the clamped people. Session-only: set by the file reader, never
    /// serialized, so a restored snapshot writes this build's text.
    #[serde(skip)]
    pub loaded_legacy_surveyor: Option<LoadedLegacyText>,
    /// The `sections.created_at` this line was READ with. Not a time the app
    /// tracks or shows: builds before this one stamped every line with the
    /// time of each save, so it only ever held when the file was last saved.
    /// A save writes it back unchanged, so an unchanged project saves to the
    /// same bytes; a line with none (new this session, re-created by an undo
    /// after a delete, or read with a value the reader refused) is written
    /// with the project's `created_at` instead (`Database::save_project`).
    /// Session-only, like [`Self::loaded_legacy_surveyor`]: set by the file
    /// reader, never serialized, and carried across a whole-project undo or
    /// redo by line id (`reapply_heals_after_restore`).
    #[serde(skip)]
    pub loaded_created_at: Option<DateTime<Utc>>,
}

/// [`Line::loaded_legacy_surveyor`]: the text as stored, and the text this
/// build derived for the line right after reading it. While the line
/// still derives that same text, who is credited in it has not changed, and
/// the stored text is still its truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedLegacyText {
    /// `sections.surveyor` as the file held it.
    pub raw: String,
    /// [`Line::legacy_surveyor_text`] once the line was read.
    pub derived: Option<String>,
}

/// Everything about a line that [`LineData`] cannot carry: the identity of
/// its shots, and the two flags that decide where its least-squares gauge sits.
///
/// `LineData` is the CREATE shape - what the user typed - so it holds no
/// uuid, no [`Line::reverse_parity`] and neither gauge anchor. That makes it
/// lossy for RESTORE: rebuilding a line from it alone mints a fresh uuid per
/// shot (dangling every `StationKey::AfterShot` anchor and the Ariane export
/// identity) and resets the gauge pair a save captured (issue #413, finding 1).
/// An undo entry that re-creates a line carries this alongside the data, and
/// [`Line::restore_with_identity`] puts both back.
///
/// FORMAT EVOLUTION (additive, no schema bump): an entry written before this
/// existed deserializes to [`LineIdentity::default`] - no uuids, parity
/// false, no anchors - which is exactly the pre-fix behaviour (mint fresh).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LineIdentity {
    /// The shots' persistent UUIDs, in shot order. An entry per shot, `None`
    /// where the shot genuinely had none (a pre-v6 file loaded with a NULL uuid
    /// column). SHORTER than the line's shots - empty, for a legacy entry -
    /// means the rest mint fresh.
    #[serde(default)]
    pub shot_uuids: Vec<Option<String>>,
    #[serde(default)]
    pub reverse_parity: bool,
    #[serde(default)]
    pub gauge_shot_uuid: Option<String>,
    #[serde(default)]
    pub gauge_neighbour_uuid: Option<String>,
    /// The line's north reference (#438). [`LineData`] cannot carry it, so
    /// without this an undo that re-creates a line would silently drop the
    /// declination and rotate that line by it. `None` on a legacy entry means
    /// "inherit the project default", which is what a restored line would
    /// have had anyway before the field existed.
    #[serde(default)]
    pub azimuth_reference: Option<AzimuthReference>,
    /// Each station's Station ID, by station index, so a re-created line
    /// keeps its stations' ids. Filled by `Project::line_identity` (the line
    /// alone does not hold them); empty in a legacy entry, whose redo then
    /// mints fresh ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub station_ids: Vec<Option<crate::models::StationId>>,
    /// Each station's name, by station index; empty in a legacy entry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub station_names: Vec<Option<String>>,
    /// The Section the line belonged to (#327). [`LineData`] cannot carry
    /// it, so a redo that re-creates a line puts its membership back from
    /// here. Absent (`None`, ungrouped) on a legacy entry.
    #[serde(
        default,
        rename = "cave_section_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub section_id: Option<SectionId>,
}

impl LineIdentity {
    /// The identity `line` carries right now.
    pub fn of(line: &Line) -> Self {
        Self {
            shot_uuids: line.shots.iter().map(|shot| shot.uuid.clone()).collect(),
            reverse_parity: line.reverse_parity,
            gauge_shot_uuid: line.gauge_shot_uuid.clone(),
            gauge_neighbour_uuid: line.gauge_neighbour_uuid.clone(),
            azimuth_reference: line.azimuth_reference,
            station_ids: Vec::new(),
            station_names: Vec::new(),
            section_id: line.section_id,
        }
    }
}

/// Data for creating a new line
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineData {
    pub name: Option<String>,
    /// LEGACY template: free text split by [`legacy_people`] and given, with
    /// [`Self::date`], to every shot whose credit is empty when the line is
    /// built ([`Line::new`]). The line itself keeps no surveyor.
    pub surveyor: Option<String>,
    /// LEGACY template date, applied with [`Self::surveyor`].
    pub date: Option<NaiveDate>,
    pub color: String,
    /// Survey direction (defaults to In if not specified)
    #[serde(default)]
    pub direction: Option<SurveyDirection>,
    /// Quality factor (1.0 = default). Used for weighted adjustments.
    #[serde(default = "default_line_quality")]
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

/// Partial updates to a line
///
/// The three nullable fields use `double_option` + `skip_serializing_if` so the
/// JSON round-trip is lossless: absent <-> outer `None`, `null` <-> `Some(None)`.
/// Undo relies on this - previous values are stored as a serialized
/// `LineUpdates` (`values_json`), so an un-set field must serialize as absent,
/// not `null`. Without it (issue #413 finding 5) a capture of `Some(None)` - "this
/// line had no surveyor, restore that" - came back as `None` and the undo left
/// the new value in place. `ShotUpdates` has carried the same pair since v7.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LineUpdates {
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<Option<String>>,
    /// LEGACY: set on EVERY shot of the line - the surveyors (and, for the
    /// Ariane `"explorers / surveyors"` form, the explorers) parsed by
    /// [`legacy_people`]. `null` clears every shot's surveyors.
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub surveyor: Option<Option<String>>,
    /// LEGACY: set on EVERY shot of the line; `null` clears every shot's date.
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub date: Option<Option<NaiveDate>>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub direction: Option<SurveyDirection>,
    #[serde(default)]
    pub quality: Option<f64>,
}

/// Line statistics (computed, not stored)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineStats {
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

/// Shot-line geometry for a single line within a segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentLineGeometry {
    pub line_id: LineId,
    pub color: String,
    /// Shot lines as pairs of [x, y] coords in meters, relative to the anchor line's station 0
    pub shot_lines: Vec<[[f64; 2]; 2]>,
}

/// Shot-line geometry for all lines in a segment, positioned relative to a given line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentGeometry {
    pub lines: Vec<SegmentLineGeometry>,
}

/// Configuration for a single line being imported via the batch import command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportLineConfig {
    /// Index of this line in the import array (used for fromImporting references)
    pub line_index: u32,
    pub name: Option<String>,
    /// LEGACY template, as [`LineData::surveyor`]: given to every imported
    /// shot whose credit is empty.
    pub surveyor: Option<String>,
    /// LEGACY template date, as [`LineData::date`].
    pub date: Option<NaiveDate>,
    /// When set, REPLACES the surveyors of every imported shot (the import
    /// dialog's Surveyors field). `None` keeps what the file said.
    #[serde(default)]
    pub surveyors: Option<Vec<Person>>,
    /// When set, replaces the explorers of every imported shot.
    #[serde(default)]
    pub explorers: Option<Vec<Person>>,
    /// When set, replaces the survey date of every imported shot.
    #[serde(default)]
    pub override_date: Option<NaiveDate>,
    pub color: String,
    #[serde(default)]
    pub direction: Option<SurveyDirection>,
    /// Quality factor (1.0 = default). Used for weighted adjustments.
    #[serde(default = "default_line_quality")]
    pub quality: f64,
    pub shots: Vec<ShotData>,
    pub placement: ImportPlacement,
    /// Station comments to apply after line creation.
    /// Each tuple is (station_index, comment_text).
    #[serde(default)]
    pub station_comments: Vec<(u32, String)>,
    /// Station names to apply after line creation, as (station_index, name),
    /// such as the MNemo station numbers of a DMP import. Same index convention
    /// as `station_comments` (see `reverse` below).
    #[serde(default)]
    pub station_names: Vec<(u32, String)>,
    /// Reverse this line end-to-end as it is imported (#24), so the file's
    /// last station becomes station 0.
    ///
    /// A one-shot transform, not persisted state: the backend reverses the
    /// shots *before* creating the line.
    ///
    /// Index conventions, which differ by field:
    /// * `station_comments` and `station_names` indices are given in the FILE's
    ///   original order - the backend mirrors them itself
    ///   (`execute_import_lines`), so an importer never has to think about it.
    /// * `placement` indices must already address the final (reversed) order.
    ///   Placement is a graph of cross-line references the wizard resolves
    ///   while the user is looking at the original numbering, so only the caller
    ///   knows which side of a connection this batch renumbers; see
    ///   `src/utils/reversePlacement.ts`.
    ///
    /// Left/right passage dimensions are NOT swapped on the import path - the
    /// wizard has no dialog moment to ask, and mirroring LRUD without asking
    /// would silently rewrite measurements.
    #[serde(default)]
    pub reverse: bool,
    /// The north reference this line's readings were taken against (#438),
    /// as declared by the source file. `None` - what the frontend wizards send,
    /// since none of them asks the user - leaves the line inheriting the
    /// project default, i.e. no correction.
    ///
    /// It rides on the CONFIG rather than on `LineData` because it is
    /// per-line provenance the importer knows and the create shape does not
    /// carry; `execute_import_lines` writes it onto the line as soon as
    /// the line exists.
    #[serde(default)]
    pub azimuth_reference: Option<AzimuthReference>,
    /// The Section (#327) to group this line into, by name - the importers'
    /// auto-grouping (an Ariane section name that fanned out into several
    /// lines, a Compass survey, a Walls book). The import reuses the
    /// project's Section of that name, or creates one. `None` (what the
    /// wizards send) leaves the line ungrouped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cave_section: Option<String>,
}

/// Update to apply to an existing line's shot during batch import.
/// Used for overlap interpolation: when an import line overlaps with an
/// existing project line, the overlap shots on the existing side get
/// updated with interpolated values as part of the same atomic transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExistingShotUpdate {
    pub line_id: LineId,
    pub shot_id: ShotId,
    pub updates: ShotUpdates,
}

/// Placement strategy for an imported line.
///
/// `export_to` without `export`: the file itself is still written only by the
/// gen_ipc_types example (which takes the file name), but the path must use the
/// member-crate base so `stalagmight-parsers`' export test for
/// `GpxLinePlacement` can compute the sibling `./ImportPlacement` import -
/// ts-rs diffs the two `EXPORT_TO` strings, and mixing the root-crate
/// `bindings/` base with the member `../../bindings/` base makes that diff
/// unrepresentable (the export test panics).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
    /// Merge a station with a station in another line being imported in the same batch.
    FromImporting {
        import_station: u32,
        target_line_index: u32,
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

/// A loop closure to create during import, referencing lines by their import index.
#[derive(Debug, Clone)]
pub struct ImportClosure {
    pub from_line_index: u32,
    pub from_station_index: u32,
    pub to_line_index: u32,
    pub to_station_index: u32,
}

impl Default for LineData {
    fn default() -> Self {
        Self {
            name: None,
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            quality: default_line_quality(),
            shots: Vec::new(),
        }
    }
}

fn default_line_quality() -> f64 {
    1.0
}

impl Line {
    /// Create a new line from line data
    pub fn new(id: LineId, data: LineData) -> Self {
        let template = crate::models::SharedCredit::new(SurveyCredit::from_legacy(
            data.surveyor.as_deref(),
            data.date,
        ));
        // A shot-less line keeps the legacy pair: no shot can carry it yet.
        let shotless = data.shots.is_empty();
        let mut line = Self {
            id,
            // Line names are stored trimmed (`Main ` is `Main`); a blank one
            // is no name, which the normalization seeds.
            name: data
                .name
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty()),
            legacy_surveyor: data.surveyor.filter(|_| shotless),
            legacy_date: data.date.filter(|_| shotless),
            color: data.color,
            direction: data.direction.unwrap_or_default(),
            quality: if data.quality.is_finite() && data.quality > 0.0 {
                data.quality
            } else {
                default_line_quality()
            },
            shots: Vec::new(),
            next_shot_id: id * FEATURE_ID_STRIDE,
            reverse_parity: false,
            gauge_shot_uuid: None,
            gauge_neighbour_uuid: None,
            azimuth_reference: None,
            section_id: None,
            loaded_legacy_surveyor: None,
            loaded_created_at: None,
        };

        // In-line UUID uniqueness, enforced where a `Vec<ShotData>` becomes
        // shots. This is the only door for a whole shot VECTOR - a TML import, a
        // line split, a project merge and a line-level undo restore all
        // come through here. It is NOT the only door into a line: the
        // single-shot inserts ([`Self::add_shot`],
        // [`Self::insert_shot_after_with_id`], [`Self::insert_shot_before_with_id`])
        // and the redo of a created shot (`core::state::undo`) build a `Shot`
        // and push it directly, bypassing this loop.
        //
        // What keeps those safe is a property of this function, not of them: a
        // FOREIGN id can only enter a line HERE, so every identity they can
        // introduce is either a fresh v4 (no collision possible) or one this
        // line already de-duped. The IPC `create_shot` clears `uuid` at the
        // boundary so a caller cannot smuggle one in, and the redo of a created
        // shot refuses a uuid the line already holds. A NEW path that
        // admits a foreign id one shot at a time would break that property and
        // needs its own answer - do not read this as blanket cover.
        //
        // `ShotData::uuid` can now carry a foreign id (#423), and a source file
        // is free to repeat one inside a line. A repeat makes `resolve_key`
        // mis-resolve: it takes the FIRST match, so a merge/closure/datum
        // anchored on the second station silently snaps to the first. Repairing
        // here rather than after the fact means the duplicate never exists in
        // the project, so no cross-reference can be built against it in the
        // window between creation and repair.
        //
        // Policy matches `Project::ensure_unique_shot_uuids` (the pre-save
        // backstop): the FIRST occurrence keeps the id, later ones are cleared
        // and get a fresh mint from `Shot::new`. A repeat ACROSS lines is not
        // visible from here; every door that creates lines from foreign ids
        // settles it once the lines exist
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
            // The legacy line-level surveyor/date is a template for the
            // shots that carry no credit of their own, SHARED by all of them.
            let inherits = shot_data.credit().is_empty();
            line.add_shot(shot_data);
            if inherits {
                if let Some(shot) = line.shots.last_mut() {
                    shot.set_shared_credit(&template);
                }
            }
        }
        if repaired > 0 {
            // Losing a foreign identity is exactly the kind of thing that is
            // invisible until someone asks why a round trip changed ids.
            tracing::warn!(
                line_id = id,
                repaired,
                purpose = "in-line shot uuid uniqueness (#423)",
                "Line carried repeated (or empty) shot UUIDs; kept the first \
                 occurrence and minted fresh identities for the rest"
            );
        }

        line
    }

    /// Create a new line with a specific ID (for undo restoration)
    pub fn new_with_id(id: LineId, data: LineData) -> Self {
        Self::new(id, data)
    }

    /// Restore a line with a specific ID *and* the identity
    /// [`LineData`] cannot carry (issue #413, finding 1).
    ///
    /// [`Self::new`] mints a fresh uuid per shot and hardcodes
    /// `reverse_parity: false` with both gauge anchors `None`, which is right
    /// for a line being CREATED and wrong for one being PUT BACK: the uuid is
    /// what `StationKey::AfterShot` anchors and the Ariane export resolve
    /// through, and the gauge pair pins the component's absolute position.
    ///
    /// A shot with no recorded uuid (`identity.shot_uuids` shorter than the
    /// line, i.e. a legacy entry) keeps the fresh mint - the pre-fix
    /// behaviour.
    pub fn restore_with_identity(id: LineId, data: LineData, identity: &LineIdentity) -> Self {
        let mut line = Self::new(id, data);
        for (shot, uuid) in line.shots.iter_mut().zip(identity.shot_uuids.iter()) {
            shot.uuid.clone_from(uuid);
        }
        line.reverse_parity = identity.reverse_parity;
        line.gauge_shot_uuid.clone_from(&identity.gauge_shot_uuid);
        line.gauge_neighbour_uuid
            .clone_from(&identity.gauge_neighbour_uuid);
        line.azimuth_reference = identity.azimuth_reference;
        line.section_id = identity.section_id;
        line
    }

    /// Convert to LineData (for undo snapshots)
    pub fn to_line_data(&self) -> LineData {
        LineData {
            name: self.name.clone(),
            // The credit rides on the shots; only a shot-less line still
            // holds a legacy pair, which is then its template.
            surveyor: self.legacy_surveyor.clone(),
            date: self.legacy_date,
            color: self.color.clone(),
            direction: Some(self.direction),
            quality: self.quality,
            shots: self.shots.iter().map(Shot::to_shot_data).collect(),
        }
    }

    /// How many more shot ids this line can hand out in this session.
    /// They come from `next_shot_id`, which only grows (a delete does not give
    /// its id back), and must stay inside the line's id window
    /// (`id * FEATURE_ID_STRIDE ..`) or they take the next line's ids. A
    /// reload renumbers every line compactly.
    pub fn shot_ids_left(&self) -> u64 {
        let window_end = (u64::from(self.id) + 1) * u64::from(FEATURE_ID_STRIDE);
        window_end.saturating_sub(u64::from(self.next_shot_id))
    }

    /// Add a shot to the end of this line
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

    /// Apply partial updates to this line
    pub fn apply_updates(&mut self, updates: &LineUpdates) {
        if let Some(v) = &updates.name {
            // Stored trimmed, as `Line::new` stores it.
            self.name = v
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string);
        }
        if self.shots.is_empty() {
            // No shot to carry them: the line keeps the legacy pair.
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
                self.quality = default_line_quality();
            }
        }
    }

    /// [`LineUpdates::surveyor`]: the legacy free text set on every shot.
    /// The surveyors are replaced by the text's; the explorers only when the
    /// text is in the Ariane `"explorers / surveyors"` form, so a plain name
    /// typed into a legacy Surveyor field leaves every shot's explorers alone.
    fn set_legacy_surveyor_on_every_shot(&mut self, text: Option<&str>) {
        let text = text.unwrap_or_default();
        let has_explorer_half = super::legacy_halves(text).is_some();
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

    /// Who surveyed and explored this line and when, aggregated over its
    /// shots: each person with the number of shots that credit them, in
    /// first-appearance order, and the earliest and latest shot date.
    pub fn credit_summary(&self) -> LineCreditSummary {
        LineCreditSummary::from_credits(self.shots.iter().map(Shot::credit))
    }

    /// The distinct surveyor names of every shot, in first-appearance order,
    /// joined with `", "`; `None` when no shot names one. What the plain-text
    /// consumers carry.
    pub fn surveyor_names(&self) -> Option<String> {
        super::join_names(&self.credit_summary().surveyor_people())
    }

    /// The legacy `sections.surveyor` text ([`super::legacy_credit_text`]):
    /// the line's explorers and surveyors in first-appearance order, in the
    /// `"explorers / surveyors"` form when anyone explored it and as the plain
    /// surveyor list otherwise, each display name once. Older builds keep only
    /// this when they save, and SpeleoDB reads only this (#718); the people
    /// themselves live in the per-shot tables.
    pub fn legacy_surveyor_text(&self) -> Option<String> {
        let summary = self.credit_summary();
        super::legacy_credit_text(&summary.explorer_people(), &summary.surveyor_people())
    }

    /// What a save writes to `sections.surveyor` for a line with shots:
    /// the text the line was read with while its people are unchanged
    /// since ([`Self::loaded_legacy_surveyor`]), else
    /// [`Self::legacy_surveyor_text`]. A long or differently spelled legacy
    /// text is therefore not cut or respelled by a save that changed nobody's
    /// credit.
    pub fn legacy_surveyor_to_write(&self) -> Option<String> {
        let derived = self.legacy_surveyor_text();
        match &self.loaded_legacy_surveyor {
            Some(loaded) if loaded.derived == derived => Some(loaded.raw.clone()),
            _ => derived,
        }
    }

    /// Remember `raw`, the `sections.surveyor` text the line was just read
    /// with, if it is not what this build would write for the line now
    /// (see [`Self::loaded_legacy_surveyor`]). Call once the shots carry their
    /// credit.
    pub fn remember_loaded_legacy_surveyor(&mut self, raw: &str) {
        let derived = self.legacy_surveyor_text();
        self.loaded_legacy_surveyor = (derived.as_deref() != Some(raw)).then(|| LoadedLegacyText {
            raw: raw.to_string(),
            derived,
        });
    }

    /// The earliest shot date; what the legacy `sections.date` column carries.
    pub fn first_survey_date(&self) -> Option<NaiveDate> {
        self.shots.iter().filter_map(|s| s.credit().date).min()
    }

    /// Move a legacy line-level surveyor/date (read from an old snapshot
    /// or undo payload into [`Self::legacy_surveyor`]/[`Self::legacy_date`])
    /// onto every shot whose credit is empty, then clear the legacy fields. A
    /// line with no shots keeps them (see [`Self::legacy_surveyor`]).
    /// Returns whether any shot changed. Idempotent.
    pub fn absorb_legacy_credit(&mut self) -> bool {
        if self.shots.is_empty() {
            return false;
        }
        let template = crate::models::SharedCredit::new(self.take_legacy_template());
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

    /// The legacy pair as a credit template, clearing it from the line.
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

    /// Calculate statistics for this line
    ///
    /// Depth is deliberately absent: it is only meaningful as the ABSOLUTE
    /// station range, which needs the computed stations rather than this
    /// line's shots, so it lives on `ProjectStats` alone (#481).
    pub fn stats(&self) -> LineStats {
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

        LineStats {
            shot_count: self.shots.len(),
            total_length,
            jump_shot_count: jump_count,
            reference_shot_count: reference_count,
        }
    }
}
