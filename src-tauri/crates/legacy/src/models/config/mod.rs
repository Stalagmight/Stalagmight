//! Pure domain config types, moved out of `config/` (A3) ahead of the
//! workspace crate split. Everything here is re-exported from `crate::models::config`
//! for compatibility, so every existing `crate::models::config::X` path keeps
//! compiling.

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
