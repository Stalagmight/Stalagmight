//! What the closure-adjusted view could not do, in words the user can act on
//! (#737): the closures it skips because they tie the two ends of one shot,
//! and the components whose solve failed and are shown measured.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{SectionId, Station, StationRef};

use super::closure_shots::ClosureAcrossOneShot;

/// One component the closure-adjusted solve could not adjust; it is shown
/// with its measured geometry instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdjustmentFailure {
    /// The station the component's solve was anchored on.
    pub anchor: StationRef,
    /// Every section of the component, sorted.
    pub section_ids: Vec<SectionId>,
    /// The solver's own error text (kept for the log; see
    /// [`AdjustmentFailure::user_reason`] for what the user is shown).
    pub error: String,
}

/// The internal error the solve reports when a closure joins two
/// datum-anchored groups.
const MULTIPLE_DATUMS: &str = "Multiple zero datums in a single constraint group";
const INCONSISTENT_TRANSLATION: &str =
    "Least squares adjustment produced inconsistent section translation";
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
    /// The section and 0-based index of the shot it collapses.
    pub shot_section_id: SectionId,
    pub shot_index: u32,
}

impl From<&ClosureAcrossOneShot> for SkippedClosure {
    fn from(c: &ClosureAcrossOneShot) -> Self {
        Self {
            closure_id: c.closure_id,
            station_a: c.station_a,
            station_b: c.station_b,
            shot_section_id: c.collapsed_shot.0,
            shot_index: u32::try_from(c.collapsed_shot.1).unwrap_or(u32::MAX),
        }
    }
}

/// A component shown measured because its closure solve failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AdjustmentFailureReport {
    pub anchor: StationRef,
    pub section_ids: Vec<SectionId>,
    /// Plain-language reason ([`AdjustmentFailure::user_reason`]).
    pub reason: String,
}

impl From<&AdjustmentFailure> for AdjustmentFailureReport {
    fn from(f: &AdjustmentFailure) -> Self {
        Self {
            anchor: f.anchor,
            section_ids: f.section_ids.clone(),
            reason: f.user_reason(),
        }
    }
}

/// What the Closures Computed View left out, for the notice above the map
/// (`get_adjustment_report`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct AdjustmentReport {
    /// Closures skipped because they span one shot (cheap; always current).
    pub skipped_closures: Vec<SkippedClosure>,
    /// Components whose solve failed. Only from a solve of the current
    /// project; empty when none has run since the last edit.
    pub failures: Vec<AdjustmentFailureReport>,
}
