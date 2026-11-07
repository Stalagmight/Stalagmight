/**
 * Validation helpers: the validation-threshold settings check, the azimuth and
 * pitch in/out discrepancy tests, and the import dialogs' editable-shot check.
 * Survey warnings themselves come from the backend (`get_project_warnings`).
 */

import { type ValidationConfig, defaultConfig } from '../types/config';
import i18n from '../i18n';
import { formatFixed } from '../i18n/format';

// Re-export ValidationConfig for convenience
export type { ValidationConfig };

/**
 * Default validation config from shared config.
 */
export const DEFAULT_VALIDATION_CONFIG: ValidationConfig = defaultConfig.validation;

/**
 * Per-field validation errors for a ValidationConfig: each field maps to its
 * i18n error key, or null when valid.
 *
 * NOTE: this intentionally OMITS `warn_azimuth_deviation_max` - none of the
 * three call sites (the global Settings dialog, the per-project settings
 * dialog, and the ValidationSettings panel) range-check that field, so the
 * record is a partial view of ValidationConfig by design. Do not assume it is
 * exhaustive.
 */
export type ValidationConfigErrors = {
  warn_shot_length_min: string | null;
  warn_shot_length_max: string | null;
  warn_depth_change_per_shot_max: string | null;
  azimuth_diff_threshold: string | null;
  pitch_diff_threshold: string | null;
  inclination_depth_diff_threshold: string | null;
};

/**
 * Single source of truth for the per-field range rules shared by the global
 * Settings dialog, the per-project settings dialog, and the ValidationSettings
 * panel. Returns a record of i18n error *keys* (not translated strings) so the
 * module stays free of react-i18next hooks; callers translate as needed.
 *
 * The bounds reproduce the three original copies exactly: `min >= 0`,
 * `max > min` (so `max <= min` is invalid), `depth change > 0`, and inclusive
 * `[0, 180]` / `[0, 90]` bounds for the diff thresholds.
 */
export function validateValidationConfig(v: ValidationConfig): ValidationConfigErrors {
  return {
    warn_shot_length_min:
      v.warn_shot_length_min < 0 ? 'settings.validation.validation.minShotLength' : null,
    warn_shot_length_max:
      v.warn_shot_length_max <= v.warn_shot_length_min
        ? 'settings.validation.validation.maxShotLength'
        : null,
    warn_depth_change_per_shot_max:
      v.warn_depth_change_per_shot_max <= 0
        ? 'settings.validation.validation.depthChange'
        : null,
    azimuth_diff_threshold:
      v.azimuth_diff_threshold < 0 || v.azimuth_diff_threshold > 180
        ? 'settings.validation.validation.azimuthDiff'
        : null,
    pitch_diff_threshold:
      v.pitch_diff_threshold < 0 || v.pitch_diff_threshold > 90
        ? 'settings.validation.validation.pitchDiff'
        : null,
    inclination_depth_diff_threshold:
      v.inclination_depth_diff_threshold < 0 || v.inclination_depth_diff_threshold > 90
        ? 'settings.validation.validation.inclinationDepthDiff'
        : null,
  };
}

/** True when the config passes every range rule. */
export function isValidationConfigValid(v: ValidationConfig): boolean {
  return Object.values(validateValidationConfig(v)).every((e) => e === null);
}

function formatDegrees(value: number, precision: number = 1): string {
  return `${formatFixed(value, undefined, precision)}°`;
}

/**
 * Check if azimuth in/out values differ from each other by more than the threshold.
 * Handles 0/360 degree wrap-around correctly.
 *
 * @param azimuthIn The incoming azimuth in degrees [0, 360)
 * @param azimuthOut The outgoing azimuth in degrees [0, 360)
 * @param threshold Maximum allowable difference between azimuth values
 * @returns true if the difference exceeds the threshold
 */
export function hasAzimuthDiscrepancy(
  azimuthIn: number,
  azimuthOut: number,
  threshold: number
): boolean {
  // Calculate the absolute difference
  let diff = Math.abs(azimuthIn - azimuthOut);
  // Normalize to [0, 180] range for wrap-around handling
  if (diff > 180) diff = 360 - diff;
  // Check if they differ by more than the threshold
  return diff > threshold;
}

/**
 * Check if pitch in/out values differ significantly.
 * Returns false if either value is null (no comparison possible).
 *
 * @param pitchIn The incoming pitch in degrees (or null)
 * @param pitchOut The outgoing pitch in degrees (or null)
 * @param threshold Maximum allowable difference between pitch values
 * @returns true if the difference exceeds the threshold
 */
export function hasPitchDiscrepancy(
  pitchIn: number | null,
  pitchOut: number | null,
  threshold: number
): boolean {
  if (pitchIn === null || pitchOut === null) return false;
  return Math.abs(pitchIn - pitchOut) > threshold;
}

// ============================================================================
// Import-specific Error Types and Validation
// ============================================================================

/**
 * Error types specific to the Import Dialog.
 * These are blocking errors that prevent import.
 */
export type ImportErrorType = 'invalidData';

/**
 * An error for a section during import.
 */
export interface ImportError {
  type: ImportErrorType;
  sectionIndex: number;
  shotIndex?: number;
  field?: string;
  message: string;
}

/**
 * Errors for a single section.
 */
export interface SectionErrors {
  sectionIndex: number;
  errors: ImportError[];
}

/**
 * Validate editable shot data for import errors.
 * Only checks for blocking errors (not warnings).
 *
 * @param shot The editable shot to validate
 * @param shotIndex The index of this shot
 * @param sectionIndex The section this shot belongs to
 * @returns Array of import errors (empty if valid)
 */
export function validateEditableShot(
  shot: {
    length: number;
    azimuth_in: number;
    azimuth_out: number;
    isDeleted?: boolean;
  },
  shotIndex: number,
  sectionIndex: number
): ImportError[] {
  const errors: ImportError[] = [];

  // Skip deleted shots
  if (shot.isDeleted) {
    return errors;
  }

  // Length must be positive (zero or negative is an error)
  if (shot.length <= 0) {
    errors.push({
      type: 'invalidData',
      sectionIndex,
      shotIndex,
      field: 'length',
      message: i18n.t('validation.import.lengthPositive'),
    });
  }

  // Azimuth in must be in range [0, 360)
  if (shot.azimuth_in < 0 || shot.azimuth_in >= 360) {
    errors.push({
      type: 'invalidData',
      sectionIndex,
      shotIndex,
      field: 'azimuth_in',
      message: i18n.t('validation.import.azimuthInRange', {
        value: formatDegrees(shot.azimuth_in),
      }),
    });
  }

  // Azimuth out must be in range [0, 360)
  if (shot.azimuth_out < 0 || shot.azimuth_out >= 360) {
    errors.push({
      type: 'invalidData',
      sectionIndex,
      shotIndex,
      field: 'azimuth_out',
      message: i18n.t('validation.import.azimuthOutRange', {
        value: formatDegrees(shot.azimuth_out),
      }),
    });
  }

  return errors;
}
