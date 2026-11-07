/**
 * Geometry for the on-map preview of a proposed shot edit in the warning
 * resolution flow (issue #57).
 *
 * Mirrors the backend solve (`src-tauri/src/core/state/commands.rs`): a shot's
 * plan-view step is `average_azimuth` with `dx = horizontal·sin(az)`,
 * `dy = horizontal·cos(az)`. The ghost is anchored to the *real solved*
 * endpoint and shows only the delta, so when the proposed value equals the
 * current one the ghost overlaps reality exactly.
 */
import { calculatePosition } from './coordinates';

export interface Pt {
  x: number;
  y: number;
}

/**
 * Horizontal (plan) projection of a slope length given its depth change: the
 * station walk's one copy of `horizontal_length_from_slope` (#576 NF-W4M-5),
 * overflow guard included - the bare `sqrt(l^2 - dz^2)` this used to be turned
 * a tape long enough to overflow its square into NaN, where the backend's
 * scales into [0, 1] first and stays finite.
 */
export { horizontalLength } from './stationWalk';

/**
 * Ghost endpoint for a proposed shot edit, or `null` when the planar model
 * can't reproduce the current solved endpoint within tolerance - e.g. a
 * rotated/offset section whose solved frame differs from raw azimuths. The
 * caller shows no ghost in that case rather than a misleading one.
 *
 * @param start          solved coords of the shot's start station (meters)
 * @param end            solved coords of the shot's end station (meters)
 * @param currentAzimuth the shot's current average azimuth (deg)
 * @param currentHoriz   the shot's current horizontal length (m)
 * @param proposedAzimuth proposed average azimuth (deg)
 * @param proposedHoriz  proposed horizontal length (m)
 */
export function computeGhostEnd(
  start: Pt,
  end: Pt,
  currentAzimuth: number,
  currentHoriz: number,
  proposedAzimuth: number,
  proposedHoriz: number,
): Pt | null {
  const predicted = calculatePosition({ x: start.x, y: start.y, z: 0 }, currentAzimuth, currentHoriz, 0);
  // Tolerance scales a little with length to absorb float noise on long shots.
  if (Math.hypot(predicted.x - end.x, predicted.y - end.y) > 0.1 + 0.05 * currentHoriz) {
    return null;
  }
  const proposedVec = calculatePosition({ x: 0, y: 0, z: 0 }, proposedAzimuth, proposedHoriz, 0);
  const currentVec = calculatePosition({ x: 0, y: 0, z: 0 }, currentAzimuth, currentHoriz, 0);
  return { x: end.x + (proposedVec.x - currentVec.x), y: end.y + (proposedVec.y - currentVec.y) };
}

// Below ~2.9° a recorded inclination can't meaningfully imply a slope length
// (length = |dz| / |sin θ| explodes), so length-from-inclination is withheld.
export const MIN_SIN_INCLINATION = 0.05;

/**
 * Slope length implied by an inclination and a depth change:
 * `length = |dz| / |sin(pitch)|`. Returns `null` when there is no pitch, or when
 * `|sin(pitch)| <= MIN_SIN_INCLINATION` (the floor that keeps the quotient from
 * exploding). This is the single home of the inclination→length floor predicate;
 * all callers reject via `lengthFromInclination(...) === null`.
 *
 * @param depthChange the shot's depth change `depth_out - depth_in` (m; sign ignored)
 * @param pitchDeg    the inclination in degrees, or `null` when none is recorded
 */
export function lengthFromInclination(depthChange: number, pitchDeg: number | null): number | null {
  if (pitchDeg === null) return null;
  const sin = Math.sin((pitchDeg * Math.PI) / 180);
  if (Math.abs(sin) <= MIN_SIN_INCLINATION) return null;
  return Math.abs(depthChange) / Math.abs(sin);
}

/**
 * Whether keeping the recorded inclination and only adjusting the slope length can
 * satisfy the geometry with a POSITIVE length. In the +down convention the descent
 * (`depth_out - depth_in = depthChange`) must equal `length * sin(pitch)` with
 * `length > 0`, so the recorded pitch has to point the SAME way as the depth
 * change. A reversed-sign inclination (e.g. a 5 m descent recorded as `-30°`) has
 * no positive-length solution - it can only be fixed by adjusting the inclination -
 * and a zero depth change has none either, so the "adjust length"/"split" options
 * are withheld for those (issue #57 / Codex, issue #296).
 *
 * @param depthChange `depth_out - depth_in` (m)
 * @param pitchDeg    the recorded inclination in degrees, or null when none
 */
export function lengthFixableFromInclination(depthChange: number, pitchDeg: number | null): boolean {
  if (pitchDeg === null) return false;
  const sin = Math.sin((pitchDeg * Math.PI) / 180);
  if (Math.abs(sin) <= MIN_SIN_INCLINATION) return false;
  return depthChange / sin > 0;
}

/**
 * Whether the proposed slope `length` is shorter than the vertical change it
 * must span - i.e. `length + ε < |depthOut - depthIn|`, the same condition the
 * backend raises `LENGTH_BELOW_DEPTH_CHANGE` for (validation.rs uses ε = 1e-6).
 * A preview built from such values would draw a clamped, misleading ghost, and
 * applying it raises an error on revalidation (issue #57). Requires a positive
 * length and finite depths; returns `false` otherwise.
 */
export function exceedsSlopeLength(length: number, depthIn: number, depthOut: number): boolean {
  if (!(length > 0) || !Number.isFinite(depthIn) || !Number.isFinite(depthOut)) return false;
  return length + 1e-6 < Math.abs(depthOut - depthIn);
}
