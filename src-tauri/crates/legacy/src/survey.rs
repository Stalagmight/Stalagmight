//! What a legacy open returns.

use std::collections::{BTreeMap, BTreeSet};

use crate::models::{
    AdjustmentFailure, AdjustmentReport, ComponentAnchor, ComponentHold, IntegrityViolation,
    LineId, SpeleoLink, Station, StationRef, UnrecognisedShotKind,
};
use crate::persistence::ResolvedWarning;
use crate::LegacyProject;

/// A station slot: the `station_index`-th station along line `line_id`
/// (0 is the line's first station, `shots.len()` its last). The 0.6.0
/// `StationRef`, ordered so position maps iterate deterministically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slot {
    pub line_id: LineId,
    pub station_index: u32,
}

impl From<StationRef> for Slot {
    fn from(sr: StationRef) -> Self {
        Self {
            line_id: sr.line_id,
            station_index: sr.station_index,
        }
    }
}

impl From<Slot> for StationRef {
    fn from(slot: Slot) -> Self {
        StationRef::new(slot.line_id, slot.station_index)
    }
}

/// `[x, y, z]` per slot, metres: x east, y north, z depth (positive down),
/// exactly the `f64`s the 0.6.0 app computed.
pub type Positions = BTreeMap<Slot, [f64; 3]>;

/// Collect `Station` coordinates into a slot map.
pub fn positions_of(stations: &[Station]) -> Positions {
    stations
        .iter()
        .map(|s| {
            (
                Slot::from(s.station_ref),
                [s.coordinates.x, s.coordinates.y, s.coordinates.z],
            )
        })
        .collect()
}

/// The per-file state a 0.6.0 `.stalx` carries outside the `Project` model:
/// what the app's save copies onto every fresh file, besides `save_project`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LegacyExtras {
    /// `display_settings` rows (key -> value).
    pub display_settings: BTreeMap<String, String>,
    /// `resolved_warnings` rows ("Ignore"d warnings), in table order.
    pub resolved_warnings: Vec<ResolvedWarning>,
    /// The SpeleoDB hand-off link row: endpoint, project id and name, base
    /// commit sha, the dirty bit (`has_unpushed_edits`), `last_edit_at`,
    /// `last_verify_at` and the undo mode.
    pub speleo_link: Option<SpeleoLink>,
    /// `speleo_undo_chain`: the Unwind chain position (#303).
    pub speleo_undo_chain: Option<String>,
    /// `speleo_undo_watermark`: the inherited-history watermark (#303).
    pub speleo_undo_watermark: Option<String>,
}

/// How much undo history the file held, counted before any migration ran
/// (several migrations drop it). The legacy reader never loads it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HistoryPresence {
    pub undo_rows: u64,
    pub redo_rows: u64,
}

impl HistoryPresence {
    pub fn any(&self) -> bool {
        self.undo_rows > 0 || self.redo_rows > 0
    }
}

/// What the 0.6.0 load path did to the stored project while settling it.
#[derive(Debug, Clone, Default)]
pub struct LoadReport {
    /// Whether the file stores its own validation thresholds. When it does not,
    /// `project.validation` holds the 0.6.0 hardcoded default, and the app
    /// would have used the user's global thresholds instead.
    pub validation_stored: bool,
    /// Shots whose uuid was missing (or repeated within its line) and was
    /// derived deterministically, as `(line id, uuid)`. The app minted random
    /// v4 ids here; see the crate docs.
    pub derived_shot_uuids: BTreeSet<(LineId, String)>,
    /// Shot uuids the load renamed because several lines held them, as
    /// `(line id, old, new)`.
    pub renamed_shot_uuids: Vec<(LineId, String, String)>,
    /// Line offsets the re-solve changed (`redrive_offsets_after_load`).
    pub healed_offsets: usize,
    /// Shots whose stored kind 0.6.0 could not read (loaded as Reference).
    pub unrecognised_shot_kinds: Vec<UnrecognisedShotKind>,
    /// Whether the v12->v13 migration's one-shot Jump/Reference
    /// classification was pending, and how many shots it changed.
    pub kind_classification_pending: bool,
    pub reclassified_shots: usize,
    /// Integrity findings on the settled project (structural + merge
    /// alignment), as the app's load banner would show them.
    pub integrity: Vec<IntegrityViolation>,
    /// Whether a load step bumped `modified_at`; the reported project keeps
    /// the file's value either way.
    pub touched_modified_at: bool,
}

/// A legacy `.stalx` opened exactly as the 0.6.0 app opened it.
#[derive(Debug, Clone)]
pub struct LegacySurvey {
    /// The settled project: loaded, migrated and healed by the frozen load
    /// path (`modified_at` is the file's).
    pub project: LegacyProject,
    /// The schema version stamped in the file, before any migration.
    pub schema_version: i32,
    /// Measured positions: the app's `compute_station_coordinates()`.
    pub raw_positions: Positions,
    /// Closures Computed View positions (`get_adjusted_stations`), or `None`
    /// when [`crate::OpenOptions::with_adjusted`] is off.
    pub adjusted_positions: Option<Positions>,
    /// The components the adjusted solve could not adjust (shown measured),
    /// with the solver's own error text.
    pub adjustment_failures: Vec<AdjustmentFailure>,
    /// What the app's `get_adjustment_report` would say: closures skipped
    /// because they span one shot, failed components, far GPS fixes.
    pub adjustment_report: AdjustmentReport,
    /// Every connected component and the station the 0.6.0 solver held for
    /// it (placement datum, else held gauge, else default gauge).
    pub component_anchors: Vec<ComponentAnchor>,
    /// Per-file state outside the model.
    pub extras: LegacyExtras,
    /// Undo/redo rows in the file before migration.
    pub history: HistoryPresence,
    /// What the load changed.
    pub load: LoadReport,
}

impl LegacySurvey {
    /// The gauge station of every datum-less component: the station the old
    /// least-squares solve held fixed (held gauge, else default gauge), with
    /// the component's lines.
    pub fn datumless_gauges(&self) -> impl Iterator<Item = &ComponentAnchor> {
        self.component_anchors
            .iter()
            .filter(|anchor| anchor.hold != ComponentHold::PlacementDatum)
    }
}
