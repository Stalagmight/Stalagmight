//! What the closure-adjusted view could not do, in words the user can act on
//! (#737): the closures it skips because they tie the two ends of one shot,
//! and the components whose solve failed and are shown measured.

use crate::{AdjustmentFailureReport, FarGpsFix, LineId, SkippedClosure, Station, StationRef};

use super::closure_shots::ClosureAcrossOneShot;
use super::Project;

/// One component the closure-adjusted solve could not adjust; it is shown
/// with its measured geometry instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdjustmentFailure {
    /// The station the component's solve was anchored on.
    pub anchor: StationRef,
    /// Every line of the component, sorted.
    pub line_ids: Vec<LineId>,
    /// The solver's own error text (kept for the log; see
    /// [`AdjustmentFailure::user_reason`] for what the user is shown).
    pub error: String,
}

/// The internal error the solve reports when a closure joins two groups
/// placed by their own zero datums (#741: GPS fixes never conflict this way).
const MULTIPLE_DATUMS: &str = "Multiple placement zero datums in a single constraint group";
const INCONSISTENT_TRANSLATION: &str =
    "Least squares adjustment produced inconsistent line translation";
const WOULD_MOVE_DATUM: &str = "Adjustment would move a zero datum station";

impl AdjustmentFailure {
    /// Plain-language reason for the failure. The known solver errors map to
    /// what the surveyor can do about them; anything else gets a generic
    /// sentence (the raw text stays in the log).
    pub fn user_reason(&self) -> String {
        let reason = match self.error.as_str() {
            MULTIPLE_DATUMS => {
                "loop closures tie two zero datums to the same point, and both positions are fixed. Remove one of those closures or one of the datums."
            }
            INCONSISTENT_TRANSLATION => {
                "the loop closures here could not be solved consistently. Check the closures in this part of the survey."
            }
            WOULD_MOVE_DATUM => {
                "the adjustment would have moved a zero datum. Check the closures and datums in this part of the survey."
            }
            _ => "the loop-closure solve failed. Check the closures in this part of the survey.",
        };
        reason.to_string()
    }
}

/// The closure-adjusted station coordinates plus the components that could
/// not be adjusted.
#[derive(Debug, Clone)]
pub struct AdjustedStations {
    pub stations: Vec<Station>,
    pub failures: Vec<AdjustmentFailure>,
}

impl From<&ClosureAcrossOneShot> for SkippedClosure {
    fn from(c: &ClosureAcrossOneShot) -> Self {
        Self {
            closure_id: c.closure_id,
            station_a: c.station_a,
            station_b: c.station_b,
            shot_line_id: c.collapsed_shot.0,
            shot_index: u32::try_from(c.collapsed_shot.1).unwrap_or(u32::MAX),
        }
    }
}

impl From<&AdjustmentFailure> for AdjustmentFailureReport {
    fn from(f: &AdjustmentFailure) -> Self {
        Self {
            anchor: f.anchor,
            line_ids: f.line_ids.clone(),
            reason: f.user_reason(),
        }
    }
}

impl Project {
    /// Every GPS fix whose gap exceeds its threshold
    /// ([`crate::DatumGap::exceeds_threshold`]), largest gap first, ties by
    /// station. The computed view is bent onto each of them.
    pub fn far_gps_fixes(&self) -> Vec<FarGpsFix> {
        let mut far: Vec<FarGpsFix> = self
            .gps_fix_gaps()
            .into_iter()
            .filter(|(_, gap)| gap.exceeds_threshold)
            .map(|(station, gap)| FarGpsFix {
                station,
                gap_m: gap.gap_m,
                threshold_m: gap.threshold_m,
            })
            .collect();
        far.sort_by(|a, b| {
            b.gap_m
                .total_cmp(&a.gap_m)
                .then_with(|| a.station.line_id.cmp(&b.station.line_id))
                .then_with(|| a.station.station_index.cmp(&b.station.station_index))
        });
        far
    }
}
