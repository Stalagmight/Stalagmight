use chrono::NaiveDate;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::str::FromStr;
use ts_rs::TS;

use super::{
    AzimuthReference, Coordinates, GpsCoordinates, Person, ShotData, ShotId, ShotUpdates,
    StationRef,
};

/// Unique identifier for a line
pub type LineId = u32;

/// What the user reads when an edit addresses a line by a handle the project
/// no longer holds (deleted since, a stale view): never the internal id,
/// which the user never sees (decision 2 of the line-identity contract).
pub const LINE_GONE: &str = "That line no longer exists.";

/// [`LINE_GONE`] for a shot.
pub const SHOT_GONE: &str = "That shot no longer exists.";

/// What a line without a name is called in user-facing text
/// (the line label of history rows, refusals and the Combine dialog).
pub const UNNAMED_LINE: &str = "unnamed line";

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

/// Data for creating a new line
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct LineData {
    pub name: Option<String>,
    /// LEGACY template: free text split by [`crate::legacy_people`] and given, with
    /// [`Self::date`], to every shot whose credit is empty when the line is
    /// built. The line itself keeps no surveyor.
    pub surveyor: Option<String>,
    /// LEGACY template date, applied with [`Self::surveyor`].
    #[ts(type = "string | null")]
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
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
pub struct LineUpdates {
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
    pub name: Option<Option<String>>,
    /// LEGACY: set on EVERY shot of the line - the surveyors (and, for the
    /// Ariane `"explorers / surveyors"` form, the explorers) parsed by
    /// [`crate::legacy_people`]. `null` clears every shot's surveyors.
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional)]
    pub surveyor: Option<Option<String>>,
    /// LEGACY: set on EVERY shot of the line; `null` clears every shot's date.
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

/// Line statistics (computed, not stored)
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
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
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct SegmentLineGeometry {
    pub line_id: LineId,
    pub color: String,
    /// Shot lines as pairs of [x, y] coords in meters, relative to the anchor line's station 0
    pub shot_lines: Vec<[[f64; 2]; 2]>,
}

/// Shot-line geometry for all lines in a segment, positioned relative to a given line.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct SegmentGeometry {
    pub lines: Vec<SegmentLineGeometry>,
}

/// Configuration for a single line being imported via the batch import command.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ImportLineConfig {
    /// Index of this line in the import array (used for fromImporting references)
    pub line_index: u32,
    pub name: Option<String>,
    /// LEGACY template, as [`LineData::surveyor`]: given to every imported
    /// shot whose credit is empty.
    pub surveyor: Option<String>,
    /// LEGACY template date, as [`LineData::date`].
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
    #[ts(optional)]
    pub azimuth_reference: Option<AzimuthReference>,
    /// The Section (#327) to group this line into, by name - the importers'
    /// auto-grouping (an Ariane section name that fanned out into several
    /// lines, a Compass survey, a Walls book). The import reuses the
    /// project's Section of that name, or creates one. `None` (what the
    /// wizards send) leaves the line ungrouped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cave_section: Option<String>,
}

/// Update to apply to an existing line's shot during batch import.
/// Used for overlap interpolation: when an import line overlaps with an
/// existing project line, the overlap shots on the existing side get
/// updated with interpolated values as part of the same atomic transaction.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
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

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

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

    #[test]
    fn test_line_data_default() {
        let data = LineData::default();
        assert!(data.name.is_none());
        assert!(data.surveyor.is_none());
        assert!(data.date.is_none());
        assert_eq!(data.color, "#4ecdc4");
        assert!(data.direction.is_none());
        assert!(data.shots.is_empty());
    }

    #[test]
    fn test_line_updates_default() {
        let updates = LineUpdates::default();

        assert!(updates.name.is_none());
        assert!(updates.surveyor.is_none());
        assert!(updates.date.is_none());
        assert!(updates.color.is_none());
        assert!(updates.direction.is_none());
    }

    #[test]
    fn test_line_updates_serde() {
        let updates = LineUpdates {
            name: Some(Some("Updated Name".to_string())),
            color: Some("#aabbcc".to_string()),
            ..Default::default()
        };

        let json = serde_json::to_string(&updates).unwrap();
        let deserialized: LineUpdates = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, Some(Some("Updated Name".to_string())));
        assert_eq!(deserialized.color, Some("#aabbcc".to_string()));
        assert!(deserialized.surveyor.is_none());
    }

    #[test]
    fn test_line_stats_serde() {
        let stats = LineStats {
            shot_count: 5,
            total_length: 42.5,
            jump_shot_count: 2,
            reference_shot_count: 1,
        };

        let json = serde_json::to_string(&stats).unwrap();
        let deserialized: LineStats = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.shot_count, 5);
        assert_eq!(deserialized.total_length, 42.5);
        assert_eq!(deserialized.jump_shot_count, 2);
        assert_eq!(deserialized.reference_shot_count, 1);
    }
}
