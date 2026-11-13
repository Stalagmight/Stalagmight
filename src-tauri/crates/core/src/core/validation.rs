use crate::config::ValidationConfig;
use crate::models::shot::implied_pitch_deg;
use crate::models::warning::severity_for_code;
use crate::models::{SectionId, ShotData};

use super::{ValidationWarning, WarningSeverity};

/// Valid range for a recorded inclination/pitch, in degrees. A value outside
/// [-90, 90] is physically impossible (issue #57).
const PITCH_MIN: f64 = -90.0;
const PITCH_MAX: f64 = 90.0;

/// Rounding tolerance, in metres, absorbed by the length-vs-depth-change
/// geometry check below (`LENGTH_BELOW_DEPTH_CHANGE`). Purely a slope/dz
/// float-rounding slack, unrelated to any other decision in this codebase -
/// do not reuse it as a "how much depth change counts as real" threshold
/// elsewhere (#482 finding: the importer's zero-means-absent pitch inference
/// needs the DIFFERENT question "would this fabricate a mismatch warning",
/// answered by `implied_pitch_deg` + `DEFAULT_INCLINATION_DEPTH_DIFF_THRESHOLD`).
const DEPTH_ROUNDING_EPSILON_M: f64 = 1e-6;

/// Validator for survey data
pub struct Validator;

