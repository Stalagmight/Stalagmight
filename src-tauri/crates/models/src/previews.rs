//! Preview payloads the dialogs show before a command runs: what deleting a
//! shot, a shot range or a line touches, what setting a zero datum would do,
//! and what merging two placed pieces moves. The station-graph model fills
//! them (`survey::wire::previews`); their serde and TS shapes are the IPC
//! contract.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{DatumRole, LineId, LoopClosureWire, StationMergeWire, StationRef};

/// Relationships that reference a given line (merges, closures, offsets, zero datums).
/// Used by the frontend to show affected relationships before line deletion.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct LineRelationships {
    /// Station merges referencing the line (positional wire form)
    pub merges: Vec<StationMergeWire>,
    /// Loop closures referencing the line (positional wire form)
    pub closures: Vec<LoopClosureWire>,
    /// Whether the line has a line offset
    pub has_line_offset: bool,
    /// Number of station offsets referencing the line
    pub station_offset_count: usize,
    /// Number of zero datums referencing the line
    pub zero_datum_count: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
pub struct StationPair {
    pub station_a: StationRef,
    pub station_b: StationRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ClosurePair {
    pub closure_id: u64,
    pub station_a: StationRef,
    pub station_b: StationRef,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
pub struct ClosureUpdate {
    pub closure_id: u64,
    pub old_station: StationRef,
    pub new_station: StationRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DeleteShotImpact {
    pub removed_station: StationRef,
    pub removed_merges: Vec<StationPair>,
    pub removed_closures: Vec<ClosurePair>,
    pub replacement_merges: Vec<StationPair>,
    pub replacement_closure_updates: Vec<ClosureUpdate>,
    /// Closures broken by the deletion: the surviving endpoints are no longer
    /// connected through the survey, so the closure is replaced by a plain
    /// station merge instead of being kept as a (false) loop. See issue #8.
    #[serde(default)]
    pub closure_to_merge: Vec<StationPair>,
    pub merge_rewire_skipped: bool,
    pub closure_rewire_skipped: bool,
    #[serde(default)]
    pub shift_blocked: bool,
    #[serde(default)]
    pub shift_blocked_reasons: Vec<String>,
}

/// Which structural case a contiguous shot-range deletion falls into. Computed
/// from the range endpoints relative to the line's shot count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RangeDeleteKind {
    /// The range covers every shot - the whole line is removed.
    WholeLine,
    /// The range starts at the first shot (nothing before); the survivors keep
    /// their absolute position by an offset shift.
    Prefix,
    /// The range ends at the last shot (nothing after); the head is truncated.
    Suffix,
    /// Shots remain both before and after - the line can be split (A=before,
    /// B=after) or gap-closed (shift).
    Interior,
}

/// Aggregate impact of deleting a contiguous range of shots `[start_pos..=end_pos]`
/// from one line. The merge/closure fields mirror [`DeleteShotImpact`] (so the
/// frontend can render them with the same component); `kind` drives the dialog
/// routing and `shift_blocked` reports whether the gap-close path is available.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DeleteShotRangeImpact {
    pub line_id: LineId,
    pub start_pos: u32,
    pub end_pos: u32,
    pub kind: RangeDeleteKind,
    /// Stations any offered action removes. For an interior range this is the
    /// gap-close (shift) set `(start_pos+1 ..= end_pos+1)` - the superset that
    /// also covers the split's interior set - so the relationship panel never
    /// hides what closing the gap touches. (Prefix/suffix/whole remove exactly
    /// the gap-close set; only an interior split keeps the boundary `end_pos+1`.)
    pub removed_stations: Vec<StationRef>,
    pub removed_merges: Vec<StationPair>,
    pub removed_closures: Vec<ClosurePair>,
    pub replacement_merges: Vec<StationPair>,
    pub replacement_closure_updates: Vec<ClosureUpdate>,
    #[serde(default)]
    pub closure_to_merge: Vec<StationPair>,
    pub merge_rewire_skipped: bool,
    pub closure_rewire_skipped: bool,
    /// True when the gap-close (shift) path is unavailable for this range (the
    /// two sides are already connected or both anchored, or the gap-close drops
    /// a zero datum).
    #[serde(default)]
    pub shift_blocked: bool,
    #[serde(default)]
    pub shift_blocked_reasons: Vec<String>,
    /// Zero datums the SPLIT would drop (anchored on a strictly-interior station
    /// the split removes) - surfaced so the dialog can warn. The gap-close path
    /// is instead blocked when it would drop a datum, so this set is split-only.
    #[serde(default)]
    pub removed_datums: Vec<StationRef>,
    /// Station comments the SPLIT would drop (anchored on a strictly-interior
    /// station the split removes) - surfaced so the dialog can warn before the
    /// annotation disappears.
    #[serde(default)]
    pub removed_comments: Vec<StationRef>,
    /// Loop closures the GAP-CLOSE would remove because, once the range is
    /// gone, they would tie the two ends of one shot together (#737) - e.g.
    /// closure A:1-A:5 with shots 1..=3 gap-closed would become A:1-A:2. Named
    /// by their stations BEFORE the delete, the ids the user created them with.
    /// Predicted by running the very gap-close the command runs on a clone,
    /// so preview and execution cannot disagree. `None` when no gap-close is offered (the
    /// whole line, or a blocked shift), or when it could not be simulated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub closures_removed_across_one_shot: Option<Vec<ClosurePair>>,
}

/// Fixed part of the gap warning threshold, in metres (#741 decision 4).
pub const DATUM_GAP_WARN_BASE_M: f64 = 15.0;

/// Part of the gap warning threshold that grows with the survey distance from
/// the datum's placement datum: 2% (#741 decision 4).
pub const DATUM_GAP_WARN_FRACTION: f64 = 0.02;

/// The gap above which a datum's GPS and its surveyed position disagree enough
/// to warn: 15 m plus 2% of the survey distance from the placement datum of
/// the datum's piece. An infinite or NaN distance (none reachable) yields the
/// base.
pub fn datum_gap_threshold_m(survey_distance_m: f64) -> f64 {
    if survey_distance_m.is_finite() && survey_distance_m > 0.0 {
        DATUM_GAP_WARN_BASE_M + DATUM_GAP_WARN_FRACTION * survey_distance_m
    } else {
        DATUM_GAP_WARN_BASE_M
    }
}

/// How far a datum's GPS position is from where the survey puts its station,
/// in the frame's local metres. Backend-owned so every UI readout agrees.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export_to = "bindings/DatumGap.ts")]
pub struct DatumGap {
    /// Horizontal distance from the surveyed position to the GPS position.
    pub gap_m: f64,
    /// Bearing of that vector, degrees clockwise from grid north, in [0, 360).
    pub bearing_deg: f64,
    /// Survey distance, along shots and merges, from the placement datum of
    /// this station's merge component (the datum its raw position is carried
    /// from), or `None` when that component has no placement datum.
    pub survey_distance_m: Option<f64>,
    /// [`datum_gap_threshold_m`] for that distance.
    pub threshold_m: f64,
    /// `gap_m > threshold_m`.
    pub exceeds_threshold: bool,
}

/// What setting a zero datum on a station would do, for the Edit Station dialog
/// to show before the user commits (#741).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export_to = "bindings/ZeroDatumPreview.ts")]
pub struct ZeroDatumPreview {
    /// The role the datum would have once stored.
    pub role: DatumRole,
    /// For a [`DatumRole::GpsFix`], how far the GPS is from the surveyed
    /// position. `None` for a placement datum (it moves its group onto the GPS,
    /// so there is no disagreement to show) or when there is no frame yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub gap: Option<DatumGap>,
    /// Whether the datum would count as an entrance: the stored answer for a
    /// station that already holds a datum, else the default rule for `role`
    /// (`Project::datum_is_entrance`), so the dialog's Entrance checkbox
    /// starts from the backend's answer.
    pub is_entrance: bool,
}

/// The piece a merge of two placed pieces moves, and how far.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export_to = "bindings/MergeDatumShift.ts")]
pub struct MergeDatumShift {
    /// Every line of the piece that moves, ascending.
    pub moving_line_ids: Vec<LineId>,
    /// How far it moves, horizontal metres.
    pub shift_m: f64,
    /// The station of its placement datum, which becomes a GPS fix.
    pub datum_station: StationRef,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_is_15_m_plus_2_percent() {
        assert!((datum_gap_threshold_m(1400.0) - 43.0).abs() < 1e-9);
        assert!((datum_gap_threshold_m(0.0) - 15.0).abs() < 1e-9);
        assert!((datum_gap_threshold_m(f64::INFINITY) - 15.0).abs() < 1e-9);
        assert!((datum_gap_threshold_m(f64::NAN) - 15.0).abs() < 1e-9);
    }
}
