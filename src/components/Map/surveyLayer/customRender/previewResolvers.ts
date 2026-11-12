/**
 * Colour resolvers for the live colour-scale PREVIEW (palette hover, Levels
 * drag). Same shape as {@link makeColorResolvers}, but the depth ramp and the
 * distance overlay read a palette baked into a lookup table instead of
 * interpolating and formatting a CSS string per vertex, so a recolour of a
 * large survey fits in a frame. Every other mode (severity, overlays without a
 * scale, comment tint, plan idle) delegates to the exact resolvers.
 *
 * The preview is approximate by up to one 8-bit step (LUT quantisation). The
 * controller replaces it with an exact colour-only recolour when the gesture
 * ends, so the approximation is never the resting state.
 */
import type { SurveyData } from '../../../../types/survey-data';
import { encodeFeatureId } from '../../../../utils/ids';
import { bakeLut } from '../../../../utils/palettes/sample';
import { effectiveScale, type ColorScaleInput } from '../../../../utils/palettes/scale';
import type { LevelsScale } from '../../../../utils/palettes/types';
import type { DisplayColorContext } from '../displayColor';
import { packCssColor, type Rgba8 } from './colorPack';
import { makeColorResolvers, type ColorResolvers } from './colorResolvers';

// 4096 entries: fine enough that a steep palette band (a metallic highlight,
// say) stays within a couple of 8-bit levels of the exact colour.
export const PREVIEW_LUT_SIZE = 4096;

/** value -> packed colour through a baked LUT, or null for a non-finite value
 *  (the caller falls back to the exact resolver). */
type LutColor = (value: number) => Rgba8 | null;

function makeLutColor(input: ColorScaleInput, auto: { min: number; max: number }): LutColor {
  const lut = bakeLut(input.palette, PREVIEW_LUT_SIZE);
  const entries: Rgba8[] = [];
  for (let i = 0; i < PREVIEW_LUT_SIZE; i++) {
    entries.push([lut[i * 4]!, lut[i * 4 + 1]!, lut[i * 4 + 2]!, 255]);
  }
  const s: LevelsScale = effectiveScale(input, auto);
  const span = s.max - s.min;
  const flat = !(span > 0);
  const invGamma = 1 / s.gamma;
  const last = PREVIEW_LUT_SIZE - 1;
  return (value: number): Rgba8 | null => {
    if (!Number.isFinite(value)) return null;
    if (flat) return entries[0]!;
    let t = (value - s.min) / span;
    t = t <= 0 ? 0 : t >= 1 ? 1 : s.gamma === 1 ? t : Math.pow(t, invGamma);
    return entries[Math.round(t * last)]!;
  };
}

export function makePreviewResolvers(surveyData: SurveyData, colorCtx: DisplayColorContext): ColorResolvers {
  const exact = makeColorResolvers(surveyData, colorCtx);
  const overlay = colorCtx.overlay ?? null;

  // Distance overlay with a scale: LUT for reachable values, grey otherwise.
  if (overlay && overlay.mode === 'distance' && overlay.scale) {
    const lut = makeLutColor(overlay.scale, { min: 0, max: overlay.max > 0 ? overlay.max : 1 });
    const grey = packCssColor(colorCtx.themeColors.grayLine);
    const colorOf = (value: number | undefined): Rgba8 =>
      value === undefined || value < 0 ? grey : (lut(value) ?? grey);
    const colorForShot = (g: number, sectionId: number): Rgba8 =>
      colorOf(overlay.shots.get(encodeFeatureId(sectionId, surveyData.shotIds[g]!)));
    return {
      ...exact,
      colorForShot,
      colorForShotEndpoints: (g, sectionId) => {
        const c = colorForShot(g, sectionId);
        return [c, c];
      },
      fillForStation: (g) =>
        colorOf(
          overlay.stations.get(encodeFeatureId(surveyData.stationSectionIds[g]!, surveyData.stationIndices[g]!)),
        ),
    };
  }

  // Plain depth mode (the resolvers' own depth-endpoint condition).
  if (exact.useDepthEndpoints && colorCtx.depthScale) {
    const lut = makeLutColor(colorCtx.depthScale, colorCtx.depthRange);
    const { shotDepthIn, shotDepthOut, stationCoords } = surveyData;
    return {
      ...exact,
      colorForShot: (g, sectionId) =>
        lut((shotDepthIn[g]! + shotDepthOut[g]!) / 2) ?? exact.colorForShot(g, sectionId),
      colorForShotEndpoints: (g, sectionId) => {
        const a = lut(shotDepthIn[g]!);
        const b = lut(shotDepthOut[g]!);
        return a && b ? [a, b] : exact.colorForShotEndpoints(g, sectionId);
      },
      fillForStation: (g) => lut(stationCoords[g * 3 + 2]!) ?? exact.fillForStation(g),
    };
  }

  return exact;
}

/**
 * The global shot index behind each drawn line segment, in the order the line
 * buffers were built (replaying `recomputeLineColors`' valid-shot iteration).
 * Computed once per built geometry so a preview frame is a flat loop instead of
 * two map lookups per shot.
 */
export function segmentShotOrder(data: SurveyData, lookup: Map<number, Map<number, number>>): Uint32Array {
  const order: number[] = [];
  for (const section of data.sections) {
    const inner = lookup.get(section.id);
    if (!inner) continue;
    for (let i = 0; i < section.shotCount; i++) {
      if (inner.get(i) === undefined || inner.get(i + 1) === undefined) continue;
      order.push(section.shotOffset + i);
    }
  }
  return Uint32Array.from(order);
}

/**
 * Write a preview recolour into the line / circle colour arrays in place. Line
 * colours per segment (endpoint pair in depth mode, flat otherwise) and station
 * FILLS only: no palette or range changes a station's stroke.
 */
export function writePreviewColors(
  data: SurveyData,
  segShots: Uint32Array,
  res: ColorResolvers,
  line: { colorFrom: Uint8Array; colorTo: Uint8Array },
  fill: Uint8Array,
): void {
  const { colorFrom, colorTo } = line;
  const sectionIds = data.shotSectionIds;
  for (let seg = 0; seg < segShots.length; seg++) {
    const g = segShots[seg]!;
    const sectionId = sectionIds[g]!;
    let a: Rgba8;
    let b: Rgba8;
    if (res.useDepthEndpoints) {
      [a, b] = res.colorForShotEndpoints(g, sectionId);
    } else {
      a = res.colorForShot(g, sectionId);
      b = a;
    }
    const base = seg * 16;
    for (let o = base; o < base + 16; o += 4) {
      colorFrom[o] = a[0];
      colorFrom[o + 1] = a[1];
      colorFrom[o + 2] = a[2];
      colorFrom[o + 3] = a[3];
      colorTo[o] = b[0];
      colorTo[o + 1] = b[1];
      colorTo[o + 2] = b[2];
      colorTo[o + 3] = b[3];
    }
  }
  for (let g = 0; g < data.stationCount; g++) {
    const c = res.fillForStation(g);
    const base = g * 16;
    for (let o = base; o < base + 16; o += 4) {
      fill[o] = c[0];
      fill[o + 1] = c[1];
      fill[o + 2] = c[2];
      fill[o + 3] = c[3];
    }
  }
}
