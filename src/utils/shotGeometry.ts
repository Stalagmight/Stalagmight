/**
 * Small, pure shot-geometry helpers shared by the Warnings resolver and the
 * map's survey-quality metrics, so both read a shot the way the Rust validator
 * (`core::validation`) does.
 *
 * Inclination is +down everywhere (docs/inclination.md): positive means the
 * shot descends and depth increases, the frame `depth_in` / `depth_out` are in.
 */

/** Valid range of a recorded inclination leg, in degrees. */
export const PITCH_LIMIT_DEG = 90;

/** Whether a recorded inclination leg is physically valid. A missing leg
 *  (null) counts as valid: there is nothing to reject. */
export function pitchLegInRange(pitch: number | null): boolean {
  return pitch === null || Math.abs(pitch) <= PITCH_LIMIT_DEG;
}

/** The recorded inclination: the mean of both legs, else the one that was
 *  recorded, else null. Mirrors the validator's `recorded_pitch`. */
export function recordedPitch(
  pitchIn: number | null | undefined,
  pitchOut: number | null | undefined,
): number | null {
  if (pitchIn != null && pitchOut != null) return (pitchIn + pitchOut) / 2;
  return pitchIn ?? pitchOut ?? null;
}

/** The inclination a shot's own slope length and depth change imply, in
 *  degrees (+down). Mirrors `implied_pitch_deg` in the Rust models crate,
 *  where `atan2(dz, sqrt(L^2 - dz^2))` equals `asin(dz / L)` clamped to
 *  [-90, 90]. Returns 0 for a non-positive length. */
export function impliedPitchDeg(length: number, depthIn: number, depthOut: number): number {
  if (!(length > 0)) return 0;
  const ratio = (depthOut - depthIn) / length;
  return (Math.asin(Math.max(-1, Math.min(1, ratio))) * 180) / Math.PI;
}

/** The angle between two azimuths, in [0, 180] degrees, after wrapping both
 *  into [0, 360). NaN when either is not finite. */
export function azimuthDelta(azimuthIn: number, azimuthOut: number): number {
  if (!Number.isFinite(azimuthIn) || !Number.isFinite(azimuthOut)) return NaN;
  const d = (((azimuthOut - azimuthIn) % 360) + 360) % 360;
  return d > 180 ? 360 - d : d;
}
