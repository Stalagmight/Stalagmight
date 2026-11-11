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

/// Warning code for a merge-connected group anchored by two or more zero datums.
/// This is a structural over-constraint, not a tolerance the surveyor can wave
/// off: it is an **error**, so (like every error) it is not ignorable - the only
/// resolutions are keeping one datum and removing the others, or going to the
/// station to fix it. (See issue #57.)
pub const MULTIPLE_DATUMS_PER_MERGE_COMPONENT: &str = "multiple_datums_per_merge_component";

/// Canonical severity for a validation warning code - the SINGLE source of truth
/// shared by `Validator::validate_shot`, the project-level over-anchoring check,
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
        | "LENGTH_BELOW_DEPTH_CHANGE"
        | MULTIPLE_DATUMS_PER_MERGE_COMPONENT => WarningSeverity::Error,
        // SHORT_SHOT, LONG_SHOT, LARGE_DEPTH_CHANGE, AZIMUTH_DEVIATION,
        // PITCH_DEVIATION and INCLINATION_DEPTH_MISMATCH are advisory; an
        // unknown code defaults to advisory so a future warning is ignorable
        // until classified.
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
    /// For `multiple_datums_per_merge_component` only: the `station_index`
    /// (within `section_id`) of the representative zero datum this row is
    /// attached to. Lets the Resolve view offer "Go to station" without
    /// re-deriving the ambiguous station↔shot mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub datum_station_index: Option<u32>,
    /// For `multiple_datums_per_merge_component` only: every over-anchoring zero
    /// datum in the merge component (one warning is emitted per component). Lets
    /// the Resolve view offer "Keep this one, remove the others" for each datum.
    /// `None` for all other warning codes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub datum_refs: Option<Vec<StationRef>>,
}
