//! Pure domain config types, moved out of `config/` (A3) ahead of the
//! workspace crate split. Everything here is re-exported from `crate::config`
//! for compatibility, so every existing `crate::config::X` path keeps
//! compiling.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

mod gas;
pub use gas::*;

fn default_azimuth_diff_threshold() -> f64 {
    20.0
}

fn default_pitch_diff_threshold() -> f64 {
    10.0
}

/// Default `ValidationConfig::inclination_depth_diff_threshold` (degrees).
///
/// Named and exported (not just a serde-default closure) because the TML
/// importer's zero-means-absent pitch inference
/// (`parsers::tml::normalize_tml_shot`, #482) needs the SAME threshold the
/// validator's `INCLINATION_DEPTH_MISMATCH` check uses, deliberately reading
/// this default rather than the user's runtime `ValidationConfig` - the
/// import mapping must be deterministic across a config change, not track it.
pub const DEFAULT_INCLINATION_DEPTH_DIFF_THRESHOLD: f64 = 15.0;

fn default_inclination_depth_diff_threshold() -> f64 {
    DEFAULT_INCLINATION_DEPTH_DIFF_THRESHOLD
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ValidationConfig {
    pub warn_shot_length_min: f64,
    pub warn_shot_length_max: f64,
    pub warn_depth_change_per_shot_max: f64,
    pub warn_azimuth_deviation_max: f64,
    /// Threshold for azimuth in/out difference warning (degrees). Both readings
    /// are forward azimuths, so they should be similar: warn when they differ by
    /// more than this after 360-degree wraparound normalisation (see
    /// `core::validation`).
    #[serde(default = "default_azimuth_diff_threshold")]
    pub azimuth_diff_threshold: f64,
    /// Threshold for the PITCH_DEVIATION warning (degrees) - compares the two
    /// recorded inclination legs against each other, when both are recorded.
    #[serde(default = "default_pitch_diff_threshold")]
    pub pitch_diff_threshold: f64,
    /// Threshold (degrees) for the INCLINATION_DEPTH_MISMATCH warning: how far
    /// the recorded inclination may differ from the inclination implied by the
    /// shot's length and depth change before it is flagged. Distinct from
    /// `pitch_diff_threshold` (which compares the in/out legs to each other).
    #[serde(default = "default_inclination_depth_diff_threshold")]
    pub inclination_depth_diff_threshold: f64,
}

impl Default for ValidationConfig {
    fn default() -> Self {
        Self {
            // Flag suspiciously short shots (under 0.3 m by default; configurable
            // via the `[validation]` config). Surfaces as the SHORT_SHOT warning.
            warn_shot_length_min: 0.3,
            warn_shot_length_max: 50.0,
            warn_depth_change_per_shot_max: 10.0,
            warn_azimuth_deviation_max: 30.0,
            azimuth_diff_threshold: 20.0,
            pitch_diff_threshold: 10.0,
            inclination_depth_diff_threshold: DEFAULT_INCLINATION_DEPTH_DIFF_THRESHOLD,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClosureAdjustmentConfig {
    #[serde(default = "default_sigma_length_m")]
    pub base_sigma_length_m: f64,
    #[serde(default = "default_sigma_azimuth_deg")]
    pub base_sigma_azimuth_deg: f64,
    #[serde(default = "default_sigma_depth_m")]
    pub base_sigma_depth_m: f64,
}

impl ClosureAdjustmentConfig {
    pub fn normalize(&mut self) {
        if !self.base_sigma_length_m.is_finite() || self.base_sigma_length_m <= 0.0 {
            self.base_sigma_length_m = default_sigma_length_m();
        }
        if !self.base_sigma_azimuth_deg.is_finite() || self.base_sigma_azimuth_deg <= 0.0 {
            self.base_sigma_azimuth_deg = default_sigma_azimuth_deg();
        }
        if !self.base_sigma_depth_m.is_finite() || self.base_sigma_depth_m <= 0.0 {
            self.base_sigma_depth_m = default_sigma_depth_m();
        }
    }
}

impl Default for ClosureAdjustmentConfig {
    fn default() -> Self {
        Self {
            base_sigma_length_m: default_sigma_length_m(),
            base_sigma_azimuth_deg: default_sigma_azimuth_deg(),
            base_sigma_depth_m: default_sigma_depth_m(),
        }
    }
}

pub fn default_sigma_length_m() -> f64 {
    0.05
}

pub fn default_sigma_azimuth_deg() -> f64 {
    1.0
}

pub fn default_sigma_depth_m() -> f64 {
    0.05
}

/// Placement suggestion algorithm configuration.
/// Controls how imported DMP sections are matched against existing project data.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PlacementSuggestionConfig {
    /// Sigma for azimuth error (degrees)
    #[serde(default = "default_ps_azimuth_sigma")]
    pub azimuth_sigma: f64,
    /// Sigma for length error (meters)
    #[serde(default = "default_ps_length_sigma")]
    pub length_sigma: f64,
    /// Sigma for depth error (meters)
    #[serde(default = "default_ps_depth_sigma")]
    pub depth_sigma: f64,
    /// Weight for azimuth score component
    #[serde(default = "default_ps_azimuth_weight")]
    pub azimuth_weight: f64,
    /// Weight for length score component
    #[serde(default = "default_ps_length_weight")]
    pub length_weight: f64,
    /// Weight for depth_in score component
    #[serde(default = "default_ps_depth_in_weight")]
    pub depth_in_weight: f64,
    /// Weight for depth_out score component
    #[serde(default = "default_ps_depth_out_weight")]
    pub depth_out_weight: f64,
    /// Minimum per-shot score to continue matching
    #[serde(default = "default_ps_min_shot_score")]
    pub min_shot_score: f64,
    /// Minimum aggregate score to include a suggestion
    #[serde(default = "default_ps_min_aggregate_score")]
    pub min_aggregate_score: f64,
    /// Maximum number of reference shots to try from each leaf
    #[serde(default = "default_ps_max_reference_shots")]
    pub max_reference_shots: u32,
    /// Maximum number of suggestions to return
    #[serde(default = "default_ps_max_suggestions")]
    pub max_suggestions: u32,
    /// Minimum partial shot length ratio
    #[serde(default = "default_ps_min_partial_ratio")]
    pub min_partial_ratio: f64,
    /// Short shot threshold (meters) - shots below this get reduced weight
    #[serde(default = "default_ps_short_shot_threshold")]
    pub short_shot_threshold: f64,
    /// Penalty multiplier for single-shot matches (0.0-1.0)
    #[serde(default = "default_ps_single_shot_penalty")]
    pub single_shot_penalty: f64,
    /// Hard cutoff: max azimuth error for full shots (degrees)
    #[serde(default = "default_ps_max_azimuth_error_full")]
    pub max_azimuth_error_full: f64,
    /// Hard cutoff: max azimuth error for partial shots (degrees)
    #[serde(default = "default_ps_max_azimuth_error_partial")]
    pub max_azimuth_error_partial: f64,
    /// Hard cutoff: max depth error per measurement point (meters)
    #[serde(default = "default_ps_max_depth_error")]
    pub max_depth_error: f64,
    /// Length error fraction for dynamic cutoff
    #[serde(default = "default_ps_max_length_error_fraction")]
    pub max_length_error_fraction: f64,
    /// Length error floor for dynamic cutoff (meters)
    #[serde(default = "default_ps_max_length_error_floor")]
    pub max_length_error_floor: f64,
}

impl PlacementSuggestionConfig {
    /// Normalize all values to valid ranges, resetting NaN/Inf to defaults.
    pub fn normalize(&mut self) {
        fn fix(val: &mut f64, default: f64, min: f64, max: f64) {
            if !val.is_finite() {
                *val = default;
            }
            *val = val.clamp(min, max);
        }

        fix(
            &mut self.azimuth_sigma,
            default_ps_azimuth_sigma(),
            0.1,
            180.0,
        );
        fix(
            &mut self.length_sigma,
            default_ps_length_sigma(),
            0.01,
            100.0,
        );
        fix(&mut self.depth_sigma, default_ps_depth_sigma(), 0.01, 100.0);
        fix(
            &mut self.azimuth_weight,
            default_ps_azimuth_weight(),
            0.0,
            1.0,
        );
        fix(
            &mut self.length_weight,
            default_ps_length_weight(),
            0.0,
            1.0,
        );
        fix(
            &mut self.depth_in_weight,
            default_ps_depth_in_weight(),
            0.0,
            1.0,
        );
        fix(
            &mut self.depth_out_weight,
            default_ps_depth_out_weight(),
            0.0,
            1.0,
        );
        fix(
            &mut self.min_shot_score,
            default_ps_min_shot_score(),
            0.0,
            1.0,
        );
        fix(
            &mut self.min_aggregate_score,
            default_ps_min_aggregate_score(),
            0.0,
            1.0,
        );
        self.max_reference_shots = self.max_reference_shots.clamp(1, 50);
        self.max_suggestions = self.max_suggestions.clamp(1, 100);
        fix(
            &mut self.min_partial_ratio,
            default_ps_min_partial_ratio(),
            0.0,
            1.0,
        );
        fix(
            &mut self.short_shot_threshold,
            default_ps_short_shot_threshold(),
            0.0,
            100.0,
        );
        fix(
            &mut self.single_shot_penalty,
            default_ps_single_shot_penalty(),
            0.0,
            1.0,
        );
        fix(
            &mut self.max_azimuth_error_full,
            default_ps_max_azimuth_error_full(),
            0.0,
            180.0,
        );
        fix(
            &mut self.max_azimuth_error_partial,
            default_ps_max_azimuth_error_partial(),
            0.0,
            180.0,
        );
        fix(
            &mut self.max_depth_error,
            default_ps_max_depth_error(),
            0.0,
            100.0,
        );
        fix(
            &mut self.max_length_error_fraction,
            default_ps_max_length_error_fraction(),
            0.0,
            1.0,
        );
        fix(
            &mut self.max_length_error_floor,
            default_ps_max_length_error_floor(),
            0.0,
            100.0,
        );
    }
}

impl Default for PlacementSuggestionConfig {
    fn default() -> Self {
        Self {
            azimuth_sigma: default_ps_azimuth_sigma(),
            length_sigma: default_ps_length_sigma(),
            depth_sigma: default_ps_depth_sigma(),
            azimuth_weight: default_ps_azimuth_weight(),
            length_weight: default_ps_length_weight(),
            depth_in_weight: default_ps_depth_in_weight(),
            depth_out_weight: default_ps_depth_out_weight(),
            min_shot_score: default_ps_min_shot_score(),
            min_aggregate_score: default_ps_min_aggregate_score(),
            max_reference_shots: default_ps_max_reference_shots(),
            max_suggestions: default_ps_max_suggestions(),
            min_partial_ratio: default_ps_min_partial_ratio(),
            short_shot_threshold: default_ps_short_shot_threshold(),
            single_shot_penalty: default_ps_single_shot_penalty(),
            max_azimuth_error_full: default_ps_max_azimuth_error_full(),
            max_azimuth_error_partial: default_ps_max_azimuth_error_partial(),
            max_depth_error: default_ps_max_depth_error(),
            max_length_error_fraction: default_ps_max_length_error_fraction(),
            max_length_error_floor: default_ps_max_length_error_floor(),
        }
    }
}

pub fn default_ps_azimuth_sigma() -> f64 {
    18.0
}
pub fn default_ps_length_sigma() -> f64 {
    0.8
}
pub fn default_ps_depth_sigma() -> f64 {
    0.35
}
pub fn default_ps_azimuth_weight() -> f64 {
    0.35
}
pub fn default_ps_length_weight() -> f64 {
    0.25
}
pub fn default_ps_depth_in_weight() -> f64 {
    0.20
}
pub fn default_ps_depth_out_weight() -> f64 {
    0.20
}
pub fn default_ps_min_shot_score() -> f64 {
    0.15
}
pub fn default_ps_min_aggregate_score() -> f64 {
    0.45
}
pub fn default_ps_max_reference_shots() -> u32 {
    5
}
pub fn default_ps_max_suggestions() -> u32 {
    10
}
pub fn default_ps_min_partial_ratio() -> f64 {
    0.15
}
pub fn default_ps_short_shot_threshold() -> f64 {
    0.5
}
pub fn default_ps_single_shot_penalty() -> f64 {
    0.6
}
pub fn default_ps_max_azimuth_error_full() -> f64 {
    25.0
}
pub fn default_ps_max_azimuth_error_partial() -> f64 {
    35.0
}
pub fn default_ps_max_depth_error() -> f64 {
    0.30
}
pub fn default_ps_max_length_error_fraction() -> f64 {
    0.03
}
pub fn default_ps_max_length_error_floor() -> f64 {
    0.4
}
