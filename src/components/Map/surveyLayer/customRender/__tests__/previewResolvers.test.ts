/**
 * The live colour-scale preview: LUT-backed resolvers stay within one 8-bit
 * step of the exact resolvers, and fall back to them outside depth / scaled
 * distance mode. The timing check is opt-in (STALAGMIGHT_PERF=1) so a slow CI
 * runner cannot flake it; run it locally when touching this path.
 */
import { describe, expect, it } from 'vitest';
import { makePreviewResolvers, segmentShotOrder, writePreviewColors } from '../previewResolvers';
import { makeColorResolvers } from '../colorResolvers';
import { recomputeCircleColors, recomputeLineColors } from '../tessellate';
import { surveyDataFromLegacy, type SurveyData } from '../../../../../types/survey-data';
import { getMapThemeColors } from '../../../../../utils/themeColors';
import { resolveChoice, type ColorScaleInput } from '../../../../../utils/palettes';
import type { DisplayColorContext } from '../../displayColor';
import type { Section } from '../../../../../types/ipc/Section';
import type { Station } from '../../../../../types/ipc/Station';

const theme = getMapThemeColors();

function synthetic(sectionCount: number, shotsPerSection: number): SurveyData {
  const sections: Section[] = [];
  const stations: Station[] = [];
  for (let s = 1; s <= sectionCount; s++) {
    const shots = [];
    for (let i = 0; i < shotsPerSection; i++) {
      const d0 = -((s * 7 + i) % 120) - Math.sin(i) * 3;
      shots.push({
        id: s * 100_000 + i, azimuth_in: 0, azimuth_out: 0, length: 4, depth_in: d0, depth_out: d0 - 1.5,
        pitch_in: null, pitch_out: null, left: null, right: null, up: null, down: null, comment: null,
        kind: 'Normal' as const, marker_index: null, temperature: null,
      });
    }
    sections.push({
      id: s, name: `S${s}`, color: '#ff0000', direction: 'in', quality: 1,
      next_shot_id: shotsPerSection, reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null,
      shots,
    });
    for (let i = 0; i <= shotsPerSection; i++) {
      stations.push({
        station_ref: { section_id: s, station_index: i }, coordinates: { x: i * 4, y: s * 10, z: -((s * 7 + i) % 120) },
        is_zero_datum: false, gps_coordinates: null, merged_with: [], closures_with: [],
      });
    }
  }
  return surveyDataFromLegacy(sections, stations);
}

function lookupOf(data: SurveyData): Map<number, Map<number, number>> {
  const lookup = new Map<number, Map<number, number>>();
  for (let g = 0; g < data.stationCount; g++) {
    const sid = data.stationSectionIds[g]!;
    let inner = lookup.get(sid);
    if (!inner) lookup.set(sid, (inner = new Map()));
    inner.set(data.stationIndices[g]!, g);
  }
  return lookup;
}

function buffers(data: SurveyData) {
  const segs = data.shotCount;
  return {
    line: { colorFrom: new Uint8Array(segs * 16), colorTo: new Uint8Array(segs * 16) },
    circle: { fill: new Uint8Array(data.stationCount * 16), stroke: new Uint8Array(data.stationCount * 16) },
  };
}

function recolour(
  data: SurveyData,
  ctx: DisplayColorContext,
  preview: boolean,
  b = buffers(data),
  lookup = lookupOf(data),
) {
  if (preview) {
    // The controller's preview path: flat segment loop, fills only (strokes
    // keep what the last exact build wrote, so seed them from it here).
    const exactRes = makeColorResolvers(data, ctx);
    recomputeCircleColors(b.circle as never, data, exactRes.fillForStation, exactRes.strokeForStation);
    writePreviewColors(data, segmentShotOrder(data, lookup), makePreviewResolvers(data, ctx), b.line, b.circle.fill);
    return b;
  }
  const res = makeColorResolvers(data, ctx);
  recomputeLineColors(
    b.line as never, data, lookup, res.colorForShot,
    res.useDepthEndpoints ? res.colorForShotEndpoints : undefined,
  );
  recomputeCircleColors(b.circle as never, data, res.fillForStation, res.strokeForStation);
  return b;
}

const depthCtx = (input: ColorScaleInput): DisplayColorContext => ({
  showDepthGradient: true, depthRange: { min: -125, max: 0 }, themeColors: theme, showIssueSeverity: false,
  depthScale: input,
});

function maxByteDiff(a: Uint8Array, b: Uint8Array): number {
  let worst = 0;
  for (let i = 0; i < a.length; i++) worst = Math.max(worst, Math.abs(a[i]! - b[i]!));
  return worst;
}

describe('colour-scale preview resolvers', () => {
  const data = synthetic(20, 150);

  for (const [id, scale] of [
    ['theme', null],
    ['mpl.viridis', { min: -100, max: -10, gamma: 1 }],
    ['mpl.twilight', { min: -125, max: 0, gamma: 2.5 }],
    ['grad.chrome', { min: -60, max: -20, gamma: 0.4 }],
  ] as const) {
    it(`stays within one 8-bit step of the exact colours (${id})`, () => {
      const input = { palette: resolveChoice({ id, reversed: false }, 'depth', theme), scale };
      const exact = recolour(data, depthCtx(input), false);
      const preview = recolour(data, depthCtx(input), true);
      // The LUT's nearest entry over a steep palette segment can land a level
      // or two off; anything past that would be a real bug.
      expect(maxByteDiff(exact.line.colorFrom, preview.line.colorFrom)).toBeLessThanOrEqual(2);
      expect(maxByteDiff(exact.line.colorTo, preview.line.colorTo)).toBeLessThanOrEqual(2);
      expect(maxByteDiff(exact.circle.fill, preview.circle.fill)).toBeLessThanOrEqual(2);
      expect(maxByteDiff(exact.circle.stroke, preview.circle.stroke)).toBe(0);
    });
  }

  it('uses the exact resolvers outside depth / scaled-distance mode', () => {
    const ctx: DisplayColorContext = {
      showDepthGradient: false, depthRange: { min: -125, max: 0 }, themeColors: theme, showIssueSeverity: false,
    };
    const exact = recolour(data, ctx, false);
    const preview = recolour(data, ctx, true);
    expect(maxByteDiff(exact.line.colorFrom, preview.line.colorFrom)).toBe(0);
    expect(maxByteDiff(exact.circle.fill, preview.circle.fill)).toBe(0);
  });

  it.skipIf(!process.env.STALAGMIGHT_PERF)('recolours ~75k shots inside a frame budget', () => {
    const big = synthetic(100, 750);
    const input = { palette: resolveChoice({ id: 'mpl.viridis', reversed: false }, 'depth', theme), scale: null };
    const ctx = depthCtx(input);
    // Buffers and the station lookup exist already in the controller; time only
    // the resolvers + in-place recolour, as a preview frame runs them.
    const b = buffers(big);
    const lookup = lookupOf(big);
    const order = segmentShotOrder(big, lookup);
    const frame = () => writePreviewColors(big, order, makePreviewResolvers(big, ctx), b.line, b.circle.fill);
    frame(); // warm up
    const runs = 10;
    let t = performance.now();
    for (let i = 0; i < runs; i++) frame();
    const preview = (performance.now() - t) / runs;
    t = performance.now();
    recolour(big, ctx, false, b, lookup);
    const exact = performance.now() - t;
    console.log(`preview recolour ${preview.toFixed(1)} ms, exact ${exact.toFixed(1)} ms (${big.shotCount} shots, ${big.stationCount} stations)`);
    expect(preview).toBeLessThan(8);
  });
});
