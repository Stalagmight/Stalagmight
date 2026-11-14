//! Project warning types.
//!
//! This module contains warning types that are used for validation and
//! displayed in the UI. Moved from commands/validation.rs for reuse
//! across the codebase.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{SectionId, ShotId, StationRef};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum WarningSeverity {
    Warning,
    Error,
}

/// Warning code for a GPS-fix zero datum whose GPS position is further from
/// its surveyed position than 15 m plus 2% of the survey distance to the
/// nearest other datum (#741 decision 4). A sanity check, not a blocker: the
/// survey or the GPS may be off, and the surveyor may know the gap is real, so
/// it is an advisory **warning** and ignorable. One row per such fix.
///
/// (It replaced `multiple_datums_per_merge_component`: several datums in one
/// merge-connected group are valid since #741.)
pub const DATUM_GPS_GAP: &str = "datum_gps_gap";

/// Canonical severity for a validation warning code - the SINGLE source of truth
/// shared by `Validator::validate_shot`, the project-level datum gap check,
/// the issue-severity map coloring, and the resolve/ignore gate. Errors are
/// blocking data problems that must be fixed; warnings are tolerances the
/// surveyor may wave off (issue #57).
pub fn severity_for_code(code: &str) -> WarningSeverity {
    match code {
        "ZERO_LENGTH"
        | "NEGATIVE_LENGTH"
        | "INVALID_AZIMUTH"
        | "INVALID_PITCH"
        | "NEGATIVE_LRUD"
        | "NON_FINITE_VALUE"
        | "LENGTH_BELOW_DEPTH_CHANGE" => WarningSeverity::Error,
        // SHORT_SHOT, LONG_SHOT, LARGE_DEPTH_CHANGE, AZIMUTH_DEVIATION,
        // PITCH_DEVIATION, INCLINATION_DEPTH_MISMATCH and DATUM_GPS_GAP are
        // advisory; an unknown code defaults to advisory so a future warning is
        // ignorable until classified.
        _ => WarningSeverity::Warning,
    }
}

/// Whether a warning of this code may be "ignored" (suppressed) by the user.
/// Only advisory (warning-severity) codes are ignorable; errors must be fixed
/// (issue #57). This is the one predicate the warnings computation and the
/// resolve command both consult, so the policy lives in a single place.
pub fn warning_is_ignorable(code: &str) -> bool {
    severity_for_code(code) == WarningSeverity::Warning
}

/// A project-level warning with reference to the source shot.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ProjectWarning {
    pub section_id: SectionId,
    pub shot_id: ShotId,
    pub shot_index: usize,
    pub code: String,
    pub message: String,
    pub severity: WarningSeverity,
    pub is_resolved: bool,
    /// Stable UUID of the shot this warning is attached to. The basis for the
    /// "Ignore" persistence (issue #57): keying suppression by the shot's
    /// `uuid` rather than the regenerated `shot_id` keeps an ignore bound to
    /// the same shot even after the user inserts, deletes, or reorders shots.
    /// `None` only for shots loaded from pre-v6 .stalx files that have not yet
    /// had a UUID minted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shot_uuid: Option<String>,
    /// For `datum_gps_gap` only: the `station_index` (within `section_id`) of
    /// the GPS-fix datum this row is about. Lets the Resolve view offer "Go to
    /// station" without re-deriving the ambiguous station-to-shot mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub datum_station_index: Option<u32>,
    /// A list of zero-datum stations for a warning about several of them. No
    /// current code sets it (`datum_gps_gap` is per fix and leaves it `None`);
    /// kept so the wire shape does not change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub datum_refs: Option<Vec<StationRef>>,
}
