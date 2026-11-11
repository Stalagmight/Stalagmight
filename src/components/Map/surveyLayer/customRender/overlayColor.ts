/**
 * Custom-render analytical-overlay colors (#34 distance gradient, #38 gas
 * reachable-frontier).
 *
 * The stock bulk layers color these overlays with a data-driven paint ramp
 * (`distanceColorExpression` / `gasReachColorExpression`) over a `feature-state`
 * value; MapLibre evaluates that ramp per feature on the CPU and packs it via
 * `packUint8ToFloat(255 * channel)` = `Math.floor`, and the 8-bit framebuffer
 * keeps the byte. The custom layer has no feature-state, so it must resolve the
 * SAME color per feature at build time. These pure helpers reproduce MapLibre's
 * exact float path (`floor(255 * (near01 + t*(far01-near01)))` with
 * `near01 = nearByte/255`) so the custom vertex color is byte-identical to the
 * stock ramp pixel. No GL, no DOM: unit-testable in the `node` vitest env.
 */
import { DISTANCE_NEAR, DISTANCE_FAR } from '../../../../utils/distanceGradient';
import { GAS_NEAR, GAS_FAR, BEYOND_GAS_RANGE } from '../../../../utils/gasReach';
import type { DistanceFeatureState } from '../../../../utils/distanceGradient';
import type { GasReachFeatureState } from '../../../../utils/gasReach';
import type { OverlayColorInput } from '../displayColor';
import { packCssColor, type Rgba8 } from './colorPack';
import { effectiveScale, levelsT, type ColorScaleInput } from '../../../../utils/palettes';

/** `Math.floor` + clamp to 0..255, mirroring MapLibre's `packUint8ToFloat`
 *  (which floors, unlike `colorPack.packCssColor`, which rounds). */
function clampByteFloor(x: number): number {
  if (Number.isNaN(x)) return 0;
  if (x < 0) return 0;
  if (x > 255) return 255;
  return Math.floor(x);
}

/**
 * One interpolated channel, byte-exact to MapLibre's data-driven color path.
 * `near01 = nearByte/255` matches `parseCssColor` (which divides hex by 255);
 * `floor(255 * (near01 + t*(far01-near01)))` matches `interpolateNumber` +
 * `packColor` (`floor(255 * channel)`) op-for-op, so the rounding quirks match.
 */
function lerpChannelFloor(nearByte: number, farByte: number, t: number): number {
  const near01 = nearByte / 255;
  const far01 = farByte / 255;
  return clampByteFloor(255 * (near01 + t * (far01 - near01)));
}

/**
 * The packed ramp color for one feature value. Mirrors the stock expression:
 * `value === BEYOND_GAS_RANGE` (connected but past the gas/turn budget) ->
 * `far` - the ramp's own far-end color, clamped, never the feature's base
 * color; any other `value < 0` (topologically unconnected) -> grey; otherwise
 * interpolate near->far over `0 -> safeMax`, with the endpoint short-circuits
 * MapLibre's `interpolate` evaluate applies (`value <= 0` -> the exact near
 * color, `value >= safeMax` -> the exact far color) so the boundaries never
 * drift by a ULP.
 */
export function packRampColor(
  value: number,
  safeMax: number,
  near: Rgba8,
  far: Rgba8,
  grey: Rgba8,
): Rgba8 {
  if (value === BEYOND_GAS_RANGE) return far;
  if (value < 0) return grey;
  if (value <= 0) return near;
  if (value >= safeMax) return far;
  const t = value / safeMax;
  return [
    lerpChannelFloor(near[0], far[0], t),
    lerpChannelFloor(near[1], far[1], t),
    lerpChannelFloor(near[2], far[2], t),
    lerpChannelFloor(near[3], far[3], t),
  ];
}

/**
 * A `value -> packed ramp color` closure for an active overlay. The near/far
 * hues are chosen by `mode` (distance and gas reuse the same gold->purple ramp
 * but stay independently sourced) and grey is the theme's gray line (the same
 * `grayLine` the stock ramp uses for unreachable). `safeMax` guards a degenerate
 * `max <= 0` exactly like the stock expression.
 */
export function makeOverlayColor(
  overlay: OverlayColorInput,
  grayLine: string,
): (value: number) => Rgba8 {
  if (overlay.mode === 'distance' && overlay.scale) {
    return makeScaledOverlayColor(overlay.scale, overlay.max, packCssColor(grayLine));
  }
  const [near, far] =
    overlay.mode === 'distance'
      ? [packCssColor(DISTANCE_NEAR), packCssColor(DISTANCE_FAR)]
      : [packCssColor(GAS_NEAR), packCssColor(GAS_FAR)];
  const grey = packCssColor(grayLine);
  const safeMax = overlay.max > 0 ? overlay.max : 1;
  return (value: number): Rgba8 => packRampColor(value, safeMax, near, far, grey);
}

/** Opaque magenta: the colour-pack fallback, for an unusable palette entry. */
const UNUSABLE: Rgba8 = [255, 0, 255, 255];

/**
 * The distance overlay through the user's palette + Levels range. Unreachable
 * (`< 0`) stays grey and a NaN stays the transparent black `packRampColor`
 * produces, so only the reachable colours change. With the Theme palette and an
 * auto range this is byte-identical to `packRampColor` (pinned in
 * src/utils/palettes/__tests__).
 */
function makeScaledOverlayColor(input: ColorScaleInput, max: number, grey: Rgba8): (value: number) => Rgba8 {
  const scale = effectiveScale(input, { min: 0, max: max > 0 ? max : 1 });
  const flat = scale.max === scale.min;
  return (value: number): Rgba8 => {
    if (Number.isNaN(value)) return [0, 0, 0, 0];
    if (value < 0) return grey;
    const c = flat ? input.palette.rgb(0) : input.palette.rgb(levelsT(value, scale));
    return c ? [c[0], c[1], c[2], 255] : UNUSABLE;
  };
}

/**
 * Build the {@link OverlayColorInput} for the CURRENTLY active overlay from
 * SurveyLayer's distance / gas state, or `null` when neither is on. Distance
 * wins if both are somehow set, mirroring `shotsLineColor`'s precedence. Kept
 * here (not inline in SurveyLayer) so the mode -> value-map wiring lives beside
 * the ramp that consumes it.
 */
export function overlayInputFrom(
  distanceMode: boolean,
  distanceState: DistanceFeatureState | null,
  gasReachMode: boolean,
  gasReachState: GasReachFeatureState | null,
  distanceScale: ColorScaleInput | null = null,
): OverlayColorInput | null {
  if (distanceMode && distanceState) {
    const { shots, stations, max } = distanceState;
    return distanceScale
      ? { mode: 'distance', shots, stations, max, scale: distanceScale }
      : { mode: 'distance', shots, stations, max };
  }
  if (gasReachMode && gasReachState) {
    const { shots, stations, max } = gasReachState;
    return { mode: 'gasReach', shots, stations, max };
  }
  return null;
}
