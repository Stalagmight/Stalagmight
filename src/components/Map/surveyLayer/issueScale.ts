/**
 * The Survey Issues score scale, on its own so the colour ramp (imported by
 * `displayColor.ts`, which some Playwright specs load in Node) does not pull in
 * the metric code and its i18n-formatting dependencies.
 */

/** Score at a check's warning threshold (yellow). */
export const SCORE_WARNING = 1;
/** Highest score: an error, or 3x a warning threshold (red). */
export const SCORE_MAX = 3;
