/**
 * Angle utilities for cave survey calculations.
 *
 * The two azimuth helpers here are the ones in `azimuth.ts`, which mirror the
 * backend (#576 D14). This file used to carry its own bodies: a unit-vector
 * mean and a normaliser that folded NaN to 0. They agreed with the backend for
 * ordinary shots but not for exactly opposite legs, so the DataTable's Avg
 * column showed 198.43 for legs 100/280 next to a map and warning previews that
 * used the backend's 10, and an edit of that cell rotated both legs by the
 * wrong amount. One implementation means one answer on screen.
 */
import { averageAzimuth, normalizeAzimuth } from './azimuth';

/**
 * Normalize an angle to the range [0, 360); see `normalizeAzimuth`.
 *
 * A non-finite angle passes through unchanged rather than becoming 0: "no
 * bearing" turned into due north passes range validation and saves.
 */
export const normalizeAngle = normalizeAzimuth;

/**
 * Mean of two azimuth legs in degrees, across the short arc (350 and 10 give
 * 0, not 180); see `averageAzimuth`. Exactly opposite legs have no mean, and
 * the tie resolves the way the backend resolves it (to `a - 90`).
 */
export const circularMeanDegrees = averageAzimuth;

/**
 * The bearing a pair of azimuth legs displays: the circular mean of two measured
 * legs, the lone leg when only one was measured, NaN when neither was.
 *
 * A shot with one leg is a real, saveable shot (the missing leg is duplicated at
 * commit), but `circularMeanDegrees` against a NaN leg is NaN - which rendered
 * that bearing as an empty cell in every average-azimuth column.
 *
 * @param a - First leg in degrees; non-finite means unmeasured
 * @param b - Second leg in degrees; non-finite means unmeasured
 * @returns The displayed average, or NaN when there is nothing to show
 */
export function meanOrLoneLeg(a: number, b: number): number {
  const aOk = Number.isFinite(a);
  const bOk = Number.isFinite(b);
  if (aOk && bOk) return circularMeanDegrees(a, b);
  if (aOk) return a;
  return bOk ? b : NaN;
}
