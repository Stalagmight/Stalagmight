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

/// What Detach Line from Station gives the detached line
/// (`preview_detach_line`, review finding UX-4; RS 4.2 and 7): the station
/// the lines that come off get (its name, Station ID and comment), and
/// whether it is one they had before a merge (a record of the junction
/// coming back). With `kept`, the line keeps the junction instead and
/// `name`, `station_id` and `comment` are what the other lines, which were
/// all named by records, get back (I-1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DetachLinePreview {
    pub name: String,
    pub station_id: u32,
    pub restored: bool,
    pub kept: bool,
    /// What every line through the station ends up with there, in slot
    /// order (demo finding V-1).
    #[serde(default)]
    pub lines: Vec<DetachLineResult>,
    /// The comment of the station that comes back ("with its comment").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub comment: Option<String>,
    /// The lines that come off with the detached one because one archived
    /// station names them all (RS 4.2 case 1: they were one station).
    #[serde(default)]
    pub together: Vec<LineId>,
    /// With `kept` and several records naming the other lines: the records
    /// the remaining junction may show (`rest_keep`), and the default (rule
    /// K over their own lines).
    #[serde(default)]
    pub rest_choices: Vec<crate::StationIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rest_default: Option<u32>,
    /// The identity the detached line showed at the junction before, when a
    /// loop closure made it another station's (RS 4.2 case 3: it will show
    /// the junction's own name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shown_before: Option<crate::StationIdentity>,
    /// The names the detach would show twice, each with its two answers.
    /// An earlier letter given because of the junction stays.
    #[serde(default)]
    pub collisions: Vec<crate::NameCollisionWire>,
    /// The slots away from the junction that show another station after
    /// the detach (RS 4.2, M-P2-8): a loop end whose closure's kept end
    /// comes off with the lines, or a station a promotion the detach makes
    /// merges. In line order, then chain order.
    #[serde(default)]
    pub shown_elsewhere: Vec<crate::ShownElsewhereWire>,
    /// The comments the detach hides (RS 12): each identity as it showed it,
    /// with the comment. Undo brings them back.
    #[serde(default)]
    pub comments_hidden: Vec<crate::StationIdentity>,
}

/// One line's result in a [`DetachLinePreview`]: the name it shows its
/// station by after the detach, that station's Station ID, whether the line
/// comes off the junction or stays on it, and the other lines on that station.
/// Lines that come off together stay joined on one station, which has one
/// Station ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DetachLineResult {
    pub line_id: LineId,
    pub name: String,
    pub station_id: u32,
    pub comes_off: bool,
    pub joined_with: Vec<LineId>,
}

/// What deleting one shot does to station relationships
/// (`preview_delete_shot`). The station graph never re-anchors a merge or a
/// closure onto a neighbour, so 0.6.0's `replacement_merges`,
/// `replacement_closure_updates` and `*_rewire_skipped` (always empty) left
/// the wire.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DeleteShotImpact {
    pub removed_station: StationRef,
    pub removed_merges: Vec<StationPair>,
    pub removed_closures: Vec<ClosurePair>,
    /// Closures broken by the deletion: the surviving endpoints are no longer
    /// connected through the survey, so the closure is replaced by a plain
    /// station merge instead of being kept as a (false) loop. See issue #8.
    #[serde(default)]
    pub closure_to_merge: Vec<StationPair>,
    #[serde(default)]
    pub shift_blocked: bool,
    #[serde(default)]
    pub shift_blocked_reasons: Vec<String>,
    /// If the user picks Split instead: the loop closures that become a
    /// junction because the split leaves each the only connection (0.6.0 did
    /// the same), by their two slots before the delete (review finding UX-5).
    #[serde(default)]
    pub split_closure_to_merge: Vec<StationPair>,
    /// If the user picks Split: the largest move of a station, metres.
    #[serde(default)]
    pub split_max_move_m: f64,
    /// What the delete does to identities on its own (RS 4.3, U-P1-3): the
    /// archived stations it brings back where a junction loses its own
    /// lines (with the comment that moves or becomes hidden), and the
    /// letters rule L gives. Absent when it does none of these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub identities: Option<crate::IdentityNotice>,
    /// The same, if the user picks Split instead (an interior delete whose
    /// gap-close is offered): a promotion the split makes may hide a
    /// comment. Absent when Split is not offered or does none of these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub split_identities: Option<crate::IdentityNotice>,
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
    #[serde(default)]
    pub closure_to_merge: Vec<StationPair>,
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
    /// What the delete does to identities on its own (RS 4.3, U-P1-3): the
    /// archived stations it brings back where a junction loses its own
    /// lines (with the comment that moves or becomes hidden), and the
    /// letters rule L gives. Absent when it does none of these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub identities: Option<crate::IdentityNotice>,
    /// The same, if the user picks Split instead (an interior delete whose
    /// gap-close is offered): a promotion the split makes may hide a
    /// comment. Absent when Split is not offered or does none of these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub split_identities: Option<crate::IdentityNotice>,
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

