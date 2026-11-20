//! What the Closures Computed View could not do, in words the user can act on
//! (#737): the closures it skips because they tie the two ends of one shot,
//! the components whose solve failed and are shown measured, and the GPS fixes
//! far from their surveyed position (`get_adjustment_report`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{LineId, StationRef};

/// A closure the Closures Computed View skips because it ties the two ends of
/// one shot together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SkippedClosure {
    /// `LoopClosure::id`. `number` in TS (ids are small counters), so it
    /// compares directly with the warning's `closureId`.
    #[ts(type = "number")]
    pub closure_id: u64,
    pub station_a: StationRef,
    pub station_b: StationRef,
    /// The line and 0-based index of the shot it collapses.
    pub shot_line_id: LineId,
    pub shot_index: u32,
}

/// A component shown measured because its closure solve failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AdjustmentFailureReport {
    pub anchor: StationRef,
    pub line_ids: Vec<LineId>,
    /// Plain-language reason ([`crate::survey::AdjustmentFailure::user_reason`]).
    pub reason: String,
}

/// A GPS fix whose GPS is further from its surveyed position than the gap
/// check expects (#741). The Closures Computed View, and so Plan and Export,
/// bend the survey onto it, so the notice above the map names it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
pub struct FarGpsFix {
    /// The fix's station.
    pub station: StationRef,
    /// [`crate::DatumGap::gap_m`].
    pub gap_m: f64,
    /// [`crate::DatumGap::threshold_m`], which `gap_m` exceeds.
    pub threshold_m: f64,
}

/// What the Closures Computed View left out, for the notice above the map
/// (`get_adjustment_report`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct AdjustmentReport {
    /// Closures skipped because they span one shot (cheap; always current).
    pub skipped_closures: Vec<SkippedClosure>,
    /// Components whose solve failed. Only from a solve of the current
    /// project; empty when none has run since the last edit.
    pub failures: Vec<AdjustmentFailureReport>,
    /// GPS fixes that fail the gap check, largest gap first
    /// ([`crate::survey::Project::far_gps_fixes`]). Always current.
    #[serde(default)]
    pub far_gps_fixes: Vec<FarGpsFix>,
}