impl Validator {
    /// Validate a shot and return any warnings
    pub fn validate_shot(
        shot: &ShotData,
        config: &ValidationConfig,
        section_id: SectionId,
        shot_index: usize,
    ) -> Vec<ValidationWarning> {
        let mut warnings = Vec::new();

        // Build every warning through one helper so its severity is sourced from
        // the single `severity_for_code` map (issue #57) - the validator, the
        // ignore gate, and the map coloring can never disagree on a code's class.
        let warn = |code: &str, message: String| ValidationWarning {
            code: code.to_string(),
            message,
            severity: severity_for_code(code),
        };

        // Non-finite backstop (#275 B4): NaN fails EVERY range comparison in
        // this function, so without an explicit check a NaN/inf field would
        // produce zero warnings while silently poisoning station coordinates
        // and the closure solve. Parsers coerce non-finite input at ingestion;
        // this guards any path that slips through.
        let non_finite: Vec<&str> = [
            ("length", Some(shot.length)),
            ("azimuth_in", Some(shot.azimuth_in)),
            ("azimuth_out", Some(shot.azimuth_out)),
            ("depth_in", Some(shot.depth_in)),
            ("depth_out", Some(shot.depth_out)),
            ("pitch_in", shot.pitch_in),
            ("pitch_out", shot.pitch_out),
            ("left", shot.left),
            ("right", shot.right),
            ("up", shot.up),
            ("down", shot.down),
        ]
        .iter()
        .filter(|(_, v)| v.is_some_and(|v| !v.is_finite()))
        .map(|(name, _)| *name)
        .collect();
        if !non_finite.is_empty() {
            warnings.push(warn(
                "NON_FINITE_VALUE",
                format!(
                    "Shot {}:{} has non-finite value(s) in: {}",
                    section_id,
                    shot_index + 1,
                    non_finite.join(", ")
                ),
            ));
        }

        // Check for zero length
        if shot.length == 0.0 {
            warnings.push(warn(
                "ZERO_LENGTH",
                format!("Shot {}:{} has zero length", section_id, shot_index + 1),
            ));
        }

        // Check for negative length
        if shot.length < 0.0 {
            warnings.push(warn(
                "NEGATIVE_LENGTH",
                format!(
                    "Shot {}:{} has negative length: {:.1}m",
                    section_id,
                    shot_index + 1,
                    shot.length
                ),
            ));
        }

        // Check length thresholds
        if shot.length > 0.0 && shot.length < config.warn_shot_length_min {
            warnings.push(warn(
                "SHORT_SHOT",
                format!(
                    "Shot {}:{} length {:.1}m below minimum threshold {:.1}m",
                    section_id,
                    shot_index + 1,
                    shot.length,
                    config.warn_shot_length_min
                ),
            ));
        }

        if shot.length > config.warn_shot_length_max {
            warnings.push(warn(
                "LONG_SHOT",
                format!(
                    "Shot {}:{} length {:.1}m above maximum threshold {:.1}m",
                    section_id,
                    shot_index + 1,
                    shot.length,
                    config.warn_shot_length_max
                ),
            ));
        }

        // Check depth change
        let depth_change = (shot.depth_out - shot.depth_in).abs();
        if depth_change > config.warn_depth_change_per_shot_max {
            warnings.push(warn(
                "LARGE_DEPTH_CHANGE",
                format!(
                    "Shot {}:{} depth change {:.1}m exceeds threshold {:.1}m",
                    section_id,
                    shot_index + 1,
                    depth_change,
                    config.warn_depth_change_per_shot_max
                ),
            ));
        }

        // Check azimuth range
        if shot.azimuth_in < 0.0 || shot.azimuth_in >= 360.0 {
            warnings.push(warn(
                "INVALID_AZIMUTH",
                format!(
                    "Shot {}:{} has invalid azimuth_in: {:.1}° (must be 0-360)",
                    section_id,
                    shot_index + 1,
                    shot.azimuth_in
                ),
            ));
        }

        if shot.azimuth_out < 0.0 || shot.azimuth_out >= 360.0 {
            warnings.push(warn(
                "INVALID_AZIMUTH",
                format!(
                    "Shot {}:{} has invalid azimuth_out: {:.1}° (must be 0-360)",
                    section_id,
                    shot_index + 1,
                    shot.azimuth_out
                ),
            ));
        }

        // Check azimuth in/out difference (both forward readings, should be similar)
        let diff = (shot.azimuth_out - shot.azimuth_in).abs();
        let angular_diff = if diff > 180.0 { 360.0 - diff } else { diff };
        if angular_diff > config.azimuth_diff_threshold {
            warnings.push(warn(
                "AZIMUTH_DEVIATION",
                format!(
                    "Shot {}:{} azimuth in/out difference {:.1}° exceeds threshold {:.1}°",
                    section_id,
                    shot_index + 1,
                    angular_diff,
                    config.azimuth_diff_threshold
                ),
            ));
        }

        // Check LRUD values are non-negative
        if let Some(left) = shot.left {
            if left < 0.0 {
                warnings.push(warn(
                    "NEGATIVE_LRUD",
                    format!(
                        "Shot {}:{} has negative left distance: {:.1}m",
                        section_id,
                        shot_index + 1,
                        left
                    ),
                ));
            }
        }

        if let Some(right) = shot.right {
            if right < 0.0 {
                warnings.push(warn(
                    "NEGATIVE_LRUD",
                    format!(
                        "Shot {}:{} has negative right distance: {:.1}m",
                        section_id,
                        shot_index + 1,
                        right
                    ),
                ));
            }
        }

        if let Some(up) = shot.up {
            if up < 0.0 {
                warnings.push(warn(
                    "NEGATIVE_LRUD",
                    format!(
                        "Shot {}:{} has negative up distance: {:.1}m",
                        section_id,
                        shot_index + 1,
                        up
                    ),
                ));
            }
        }

        if let Some(down) = shot.down {
            if down < 0.0 {
                warnings.push(warn(
                    "NEGATIVE_LRUD",
                    format!(
                        "Shot {}:{} has negative down distance: {:.1}m",
                        section_id,
                        shot_index + 1,
                        down
                    ),
                ));
            }
        }

        // A recorded inclination must be physically valid: outside [-90°, 90°] is
        // nonsensical (issue #57). Flag whichever leg is out of range.
        let leg_in_range = |p: Option<f64>| -> bool {
            match p {
                Some(v) => (PITCH_MIN..=PITCH_MAX).contains(&v),
                None => true,
            }
        };
        for (leg, pitch) in [("in", shot.pitch_in), ("out", shot.pitch_out)] {
            if let Some(p) = pitch {
                if !(PITCH_MIN..=PITCH_MAX).contains(&p) {
                    warnings.push(warn(
                        "INVALID_PITCH",
                        format!(
                            "Shot {}:{} has an out-of-range pitch_{leg}: {:.1}° (must be between -90° and 90°)",
                            section_id,
                            shot_index + 1,
                            p
                        ),
                    ));
                }
            }
        }

        // The two inclination readings are two forward readings of the same
        // line, like azimuth in/out, so they should agree. Compare them only
        // when both were recorded and both are physically valid (an
        // out-of-range leg is already an INVALID_PITCH error).
        if let (Some(pitch_in), Some(pitch_out)) = (shot.pitch_in, shot.pitch_out) {
            let pitch_diff = (pitch_out - pitch_in).abs();
            if leg_in_range(Some(pitch_in))
                && leg_in_range(Some(pitch_out))
                && pitch_diff > config.pitch_diff_threshold
            {
                warnings.push(warn(
                    "PITCH_DEVIATION",
                    format!(
                        "Shot {}:{} inclination in/out difference {:.1}° exceeds threshold {:.1}°",
                        section_id,
                        shot_index + 1,
                        pitch_diff,
                        config.pitch_diff_threshold
                    ),
                ));
            }
        }

        // Length / depth-change / inclination geometry. The slope length, the
        // vertical change (dz = depth_out - depth_in), and the inclination form a
        // right triangle: dz = length * sin(pitch). Two checks fall out of it.
        let dz = shot.depth_out - shot.depth_in;
        if shot.length > 0.0 {
            // The slope length can never be shorter than the vertical change -
            // that would need sin(pitch) > 1. A small epsilon absorbs rounding.
            if shot.length + DEPTH_ROUNDING_EPSILON_M < dz.abs() {
                warnings.push(warn(
                    "LENGTH_BELOW_DEPTH_CHANGE",
                    format!(
                        "Shot {}:{} length {:.2}m is shorter than its depth change {:.2}m (impossible: slope length must be at least the vertical change)",
                        section_id,
                        shot_index + 1,
                        shot.length,
                        dz.abs()
                    ),
                ));
            } else {
                // A recorded inclination must agree with the depth/length geometry
                // in BOTH magnitude AND direction. Both sides are +down: positive
                // means the shot DESCENDS and depth INCREASES, the same frame
                // `depth_in` / `depth_out` are already in. Every parser normalises
                // to that frame at its own import boundary, so the two values can
                // be compared directly and a reversed inclination still shows up as
                // a large signed difference (issue #57 finding #7, issue #296).
                // Skip when a leg is out of range (already flagged INVALID_PITCH)
                // or when no inclination was recorded (nothing to cross-check).
                let recorded_pitch = match (shot.pitch_in, shot.pitch_out) {
                    (Some(a), Some(b)) => Some((a + b) / 2.0),
                    (Some(a), None) | (None, Some(a)) => Some(a),
                    (None, None) => None,
                };
                if let Some(recorded) = recorded_pitch {
                    if leg_in_range(shot.pitch_in) && leg_in_range(shot.pitch_out) {
                        let implied = implied_pitch_deg(shot.length, shot.depth_in, shot.depth_out);
                        if (recorded - implied).abs() > config.inclination_depth_diff_threshold {
                            warnings.push(warn(
                                "INCLINATION_DEPTH_MISMATCH",
                                format!(
                                    "Shot {}:{} recorded inclination {:.1}° disagrees with the {:.1}° implied by its length and depth change (exceeds {:.1}° threshold)",
                                    section_id,
                                    shot_index + 1,
                                    recorded,
                                    implied,
                                    config.inclination_depth_diff_threshold
                                ),
                            ));
                        }
                    }
                }
            }
        }

        warnings
    }