/// What a merge of two unconnected stations does, for the Merge dialog to
/// say before the user confirms (demo issue I-4): the piece that moves, how
/// far, onto which station, and the depths when the two disagree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export_to = "bindings/MergePreview.ts")]
pub struct MergePreview {
    /// Every line of the piece that moves, ascending.
    pub moving_line_ids: Vec<LineId>,
    /// How far it moves to join, horizontal metres (the largest move of any
    /// station the merge moves).
    pub shift_m: f64,
    /// The picked slot of the station that stays put, which the moving piece
    /// joins.
    pub joins: StationRef,
    /// The two stations' depths (A's, then B's, as picked) and the joined
    /// station's, when A's and B's differ by more than
    /// [`crate::JUNCTION_DEPTH_SPREAD_M`]: the joined station takes the mean
    /// of the depth readings, as 0.6.0 did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub depths: Option<MergeDepths>,
    /// Each picked station that is already a junction, by its picked slot,
    /// with the lines that meet there (the picked slot's line first): the
    /// dialog says the merge joins more than the two lines picked (demo
    /// finding V-2). A first, then B.
    #[serde(default)]
    pub junctions: Vec<MergeJunction>,
    /// The two stations the merge may keep, A then B (RS 7: the "Keep"
    /// radios, each with its pill and "shown as K through a loop closure").
    #[serde(default)]
    pub candidates: Vec<crate::KeepCandidate>,
    /// The dialog's default keep (RS 4.1): the closure-forced end, else the
    /// station holding a datum when exactly one does, else rule K.
    #[serde(default)]
    pub default_keep: crate::StationId,
    /// The loop closure between the two whose kept end must be kept (RS 4.1
    /// step 4): the dialog offers no choice and says why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub keep_forced_by: Option<u64>,
    /// What each choice does to names, one per keep the dialog offers (both
    /// stations, or the forced one alone).
    #[serde(default)]
    pub keeps: Vec<crate::MergeKeepPreview>,
}

/// The depths [`MergePreview`] reports, metres (+down).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export_to = "bindings/MergeDepths.ts")]
pub struct MergeDepths {
    /// Station A's depth before the merge.
    pub a_m: f64,
    /// Station B's depth before the merge.
    pub b_m: f64,
    /// The joined station's depth.
    pub joined_m: f64,
    /// Every depth reading the joined station's mean takes, each with the
    /// line it is read on: A's line first, then the other lines at A, then
    /// B's line and the other lines at B (demo finding V-2: at a junction
    /// the two station depths alone did not add up to the mean).
    #[serde(default)]
    pub readings: Vec<MergeDepthReading>,
}

/// One depth reading in [`MergeDepths::readings`], metres (+down).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export_to = "bindings/MergeDepthReading.ts")]
pub struct MergeDepthReading {
    pub line_id: LineId,
    pub depth_m: f64,
}

/// A picked station that is already a junction ([`MergePreview::junctions`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export_to = "bindings/MergeJunction.ts")]
pub struct MergeJunction {
    /// The picked slot.
    pub station: StationRef,
    /// Every line through the station, the slot's line first.
    pub line_ids: Vec<LineId>,
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