    /// Check if any warnings are errors
    pub fn has_errors(warnings: &[ValidationWarning]) -> bool {
        warnings
            .iter()
            .any(|w| w.severity == WarningSeverity::Error)
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    #![expect(
        clippy::expect_used,
        reason = "tests assert on known-present warnings; a failed expect fails the test"
    )]
    use super::*;
    use crate::models::GpsCoordinates;

    fn default_config() -> ValidationConfig {
        ValidationConfig {
            warn_shot_length_min: 0.1,
            warn_shot_length_max: 50.0,
            warn_depth_change_per_shot_max: 10.0,
            warn_azimuth_deviation_max: 30.0,
            azimuth_diff_threshold: 20.0,
            pitch_diff_threshold: 5.0,
            inclination_depth_diff_threshold: 10.0,
        }
    }

    #[test]
    fn test_valid_shot() {
        let shot = ShotData {
            azimuth_in: 45.0,
            azimuth_out: 50.0,
            length: 5.0,
            depth_in: 10.0,
            depth_out: 11.0,
            ..Default::default()
        };

        let warnings = Validator::validate_shot(&shot, &default_config(), 0, 0);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_zero_length_shot() {
        let shot = ShotData {
            azimuth_in: 45.0,
            azimuth_out: 50.0,
            length: 0.0,
            depth_in: 10.0,
            depth_out: 10.0,
            ..Default::default()
        };

        let warnings = Validator::validate_shot(&shot, &default_config(), 0, 0);
        assert!(Validator::has_errors(&warnings));
    }

    /// #275 B4 backstop: NaN fails every range comparison in validate_shot,
    /// so before the explicit check a NaN-poisoned shot produced ZERO
    /// warnings. Pin that every non-finite field (required and optional)
    /// yields the error-severity NON_FINITE_VALUE code naming the field.
    #[test]
    fn test_non_finite_values_warn_instead_of_passing_silently() {
        let shot = ShotData {
            azimuth_in: f64::NAN,
            azimuth_out: 50.0,
            length: f64::INFINITY,
            depth_in: 10.0,
            depth_out: f64::NEG_INFINITY,
            pitch_in: Some(f64::NAN),
            left: Some(f64::NAN),
            ..Default::default()
        };

        let warnings = Validator::validate_shot(&shot, &default_config(), 3, 0);
        let nf = warnings
            .iter()
            .find(|w| w.code == "NON_FINITE_VALUE")
            .expect("non-finite fields must produce NON_FINITE_VALUE");
        assert_eq!(nf.severity, WarningSeverity::Error);
        for field in ["length", "azimuth_in", "depth_out", "pitch_in", "left"] {
            assert!(
                nf.message.contains(field),
                "message should name '{field}': {}",
                nf.message
            );
        }
        assert!(
            !nf.message.contains("azimuth_out") && !nf.message.contains("depth_in"),
            "finite fields must not be named: {}",
            nf.message
        );

        // A fully finite shot emits no NON_FINITE_VALUE.
        let ok = ShotData {
            azimuth_in: 45.0,
            azimuth_out: 50.0,
            length: 5.0,
            depth_in: 10.0,
            depth_out: 11.0,
            ..Default::default()
        };
        assert!(Validator::validate_shot(&ok, &default_config(), 3, 0)
            .iter()
            .all(|w| w.code != "NON_FINITE_VALUE"));
    }

    #[test]
    fn test_azimuth_deviation() {
        let shot = ShotData {
            azimuth_in: 45.0,
            azimuth_out: 180.0, // 135° difference, exceeds 10° threshold
            length: 5.0,
            depth_in: 10.0,
            depth_out: 11.0,
            ..Default::default()
        };

        let warnings = Validator::validate_shot(&shot, &default_config(), 0, 0);
        assert!(!warnings.is_empty());
        assert!(warnings.iter().any(|w| w.code == "AZIMUTH_DEVIATION"));
    }

    /// Verify warning messages use "Shot {section_id}:{position}" format
    #[test]
    fn test_warning_message_format() {
        let config = default_config();

        // ZERO_LENGTH with section_id=4, shot_index=2 → "Shot 4:3 has zero length"
        let zero_shot = ShotData {
            azimuth_in: 45.0,
            azimuth_out: 50.0,
            length: 0.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&zero_shot, &config, 4, 2);
        let w = warnings.iter().find(|w| w.code == "ZERO_LENGTH").unwrap();
        assert!(
            w.message.starts_with("Shot 4:3 "),
            "Expected 'Shot 4:3 ...' but got: {}",
            w.message
        );

        // SHORT_SHOT with section_id=7, shot_index=0 → "Shot 7:1 length ..."
        let short_shot = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 95.0,
            length: 0.05,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&short_shot, &config, 7, 0);
        let w = warnings.iter().find(|w| w.code == "SHORT_SHOT").unwrap();
        assert!(
            w.message.starts_with("Shot 7:1 "),
            "Expected 'Shot 7:1 ...' but got: {}",
            w.message
        );

        // INVALID_AZIMUTH with section_id=2, shot_index=5 → "Shot 2:6 has invalid azimuth_in..."
        let bad_azimuth = ShotData {
            azimuth_in: 400.0,
            azimuth_out: 180.0,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&bad_azimuth, &config, 2, 5);
        let w = warnings
            .iter()
            .find(|w| w.code == "INVALID_AZIMUTH")
            .unwrap();
        assert!(
            w.message.starts_with("Shot 2:6 "),
            "Expected 'Shot 2:6 ...' but got: {}",
            w.message
        );

        // NEGATIVE_LRUD with section_id=10, shot_index=3 → "Shot 10:4 has negative left..."
        let neg_lrud = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 270.0,
            length: 5.0,
            left: Some(-1.0),
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&neg_lrud, &config, 10, 3);
        let w = warnings.iter().find(|w| w.code == "NEGATIVE_LRUD").unwrap();
        assert!(
            w.message.starts_with("Shot 10:4 "),
            "Expected 'Shot 10:4 ...' but got: {}",
            w.message
        );
    }

    /// Test 5.1: Verify shot length validation (must be positive, reasonable max)
    #[test]
    fn test_validate_shot_length() {
        let config = default_config();

        // Valid positive length
        let valid_shot = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 95.0,
            length: 5.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&valid_shot, &config, 0, 0);
        assert!(warnings.iter().all(|w| !w.code.contains("LENGTH")));

        // Negative length should produce error
        let negative_shot = ShotData {
            length: -5.0,
            azimuth_in: 90.0,
            azimuth_out: 95.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&negative_shot, &config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "NEGATIVE_LENGTH"));
        assert!(Validator::has_errors(&warnings));

        // Zero length should produce error
        let zero_shot = ShotData {
            length: 0.0,
            azimuth_in: 90.0,
            azimuth_out: 95.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&zero_shot, &config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "ZERO_LENGTH"));
        assert!(Validator::has_errors(&warnings));

        // Very short shot (below min threshold) should produce warning
        let short_shot = ShotData {
            length: 0.05, // Below warn_shot_length_min of 0.1
            azimuth_in: 90.0,
            azimuth_out: 95.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&short_shot, &config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "SHORT_SHOT"));

        // Very long shot (above max threshold) should produce warning
        let long_shot = ShotData {
            length: 100.0, // Above warn_shot_length_max of 50.0
            azimuth_in: 90.0,
            azimuth_out: 95.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&long_shot, &config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "LONG_SHOT"));
    }

    /// Test 5.2: Verify azimuth validation (0-360 degrees)
    #[test]
    fn test_validate_azimuth_range() {
        let config = default_config();

        // Valid azimuth at 0 degrees
        let shot_0 = ShotData {
            azimuth_in: 0.0,
            azimuth_out: 5.0,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&shot_0, &config, 0, 0);
        assert!(warnings.iter().all(|w| w.code != "INVALID_AZIMUTH"));

        // Valid azimuth at 359.9 degrees
        let shot_359 = ShotData {
            azimuth_in: 359.9,
            azimuth_out: 359.5,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&shot_359, &config, 0, 0);
        assert!(warnings.iter().all(|w| w.code != "INVALID_AZIMUTH"));

        // Invalid: azimuth_in >= 360
        let shot_360 = ShotData {
            azimuth_in: 360.0,
            azimuth_out: 180.0,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&shot_360, &config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "INVALID_AZIMUTH"));
        assert!(Validator::has_errors(&warnings));

        // Invalid: azimuth_out >= 360
        let shot_out_400 = ShotData {
            azimuth_in: 45.0,
            azimuth_out: 400.0,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&shot_out_400, &config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "INVALID_AZIMUTH"));

        // Invalid: negative azimuth_in
        let shot_neg = ShotData {
            azimuth_in: -10.0,
            azimuth_out: 180.0,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&shot_neg, &config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "INVALID_AZIMUTH"));

        // Invalid: negative azimuth_out
        let shot_neg_out = ShotData {
            azimuth_in: 45.0,
            azimuth_out: -45.0,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&shot_neg_out, &config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "INVALID_AZIMUTH"));
    }

    /// Test 5.3: Verify LRUD validation (non-negative values)
    #[test]
    fn test_validate_lrud_values() {
        let config = default_config();

        // Valid LRUD values (all positive)
        let valid_shot = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 95.0,
            length: 5.0,
            left: Some(2.0),
            right: Some(3.0),
            up: Some(1.5),
            down: Some(0.5),
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&valid_shot, &config, 0, 0);
        assert!(warnings.iter().all(|w| w.code != "NEGATIVE_LRUD"));

        // Valid: zero LRUD values are allowed (wall touching)
        let zero_lrud = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 95.0,
            length: 5.0,
            left: Some(0.0),
            right: Some(0.0),
            up: Some(0.0),
            down: Some(0.0),
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&zero_lrud, &config, 0, 0);
        assert!(warnings.iter().all(|w| w.code != "NEGATIVE_LRUD"));

        // Valid: None LRUD values (not measured)
        let none_lrud = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 95.0,
            length: 5.0,
            left: None,
            right: None,
            up: None,
            down: None,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&none_lrud, &config, 0, 0);
        assert!(warnings.iter().all(|w| w.code != "NEGATIVE_LRUD"));

        // Invalid: negative left
        let neg_left = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 95.0,
            length: 5.0,
            left: Some(-1.0),
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&neg_left, &config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "NEGATIVE_LRUD"));
        assert!(Validator::has_errors(&warnings));

        // Invalid: negative right
        let neg_right = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 95.0,
            length: 5.0,
            right: Some(-2.5),
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&neg_right, &config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "NEGATIVE_LRUD"));

        // Invalid: negative up
        let neg_up = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 95.0,
            length: 5.0,
            up: Some(-0.5),
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&neg_up, &config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "NEGATIVE_LRUD"));

        // Invalid: negative down
        let neg_down = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 95.0,
            length: 5.0,
            down: Some(-3.0),
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&neg_down, &config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "NEGATIVE_LRUD"));
    }

    /// Test 5.4: Verify GPS coordinate validation (latitude -90 to 90, longitude -180 to 180)
    #[test]
    fn test_validate_gps_coordinates() {
        // Valid coordinates: typical cave location
        assert!(GpsCoordinates::validate(45.0, -93.0, 10.0).is_ok());

        // Valid: equator and prime meridian
        assert!(GpsCoordinates::validate(0.0, 0.0, 0.0).is_ok());

        // Valid: north pole
        assert!(GpsCoordinates::validate(90.0, 0.0, 0.0).is_ok());

        // Valid: south pole
        assert!(GpsCoordinates::validate(-90.0, 0.0, 0.0).is_ok());

        // Valid: date line (positive)
        assert!(GpsCoordinates::validate(0.0, 180.0, 0.0).is_ok());

        // Valid: date line (negative)
        assert!(GpsCoordinates::validate(0.0, -180.0, 0.0).is_ok());

        // Valid: extreme valid coordinates
        assert!(GpsCoordinates::validate(90.0, 180.0, 0.0).is_ok());
        assert!(GpsCoordinates::validate(-90.0, -180.0, 0.0).is_ok());

        // Invalid: latitude > 90
        let result = GpsCoordinates::validate(91.0, 0.0, 0.0);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Latitude"));

        // Invalid: latitude < -90
        let result = GpsCoordinates::validate(-91.0, 0.0, 0.0);
        assert!(result.is_err());

        // Invalid: longitude > 180
        let result = GpsCoordinates::validate(0.0, 181.0, 0.0);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Longitude"));

        // Invalid: longitude < -180
        let result = GpsCoordinates::validate(0.0, -181.0, 0.0);
        assert!(result.is_err());

        // Invalid: NaN latitude
        let result = GpsCoordinates::validate(f64::NAN, 0.0, 0.0);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("finite"));

        // Invalid: infinity longitude
        let result = GpsCoordinates::validate(0.0, f64::INFINITY, 0.0);
        assert!(result.is_err());

        // Invalid: negative infinity depth reference
        let result = GpsCoordinates::validate(0.0, 0.0, f64::NEG_INFINITY);
        assert!(result.is_err());

        // Test GpsCoordinates::new() uses validation
        assert!(GpsCoordinates::new(45.0, -93.0, 10.0).is_ok());
        assert!(GpsCoordinates::new(100.0, 0.0, 0.0).is_err());

        // Test is_valid() method
        let valid_gps = GpsCoordinates {
            latitude: 45.0,
            longitude: -93.0,
            depth_reference: 10.0,
        };
        assert!(valid_gps.is_valid());
    }

    /// Test 5.5: Verify azimuth in/out difference threshold
    #[test]
    fn test_validate_azimuth_diff_threshold() {
        // azimuth_in and azimuth_out are both forward readings, should be similar.
        // The validator checks if the angular difference exceeds azimuth_diff_threshold.

        // Test with default config (10° threshold)
        let config = default_config();

        // Valid: identical readings (0° difference)
        let identical = ShotData {
            azimuth_in: 45.0,
            azimuth_out: 45.0,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&identical, &config, 0, 0);
        assert!(warnings.iter().all(|w| w.code != "AZIMUTH_DEVIATION"));

        // Valid: within threshold (5° difference)
        let within_threshold = ShotData {
            azimuth_in: 45.0,
            azimuth_out: 50.0,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&within_threshold, &config, 0, 0);
        assert!(warnings.iter().all(|w| w.code != "AZIMUTH_DEVIATION"));

        // Valid: exactly at threshold (20° difference, not exceeded)
        let at_threshold = ShotData {
            azimuth_in: 45.0,
            azimuth_out: 65.0,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&at_threshold, &config, 0, 0);
        assert!(warnings.iter().all(|w| w.code != "AZIMUTH_DEVIATION"));

        // Invalid: exceeds threshold (25° difference)
        let exceeds_threshold = ShotData {
            azimuth_in: 45.0,
            azimuth_out: 70.0,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&exceeds_threshold, &config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "AZIMUTH_DEVIATION"));

        // Test wraparound: 355° and 5° should be 10° apart (within threshold)
        let wraparound_within = ShotData {
            azimuth_in: 355.0,
            azimuth_out: 5.0,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&wraparound_within, &config, 0, 0);
        assert!(warnings.iter().all(|w| w.code != "AZIMUTH_DEVIATION"));

        // Test wraparound with warning: 355° and 20° = 25° apart
        let wraparound_exceeds = ShotData {
            azimuth_in: 355.0,
            azimuth_out: 20.0,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&wraparound_exceeds, &config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "AZIMUTH_DEVIATION"));

        // Test with strict config (5° threshold)
        let strict_config = ValidationConfig {
            azimuth_diff_threshold: 5.0,
            ..default_config()
        };

        // Small difference (3°) should pass with strict config
        let small_diff = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 93.0,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&small_diff, &strict_config, 0, 0);
        assert!(warnings.iter().all(|w| w.code != "AZIMUTH_DEVIATION"));

        // 10° difference should fail with strict 5° config
        let medium_diff = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 100.0,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&medium_diff, &strict_config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "AZIMUTH_DEVIATION"));

        // Test with lenient config (60° threshold)
        let lenient_config = ValidationConfig {
            azimuth_diff_threshold: 60.0,
            ..default_config()
        };

        // 50° difference should pass with lenient config
        let large_diff = ShotData {
            azimuth_in: 0.0,
            azimuth_out: 50.0,
            length: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&large_diff, &lenient_config, 0, 0);
        assert!(warnings.iter().all(|w| w.code != "AZIMUTH_DEVIATION"));
    }

    /// Slope length must be >= the vertical (depth) change - otherwise the
    /// geometry is impossible (would need sin(pitch) > 1).
    #[test]
    fn test_length_below_depth_change() {
        let config = default_config();

        // length 3m but a 5m drop - impossible.
        let bad = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 3.0,
            depth_in: 0.0,
            depth_out: 5.0,
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&bad, &config, 0, 0);
        assert!(warnings
            .iter()
            .any(|w| w.code == "LENGTH_BELOW_DEPTH_CHANGE"));
        assert!(Validator::has_errors(&warnings));

        // Sign-agnostic: a 5m rise with a 3m slope is equally impossible.
        let bad_up = ShotData {
            depth_in: 5.0,
            depth_out: 0.0,
            ..bad.clone()
        };
        assert!(Validator::validate_shot(&bad_up, &config, 0, 0)
            .iter()
            .any(|w| w.code == "LENGTH_BELOW_DEPTH_CHANGE"));

        // length == |dz| (vertical shot) is valid; length > |dz| is valid.
        let vertical = ShotData {
            length: 5.0,
            depth_in: 0.0,
            depth_out: 5.0,
            ..bad.clone()
        };
        assert!(Validator::validate_shot(&vertical, &config, 0, 0)
            .iter()
            .all(|w| w.code != "LENGTH_BELOW_DEPTH_CHANGE"));
        let ok = ShotData {
            length: 10.0,
            depth_in: 0.0,
            depth_out: 5.0,
            ..bad.clone()
        };
        assert!(Validator::validate_shot(&ok, &config, 0, 0)
            .iter()
            .all(|w| w.code != "LENGTH_BELOW_DEPTH_CHANGE"));
    }

    /// Real MNemo shots must survive the cross-check. This is the regression
    /// for issue #296: the device reports inclination +up, so before the import
    /// was normalised every non-horizontal DMP shot arrived with a pitch that
    /// contradicted its own depth change, and validation carried a compensating
    /// -1x to hide it. Both ends are +down now, and these readings from
    /// `9_20260522-naharon.dmp`, normalised exactly as the DMP parser does,
    /// have to come through clean.
    #[test]
    fn dmp_shots_do_not_trip_the_inclination_cross_check() {
        let config = default_config();

        // (length, depth_in, depth_out, device pitch as written by the MNemo)
        let device_shots = [
            (12.50, 14.51, 18.15, -16.9),
            (9.09, 13.55, 14.71, -6.0),
            (4.81, 16.93, 15.45, 19.2),
            (10.35, 18.29, 16.92, 10.5),
            (18.25, 18.96, 17.48, 6.8),
        ];

        for (length, depth_in, depth_out, device_pitch) in device_shots {
            // `parse_section` normalises the device's +up reading into the
            // model's +down frame, so this is the pitch a `DmpShot` carries by
            // the time anything downstream sees it.
            let pitch = -device_pitch;
            let dmp_shot = crate::parsers::dmp::DmpShot {
                shot_type: crate::parsers::dmp::DmpShotType::Standard,
                heading_in: 0.0,
                heading_out: 0.0,
                length,
                depth_in,
                depth_out,
                pitch_in: pitch,
                pitch_out: pitch,
                left: 0.0,
                right: 0.0,
                up: 0.0,
                down: 0.0,
                temperature: 0.0,
                hour: 0,
                minute: 0,
                second: 0,
                marker_index: 0,
                comment: None,
            };

            let shot_data = dmp_shot.to_shot_data();

            // The property that makes the cross-check pass rather than an
            // accident of thresholds: descending shots (depth increasing) carry
            // a positive pitch, ascending shots a negative one.
            let descending = depth_out > depth_in;
            let pitch_in = shot_data.pitch_in.expect("pitch_in survives conversion");
            assert_eq!(
                pitch_in > 0.0,
                descending,
                "pitch {pitch_in}° does not match the {depth_in}m -> {depth_out}m depth change"
            );

            let warnings = Validator::validate_shot(&shot_data, &config, 0, 0);
            let mismatch = warnings
                .iter()
                .find(|w| w.code == "INCLINATION_DEPTH_MISMATCH");
            assert!(
                mismatch.is_none(),
                "device pitch {device_pitch}° (imported as {pitch}°) over {length}m \
                 ({depth_in}m -> {depth_out}m) was flagged after import: {:?}",
                mismatch.map(|w| &w.message)
            );
        }
    }

    /// A recorded inclination must agree with the inclination implied by the
    /// length and depth change in BOTH magnitude and DIRECTION. Convention is
    /// +down (the same frame as `depth_in` / `depth_out`): positive = descending,
    /// depth increasing; negative = ascending, depth decreasing.
    #[test]
    fn test_inclination_depth_mismatch() {
        let config = default_config(); // inclination_depth_diff_threshold = 10.0

        // length 10, dz +5 (depth INCREASES = going DOWN) → implied +30° (+down).
        let down = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 5.0,
            ..Default::default()
        };
        let has_mismatch = |s: &ShotData| {
            Validator::validate_shot(s, &config, 0, 0)
                .iter()
                .any(|w| w.code == "INCLINATION_DEPTH_MISMATCH")
        };

        // Recorded +30° (descending) agrees in sign and magnitude with the +30° implied.
        let consistent = ShotData {
            pitch_in: Some(30.0),
            pitch_out: Some(30.0),
            ..down.clone()
        };
        assert!(
            !has_mismatch(&consistent),
            "+30° matches the +30° implied for a downward shot"
        );

        // Recorded -30° has the right MAGNITUDE but the WRONG DIRECTION (says the
        // shot ascends while the depth change says it descends) - flagged (finding #7).
        let reversed = ShotData {
            pitch_in: Some(-30.0),
            pitch_out: Some(-30.0),
            ..down.clone()
        };
        assert!(
            has_mismatch(&reversed),
            "-30° contradicts the downward depth change and must be flagged"
        );

        // Genuine magnitude disagreement is still flagged.
        for recorded in [-10.0_f64, 10.0_f64, 80.0_f64] {
            let mismatched = ShotData {
                pitch_in: Some(recorded),
                pitch_out: Some(recorded),
                ..down.clone()
            };
            assert!(
                has_mismatch(&mismatched),
                "recorded {recorded}° disagrees with the +30° implied and must be flagged"
            );
        }

        // Symmetry: a shot going UP (dz -5) implies -30°; -30° is consistent, +30° reversed.
        let up = ShotData {
            depth_in: 5.0,
            depth_out: 0.0,
            ..down.clone()
        };
        let up_consistent = ShotData {
            pitch_in: Some(-30.0),
            pitch_out: Some(-30.0),
            ..up.clone()
        };
        assert!(
            !has_mismatch(&up_consistent),
            "-30° matches the -30° implied for an upward shot"
        );
        let up_reversed = ShotData {
            pitch_in: Some(30.0),
            pitch_out: Some(30.0),
            ..up.clone()
        };
        assert!(
            has_mismatch(&up_reversed),
            "+30° contradicts the upward depth change and must be flagged"
        );

        // A vertical shot (length == |dz|) implies ±90°; recording it clears it.
        // This is exactly what "set length to depth change" does (it also sets the
        // pitch to the matching ±90°), so that fix must not re-raise the warning.
        // Downward (depth increases) ⇒ +90° in the +down convention.
        let vertical = ShotData {
            length: 5.0,
            depth_in: 0.0,
            depth_out: 5.0,
            pitch_in: Some(90.0),
            pitch_out: Some(90.0),
            ..down.clone()
        };
        assert!(
            !has_mismatch(&vertical),
            "a vertical downward shot with +90° pitch is consistent"
        );

        // No recorded inclination → nothing to cross-check, not flagged.
        let no_pitch = ShotData {
            pitch_in: None,
            pitch_out: None,
            ..down.clone()
        };
        assert!(!has_mismatch(&no_pitch));

        // An out-of-range pitch is flagged as INVALID_PITCH, and the geometry
        // cross-check is skipped for it (no duplicate INCLINATION_DEPTH_MISMATCH).
        let out_of_range = ShotData {
            pitch_in: Some(120.0),
            pitch_out: Some(120.0),
            ..down.clone()
        };
        let w = Validator::validate_shot(&out_of_range, &config, 0, 0);
        assert!(
            w.iter().any(|w| w.code == "INVALID_PITCH"),
            "120° pitch is out of range"
        );
        assert!(
            w.iter().all(|w| w.code != "INCLINATION_DEPTH_MISMATCH"),
            "skip the cross-check for an invalid pitch"
        );
    }

    /// #482: the validator itself must NOT be weakened. A genuinely recorded
    /// `Some(0.0)` pitch on a shot with a real depth change (as any non-TML
    /// importer, or a shot entered by hand, can produce) still disagrees
    /// with the geometry and must still warn - only the TML importer's
    /// zero-means-absent inference (`parsers::tml::normalize_tml_shot`)
    /// tells `Some(0.0)` apart from "never recorded"; the validator has no
    /// such distinction to make and keeps cross-checking every `Some`.
    #[test]
    fn test_recorded_zero_pitch_with_depth_change_still_warns() {
        let config = default_config();
        let shot = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 5.0,
            pitch_in: Some(0.0),
            pitch_out: Some(0.0),
            ..Default::default()
        };
        let warnings = Validator::validate_shot(&shot, &config, 0, 0);
        assert!(
            warnings
                .iter()
                .any(|w| w.code == "INCLINATION_DEPTH_MISMATCH"),
            "a recorded 0° pitch on a shot that descended 5m over 10m must still \
             be cross-checked, got {warnings:?}"
        );
    }

    /// Pitch outside [-90°, 90°] is physically impossible → INVALID_PITCH error.
    #[test]
    fn test_invalid_pitch_range() {
        let config = default_config();
        let base = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        };

        // In range (inclusive of the ±90° bounds): no INVALID_PITCH.
        for p in [0.0_f64, 45.0, -45.0, 90.0, -90.0] {
            let ok = ShotData {
                pitch_in: Some(p),
                pitch_out: Some(p),
                ..base.clone()
            };
            assert!(
                Validator::validate_shot(&ok, &config, 0, 0)
                    .iter()
                    .all(|w| w.code != "INVALID_PITCH"),
                "{p}° is a valid pitch"
            );
        }

        // Out of range on either leg → error.
        let bad_in = ShotData {
            pitch_in: Some(91.0),
            pitch_out: Some(0.0),
            ..base.clone()
        };
        let warnings = Validator::validate_shot(&bad_in, &config, 0, 0);
        assert!(warnings.iter().any(|w| w.code == "INVALID_PITCH"));
        assert!(Validator::has_errors(&warnings));

        let bad_out = ShotData {
            pitch_in: Some(0.0),
            pitch_out: Some(-100.0),
            ..base.clone()
        };
        assert!(Validator::validate_shot(&bad_out, &config, 0, 0)
            .iter()
            .any(|w| w.code == "INVALID_PITCH"));
    }

    /// The two inclination readings must agree within `pitch_diff_threshold`
    /// (5° in the test config). Exactly at the threshold is not a warning, the
    /// same as AZIMUTH_DEVIATION; a single recorded leg has nothing to compare
    /// against; an out-of-range leg is only the INVALID_PITCH error.
    #[test]
    fn test_pitch_deviation() {
        let config = default_config();
        let base = ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        };
        let pitch_deviation = |pitch_in: Option<f64>, pitch_out: Option<f64>| {
            let shot = ShotData {
                pitch_in,
                pitch_out,
                ..base.clone()
            };
            Validator::validate_shot(&shot, &config, 2, 4)
                .into_iter()
                .find(|w| w.code == "PITCH_DEVIATION")
        };

        assert!(pitch_deviation(Some(2.0), Some(2.0)).is_none());
        assert!(
            pitch_deviation(Some(2.0), Some(7.0)).is_none(),
            "exactly at the threshold is not a warning"
        );
        let w = pitch_deviation(Some(2.0), Some(8.0)).expect("6° exceeds the 5° threshold");
        assert_eq!(w.severity, WarningSeverity::Warning);
        assert!(
            w.message.starts_with("Shot 2:5 ") && w.message.contains("6.0°"),
            "unexpected message: {}",
            w.message
        );
        assert!(
            pitch_deviation(Some(-3.0), Some(3.0)).is_some(),
            "legs on opposite sides of level differ by their sum"
        );
        assert!(pitch_deviation(Some(2.0), None).is_none());
        assert!(pitch_deviation(None, Some(8.0)).is_none());
        assert!(
            pitch_deviation(Some(0.0), Some(120.0)).is_none(),
            "an out-of-range leg is INVALID_PITCH only"
        );

        let lenient = ValidationConfig {
            pitch_diff_threshold: 20.0,
            ..default_config()
        };
        let shot = ShotData {
            pitch_in: Some(2.0),
            pitch_out: Some(8.0),
            ..base.clone()
        };
        assert!(Validator::validate_shot(&shot, &lenient, 0, 0)
            .iter()
            .all(|w| w.code != "PITCH_DEVIATION"));
    }

    /// `severity_for_code` is the single source of truth, so every warning the
    /// validator emits must carry the severity it declares.
    #[test]
    fn test_validator_severity_matches_single_source() {
        use crate::models::warning::severity_for_code;
        let config = default_config();
        // A shot that trips a spread of codes (errors + warnings).
        let shot = ShotData {
            azimuth_in: 400.0, // INVALID_AZIMUTH (error)
            azimuth_out: 90.0,
            length: 0.0, // ZERO_LENGTH (error)
            depth_in: 0.0,
            depth_out: 0.0,
            left: Some(-1.0),      // NEGATIVE_LRUD (error)
            pitch_in: Some(120.0), // INVALID_PITCH (error)
            ..Default::default()
        };
        for w in Validator::validate_shot(&shot, &config, 0, 0) {
            assert_eq!(
                w.severity,
                severity_for_code(&w.code),
                "severity drift for {}",
                w.code
            );
        }
    }
}
