import { afterEach, describe, expect, it, vi } from 'vitest';
import { packCssColor, writeCssColor } from '../colorPack';
import { SurveyCustomLayer, type CustomRenderData } from '../surveyCustomLayer';
import { mat4Multiply, composeAnchorMatrix, computeMercatorAnchor } from '../mercator';
import {
  buildStationLookup,
  computeStationAllReference,
  projectStationsRel,
  countValidShots,
  buildLineBuffers,
  buildCircleBuffers,
  computeLineDim,
  computeCircleDim,
  recomputeLineColors,
  recomputeCircleColors,
  writeCircleDrawOrder,
} from '../tessellate';
import { planColorOnlyUpdate } from '../incremental';
import type { DisplayColorContext } from '../../displayColor';
import {
  LINE_VERTEX_SRC,
  LINE_FRAGMENT_SRC,
  CIRCLE_VERTEX_SRC,
  CIRCLE_FRAGMENT_SRC,
} from '../shaders';
import type { Rgba8 } from '../colorPack';
import {
  emptySurveyData,
  shotKindToFlags,
  STATION_FLAG_IS_ZERO_DATUM,
  type SurveyData,
} from '../../../../../types/survey-data';
import { lngLatToMercator } from '../../mercatorCoords';
import { metersToMapCoords } from '../../../../../utils/coordinates';

/**
 * Minimal fixture: two sections.
 *  - Section 1: 3 stations (idx 0,1,2) + 2 shots -> both shots valid (2 segments).
 *  - Section 2: 1 station (idx 0) + 1 shot -> missing endpoint (idx 1) -> 0 segments.
 * Station 0 of section 1 is a zero-datum. Total stations = 4.
 */
function makeFixture(): SurveyData {
  const d = emptySurveyData();
  d.sections = [
    { id: 1, name: 'A', surveyor: null, date: null, color: '#ff0000', direction: 'in' as never, quality: 0, next_shot_id: 2, shotOffset: 0, shotCount: 2 },
    { id: 2, name: 'B', surveyor: null, date: null, color: '#00ff00', direction: 'in' as never, quality: 0, next_shot_id: 1, shotOffset: 2, shotCount: 1 },
  ];
  d.shotCount = 3;
  d.shotIds = new Uint32Array([0, 1, 0]);
  d.shotSectionIds = new Uint32Array([1, 1, 2]);
  d.shotFlags = new Uint8Array([0, 0, 0]);

  d.stationCount = 4;
  d.stationSectionIds = new Uint32Array([1, 1, 1, 2]);
  d.stationIndices = new Uint32Array([0, 1, 2, 0]);
  d.stationCoords = new Float64Array([
    0, 0, 0,
    10, 0, -1,
    10, 10, -2,
    100, 100, 0,
  ]);
  d.stationFlags = new Uint8Array([STATION_FLAG_IS_ZERO_DATUM, 0, 0, 0]);
  return d;
}

const colorCtxNoop = { showDepthGradient: false } as never;

describe('colorPack', () => {
  it('parses #rgb / #rrggbb / #rrggbbaa', () => {
    expect(packCssColor('#f00')).toEqual([255, 0, 0, 255]);
    expect(packCssColor('#ff0000')).toEqual([255, 0, 0, 255]);
    expect(packCssColor('#0000ff80')).toEqual([0, 0, 255, 128]);
    expect(packCssColor('#4ecdc4')).toEqual([78, 205, 196, 255]);
  });

  it('parses rgb() / rgba()', () => {
    expect(packCssColor('rgb(1, 2, 3)')).toEqual([1, 2, 3, 255]);
    expect(packCssColor('rgba(255, 0, 0, 0.5)')).toEqual([255, 0, 0, 128]);
  });

  it('parses hsl() / hsla()', () => {
    // Primary hues at s=100% l=50% are exact.
    expect(packCssColor('hsl(0, 100%, 50%)')).toEqual([255, 0, 0, 255]);
    expect(packCssColor('hsl(120, 100%, 50%)')).toEqual([0, 255, 0, 255]);
    expect(packCssColor('hsl(240, 100%, 50%)')).toEqual([0, 0, 255, 255]);
    // Grey: any hue, 0 saturation, 50% lightness.
    expect(packCssColor('hsl(200, 0%, 50%)')).toEqual([128, 128, 128, 255]);
    // The global view's per-system form (70% sat / 50% light) parses to a
    // credible mid-tone (not the magenta fallback) and honors alpha.
    const c = packCssColor('hsl(210, 70%, 50%)');
    expect(c).not.toEqual([255, 0, 255, 255]);
    expect(packCssColor('hsla(0, 100%, 50%, 0.5)')).toEqual([255, 0, 0, 128]);
  });

  it('returns a visible fallback for garbage', () => {
    expect(packCssColor('not-a-color')).toEqual([255, 0, 255, 255]);
    expect(packCssColor('#12345')).toEqual([255, 0, 255, 255]);
  });

  it('writes into a Uint8Array at an offset', () => {
    const out = new Uint8Array(8);
    writeCssColor(out, 4, '#010203');
    expect(Array.from(out)).toEqual([0, 0, 0, 0, 1, 2, 3, 255]);
  });
});

describe('mat4 + anchor matrix', () => {
  const identity = new Float64Array([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]);

  it('mat4Multiply is identity-neutral', () => {
    const m = new Float64Array([2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53]);
    expect(Array.from(mat4Multiply(identity, m))).toEqual(Array.from(m));
    expect(Array.from(mat4Multiply(m, identity))).toEqual(Array.from(m));
  });

  it('composeAnchorMatrix == mvp * translate(anchor)', () => {
    const mvp = new Float64Array([2, 0, 0, 0, 0, 3, 0, 0, 0, 0, 1, 0, 5, 7, 0, 1]);
    const anchorX = 0.4;
    const anchorY = 0.6;
    const translate = new Float64Array([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, anchorX, anchorY, 0, 1]);
    const expected = Float32Array.from(mat4Multiply(mvp, translate));
    const got = composeAnchorMatrix(mvp, anchorX, anchorY);
    expect(Array.from(got)).toEqual(Array.from(expected));
  });

  it('anchor-relative positions reconstruct absolute mercator via the composed matrix', () => {
    // A pure translation mvp: clip = mercator + [10, 20]. The composed matrix
    // applied to (mercator - anchor) must equal mvp applied to mercator.
    const mvp = new Float64Array([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 10, 20, 0, 1]);
    const anchorX = 0.5;
    const anchorY = 0.5;
    const composed = composeAnchorMatrix(mvp, anchorX, anchorY);
    const merc = [0.5001, 0.4998];
    const local = [merc[0]! - anchorX, merc[1]! - anchorY];
    // composed * [local, 0, 1] -> x,y (column-major)
    const cx = composed[0]! * local[0]! + composed[4]! * local[1]! + composed[12]!;
    const cy = composed[1]! * local[0]! + composed[5]! * local[1]! + composed[13]!;
    expect(cx).toBeCloseTo(merc[0]! + 10, 6);
    expect(cy).toBeCloseTo(merc[1]! + 20, 6);
  });

  // Locks the input-matrix contract that MEM-92 regressed: composeAnchorMatrix
  // must be fed MapLibre's mercator [0,1] -> clip matrix
  // (defaultProjectionData.mainMatrix), NOT the world-pixel
  // modelViewProjectionMatrix. Pure math, no GL.
  it('projects the anchor point into clip with mainMatrix but off-screen with a world-pixel matrix', () => {
    // A mercator [0,1] -> clip matrix (what defaultProjectionData.mainMatrix
    // supplies): orthographic, centered on the survey's mercator center so that
    // center maps to the clip origin at a fixed scale.
    const cx = 0.512;
    const cy = 0.498;
    const s = 2000; // clip units per mercator unit for a zoomed-in view
    const mercatorMatrix = new Float64Array([
      s, 0, 0, 0,
      0, s, 0, 0,
      0, 0, 1, 0,
      -s * cx, -s * cy, 0, 1,
    ]);

    // Column-major homogeneous projection of an anchor-relative local point.
    const project = (m: Float32Array, px: number, py: number) => ({
      x: m[0]! * px + m[4]! * py + m[12]!,
      y: m[1]! * px + m[5]! * py + m[13]!,
      w: m[3]! * px + m[7]! * py + m[15]!,
    });

    // Anchor at the mercator center; the survey point sits AT the anchor, so its
    // anchor-relative local position is (0, 0). With the correct mainMatrix it
    // must land at the clip origin.
    const composed = composeAnchorMatrix(mercatorMatrix, cx, cy);
    const clip = project(composed, 0, 0);
    expect(clip.w).toBeGreaterThan(0);
    expect(Math.abs(clip.x)).toBeLessThan(1);
    expect(Math.abs(clip.y)).toBeLessThan(1);
    expect(clip.x).toBeCloseTo(0, 6);
    expect(clip.y).toBeCloseTo(0, 6);

    // The regression: MapLibre 5.16's modelViewProjectionMatrix maps world-pixel
    // space (0..worldSize, worldSize = 512 * 2^zoom) to clip, not mercator [0,1].
    // Model it by removing the 1/worldSize scaling from the projection's linear
    // block (scale it by 512 * 2^zoom) while leaving the translation. Feeding the
    // same anchor point through it lands millions of clip units off-screen.
    const worldSize = 512 * 2 ** 14; // ~8.4M px
    const worldMatrix = Float64Array.from(mercatorMatrix);
    for (const i of [0, 1, 2, 4, 5, 6, 8, 9, 10]) worldMatrix[i]! *= worldSize;
    const composedWorld = composeAnchorMatrix(worldMatrix, cx, cy);
    const clipWorld = project(composedWorld, 0, 0);
    expect(clipWorld.w).toBeGreaterThan(0);
    expect(Math.abs(clipWorld.x)).toBeGreaterThan(1e6);
  });
});

describe('anchor-relative station projection', () => {
  it('computeMercatorAnchor is the mercator bbox center', () => {
    const d = makeFixture();
    const [ax, ay] = computeMercatorAnchor(d, null);
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    for (let g = 0; g < d.stationCount; g++) {
      const ll = metersToMapCoords(d.stationCoords[g * 3]!, d.stationCoords[g * 3 + 1]!, null);
      const [mx, my] = lngLatToMercator(ll[0], ll[1]);
      minX = Math.min(minX, mx); minY = Math.min(minY, my);
      maxX = Math.max(maxX, mx); maxY = Math.max(maxY, my);
    }
    expect(ax).toBeCloseTo((minX + maxX) / 2, 12);
    expect(ay).toBeCloseTo((minY + maxY) / 2, 12);
  });

  it('projectStationsRel == mercator - anchor', () => {
    const d = makeFixture();
    const [ax, ay] = computeMercatorAnchor(d, null);
    const rel = projectStationsRel(d, null, ax, ay);
    expect(rel.length).toBe(d.stationCount * 2);
    for (let g = 0; g < d.stationCount; g++) {
      const ll = metersToMapCoords(d.stationCoords[g * 3]!, d.stationCoords[g * 3 + 1]!, null);
      const [mx, my] = lngLatToMercator(ll[0], ll[1]);
      expect(rel[g * 2]!).toBeCloseTo(mx - ax, 6);
      expect(rel[g * 2 + 1]!).toBeCloseTo(my - ay, 6);
    }
  });

  it('empty survey anchors at mercator world center', () => {
    expect(computeMercatorAnchor(emptySurveyData(), null)).toEqual([0.5, 0.5]);
  });
});

describe('line tessellation', () => {
  const stubColor = (shotIdx: number, sectionId: number): Rgba8 => [shotIdx, sectionId, 7, 255];

  it('counts only valid shots (both endpoints present)', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    expect(countValidShots(d, lookup)).toBe(2); // section 2's shot is dropped
  });

  it('sizes buffers as 4 verts + 6 indices per segment', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildLineBuffers(d, rel, lookup, stubColor);
    expect(b.segmentCount).toBe(2);
    expect(b.vertexCount).toBe(8);
    expect(b.posA.length).toBe(16);
    expect(b.posB.length).toBe(16);
    expect(b.endpoint.length).toBe(8);
    expect(b.side.length).toBe(8);
    expect(b.colorFrom.length).toBe(32);
    expect(b.colorTo.length).toBe(32);
    expect(b.indices.length).toBe(12);
  });

  it('uses the A/B endpoint + side template and quad winding', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildLineBuffers(d, rel, lookup, stubColor);
    // Segment 0 vertices: v0 A-left, v1 A-right, v2 B-left, v3 B-right.
    expect(Array.from(b.endpoint.slice(0, 4))).toEqual([0, 0, 1, 1]);
    expect(Array.from(b.side.slice(0, 4))).toEqual([-1, 1, -1, 1]);
    // Two triangles covering the quad (0,1,2)+(2,1,3).
    expect(Array.from(b.indices.slice(0, 6))).toEqual([0, 1, 2, 2, 1, 3]);
    // Segment 1 indices are offset by 4 verts.
    expect(Array.from(b.indices.slice(6, 12))).toEqual([4, 5, 6, 6, 5, 7]);
  });

  it('posA/posB are the anchor-relative segment endpoints', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const [ax, ay] = computeMercatorAnchor(d, null);
    const rel = projectStationsRel(d, null, ax, ay);
    const b = buildLineBuffers(d, rel, lookup, stubColor);
    // Segment 0 goes station idx0 -> idx1 (global 0 -> 1).
    expect(b.posA[0]).toBeCloseTo(rel[0]!, 6);
    expect(b.posA[1]).toBeCloseTo(rel[1]!, 6);
    expect(b.posB[0]).toBeCloseTo(rel[2]!, 6);
    expect(b.posB[1]).toBeCloseTo(rel[3]!, 6);
  });

  it('propagates the resolved color to all 4 vertices of a segment', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildLineBuffers(d, rel, lookup, stubColor);
    // Segment 0 = shot global 0, section 1 -> [0, 1, 7, 255]. Flat mode: colorFrom
    // and colorTo are identical (the fragment's from->to mix is a no-op).
    for (let v = 0; v < 4; v++) {
      expect(Array.from(b.colorFrom.slice(v * 4, v * 4 + 4))).toEqual([0, 1, 7, 255]);
      expect(Array.from(b.colorTo.slice(v * 4, v * 4 + 4))).toEqual([0, 1, 7, 255]);
    }
    // Segment 1 = shot global 1, section 1 -> [1, 1, 7, 255].
    expect(Array.from(b.colorFrom.slice(16, 20))).toEqual([1, 1, 7, 255]);
  });
});

describe('circle tessellation', () => {
  const fill = (g: number): Rgba8 => [g, 0, 0, 255];
  const stroke = (_g: number): Rgba8 => [9, 9, 9, 255];

  it('sizes buffers as 4 verts + 6 indices per station', () => {
    const d = makeFixture();
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildCircleBuffers(d, rel, fill, stroke);
    expect(b.stationCount).toBe(4);
    expect(b.vertexCount).toBe(16);
    expect(b.center.length).toBe(32);
    expect(b.corner.length).toBe(32);
    expect(b.fill.length).toBe(64);
    expect(b.stroke.length).toBe(64);
    expect(b.indices.length).toBe(24);
  });

  it('uses the unit-corner template + winding', () => {
    const d = makeFixture();
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildCircleBuffers(d, rel, fill, stroke);
    expect(Array.from(b.corner.slice(0, 8))).toEqual([-1, -1, 1, -1, 1, 1, -1, 1]);
    expect(Array.from(b.indices.slice(0, 6))).toEqual([0, 1, 2, 0, 2, 3]);
    expect(Array.from(b.indices.slice(6, 12))).toEqual([4, 5, 6, 4, 6, 7]);
  });

  // Where markers of several sections coincide (a jump / reference section tied
  // in at a real station), the real-line one must draw on top. One draw call, no
  // depth test: the LAST quad in the index buffer paints over the earlier ones.
  it('draws stations by rank: reference first, then jump, real last (on top)', () => {
    const d = makeFixture();
    const rel = projectStationsRel(d, null, 0, 0);
    // Kind masks per global station: 0 real, 2 reference-only, 1 jump-only, 3
    // both virtual kinds (ranks as a jump).
    const b = buildCircleBuffers(d, rel, fill, stroke, undefined, Float32Array.from([0, 2, 1, 3]));
    const drawn = (bb: typeof b) =>
      Array.from({ length: bb.stationCount }, (_, k) => bb.indices[k * 6]! / 4);
    expect(drawn(b)).toEqual([1, 2, 3, 0]);
    // Each quad keeps its own winding, just moved in the draw order.
    expect(Array.from(b.indices.slice(18, 24))).toEqual([0, 1, 2, 0, 2, 3]);
    // Vertices stay in global-index order (the incremental path relies on it).
    expect(Array.from(b.fill.slice(0, 4))).toEqual([0, 0, 0, 255]);
    expect(Array.from(b.fill.slice(4 * 4, 4 * 4 + 4))).toEqual([1, 0, 0, 255]);

    // A compacted (excluded) build orders by slot, from the masks it kept.
    const c = buildCircleBuffers(
      d, rel, fill, stroke, { stationGlobalIndices: new Set([1]) }, Float32Array.from([0, 2, 1, 3]),
    );
    expect(drawn(c)).toEqual([1, 2, 0]); // slots: 0 = global 0, 1 = global 2, 2 = global 3
  });

  it('writeCircleDrawOrder reports whether the order changed', () => {
    const d = makeFixture();
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildCircleBuffers(d, rel, fill, stroke);
    expect(writeCircleDrawOrder(b)).toBe(false); // all real: already in slot order
    // Station 3 becomes reference-only (an edit rewrote its per-vertex mask).
    b.allReference.fill(2, 12, 16);
    expect(writeCircleDrawOrder(b)).toBe(true);
    expect(b.indices[0]).toBe(12); // now drawn first, under the rest
    expect(b.indices[6]).toBe(0);
    expect(writeCircleDrawOrder(b)).toBe(false);
    // Station 0 turning reference-only too: slot order within a rank.
    b.allReference.fill(2, 0, 4);
    expect(writeCircleDrawOrder(b)).toBe(true);
    expect(Array.from({ length: 4 }, (_, k) => b.indices[k * 6]! / 4)).toEqual([0, 3, 1, 2]);
  });

  it('centers are the anchor-relative station positions; colors propagate', () => {
    const d = makeFixture();
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildCircleBuffers(d, rel, fill, stroke);
    // Station 2 (global idx 2) center at all 4 of its corners.
    for (let c = 0; c < 4; c++) {
      const v = 2 * 4 + c;
      expect(b.center[v * 2]).toBeCloseTo(rel[4]!, 6);
      expect(b.center[v * 2 + 1]).toBeCloseTo(rel[5]!, 6);
      expect(Array.from(b.fill.slice(v * 4, v * 4 + 4))).toEqual([2, 0, 0, 255]);
      expect(Array.from(b.stroke.slice(v * 4, v * 4 + 4))).toEqual([9, 9, 9, 255]);
    }
  });
});

describe('tessellation exclusion (move-drag hide semantics)', () => {
  const stubColor = (shotIdx: number, sectionId: number): Rgba8 => [shotIdx, sectionId, 7, 255];
  const fill = (g: number): Rgba8 => [g, 0, 0, 255];
  const stroke = (_g: number): Rgba8 => [9, 9, 9, 255];

  // Shared fixture handles: section 1 owns stations 0,1,2 + shots (global 0,1);
  // section 2 owns station 3 + an invalid shot (global 2, dropped).

  it('countValidShots drops excluded sections and shots, in lock-step', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    // Baseline.
    expect(countValidShots(d, lookup)).toBe(2);
    // Empty exclusion object == no exclusion.
    expect(countValidShots(d, lookup, {})).toBe(2);
    // Whole section 1 excluded -> both its shots gone.
    expect(countValidShots(d, lookup, { sectionIds: new Set([1]) })).toBe(0);
    // Single shot (global 0) excluded -> one valid shot remains.
    expect(countValidShots(d, lookup, { shotGlobalIndices: new Set([0]) })).toBe(1);
  });

  it('buildLineBuffers omits an excluded shot and keeps the rest intact', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildLineBuffers(d, rel, lookup, stubColor, { shotGlobalIndices: new Set([0]) });
    expect(b.segmentCount).toBe(1);
    expect(b.vertexCount).toBe(4);
    expect(b.indices.length).toBe(6);
    expect(Array.from(b.indices)).toEqual([0, 1, 2, 2, 1, 3]);
    // The surviving segment is shot global 1 (station idx1 -> idx2), color stub(1, 1).
    expect(Array.from(b.colorFrom.slice(0, 4))).toEqual([1, 1, 7, 255]);
    expect(b.posA[0]).toBeCloseTo(rel[2]!, 6); // station global 1
    expect(b.posB[0]).toBeCloseTo(rel[4]!, 6); // station global 2
  });

  it('buildLineBuffers with an excluded section yields zero segments', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildLineBuffers(d, rel, lookup, stubColor, { sectionIds: new Set([1]) });
    expect(b.segmentCount).toBe(0);
    expect(b.vertexCount).toBe(0);
    expect(b.indices.length).toBe(0);
  });

  it('buildCircleBuffers omits an excluded station and compacts output', () => {
    const d = makeFixture();
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildCircleBuffers(d, rel, fill, stroke, { stationGlobalIndices: new Set([1]) });
    expect(b.stationCount).toBe(3); // globals 0, 2, 3 survive
    expect(b.vertexCount).toBe(12);
    expect(b.indices.length).toBe(18);
    // Output slot 0 = global 0, slot 1 = global 2, slot 2 = global 3 (compacted).
    expect(Array.from(b.fill.slice(0, 4))).toEqual([0, 0, 0, 255]);
    expect(Array.from(b.fill.slice(4 * 4, 4 * 4 + 4))).toEqual([2, 0, 0, 255]);
    expect(Array.from(b.fill.slice(8 * 4, 8 * 4 + 4))).toEqual([3, 0, 0, 255]);
    // Slot 1 (out index 1) center is global 2's anchor-relative position.
    expect(b.center[4 * 2]).toBeCloseTo(rel[4]!, 6);
    expect(b.center[4 * 2 + 1]).toBeCloseTo(rel[5]!, 6);
    // Compacted indices reference the compacted vertex slots.
    expect(Array.from(b.indices.slice(6, 12))).toEqual([4, 5, 6, 4, 6, 7]);
  });

  it('buildCircleBuffers with an excluded section drops all its stations', () => {
    const d = makeFixture();
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildCircleBuffers(d, rel, fill, stroke, { sectionIds: new Set([1]) });
    expect(b.stationCount).toBe(1); // only section 2's station 3
    expect(Array.from(b.fill.slice(0, 4))).toEqual([3, 0, 0, 255]);
  });

  it('a station/shot move excludes both endpoint stations and the shot together', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const exclude = { stationGlobalIndices: new Set([1, 2]), shotGlobalIndices: new Set([1]) };
    const line = buildLineBuffers(d, rel, lookup, stubColor, exclude);
    const circle = buildCircleBuffers(d, rel, fill, stroke, exclude);
    expect(line.segmentCount).toBe(1); // shot global 1 hidden; shot global 0 remains
    expect(circle.stationCount).toBe(2); // globals 0, 3 remain
  });

  it('no exclusion (undefined / {} ) is byte-identical to the plain build', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const baseLine = buildLineBuffers(d, rel, lookup, stubColor);
    const emptyLine = buildLineBuffers(d, rel, lookup, stubColor, {});
    expect(emptyLine.segmentCount).toBe(baseLine.segmentCount);
    expect(Array.from(emptyLine.indices)).toEqual(Array.from(baseLine.indices));
    expect(Array.from(emptyLine.colorFrom)).toEqual(Array.from(baseLine.colorFrom));
    expect(Array.from(emptyLine.colorTo)).toEqual(Array.from(baseLine.colorTo));
    expect(Array.from(emptyLine.posA)).toEqual(Array.from(baseLine.posA));

    const baseCircle = buildCircleBuffers(d, rel, fill, stroke);
    const emptyCircle = buildCircleBuffers(d, rel, fill, stroke, {});
    expect(emptyCircle.stationCount).toBe(baseCircle.stationCount);
    expect(Array.from(emptyCircle.indices)).toEqual(Array.from(baseCircle.indices));
    expect(Array.from(emptyCircle.fill)).toEqual(Array.from(baseCircle.fill));
    expect(Array.from(emptyCircle.center)).toEqual(Array.from(baseCircle.center));
  });
});

describe('reference-shot flags (setShowReferenceShots parity)', () => {
  const stubColor = (shotIdx: number, sectionId: number): Rgba8 => [shotIdx, sectionId, 7, 255];
  const fill = (g: number): Rgba8 => [g, 0, 0, 255];
  const stroke = (_g: number): Rgba8 => [9, 9, 9, 255];

  /** makeFixture with shot global 0 (section 1, shot idx0) flagged reference. */
  function refFixture(): SurveyData {
    const d = makeFixture();
    d.shotFlags = new Uint8Array([shotKindToFlags('Reference'), 0, 0]);
    return d;
  }

  it('buildLineBuffers writes the ShotKind discriminant on all 4 verts', () => {
    const d = refFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildLineBuffers(d, rel, lookup, stubColor);
    // #27: the attribute carries the kind (0/1/2), not a bool, so the shader
    // can fade and hide jumps and references separately. Segment 0 is a
    // Reference -> 2 on all 4 verts.
    expect(Array.from(b.reference.slice(0, 4))).toEqual([2, 2, 2, 2]);
    // Segment 1 is permanent -> 0.
    expect(Array.from(b.reference.slice(4, 8))).toEqual([0, 0, 0, 0]);
    // Every "is virtual" test still reads as > 0.5, which is what the hide
    // path and computeStationAllReference rely on.
    expect(b.reference[0]!).toBeGreaterThan(0.5);
  });

  it('buildLineBuffers writes 1 for a jump, distinct from a reference', () => {
    const d = refFixture();
    // Flip the virtual shot from Reference (2) to Jump (1).
    d.shotFlags = Uint8Array.from(d.shotFlags);
    d.shotFlags[0] = shotKindToFlags('Jump');
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildLineBuffers(d, rel, lookup, stubColor);
    expect(Array.from(b.reference.slice(0, 4))).toEqual([1, 1, 1, 1]);
  });

  it('computeStationAllReference records WHICH kinds touch each station', () => {
    const d = refFixture();
    const lookup = buildStationLookup(d);
    const allRef = computeStationAllReference(d, lookup);
    // The value is a mask, not a bool (#27): 0 = a Normal shot touches it,
    // 1 = only Jumps, 2 = only References, 3 = both / no legs at all. Splitting
    // the virtual kinds is what lets a reference-only station disappear when
    // References are hidden while Jumps are still drawn.
    // Station 0: only endpoint of the Reference shot -> 2.
    // Stations 1,2: endpoints of the Normal shot global 1 -> 0.
    // Station 3: section 2's only shot is invalid (missing endpoint) -> no legs -> 3.
    expect(Array.from(allRef)).toEqual([2, 0, 0, 3]);
  });

  it('a fully non-reference survey has no all-reference stations', () => {
    const d = makeFixture(); // all shotFlags 0
    const lookup = buildStationLookup(d);
    const allRef = computeStationAllReference(d, lookup);
    // Stations 0,1,2 are all endpoints of Normal shots -> 0; station 3 is
    // isolated, so it has no legs and keeps the both-off default of 3.
    expect(Array.from(allRef)).toEqual([0, 0, 0, 3]);
  });

  it('buildCircleBuffers propagates the all-reference flag to every vertex', () => {
    const d = refFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const allRef = computeStationAllReference(d, lookup);
    const b = buildCircleBuffers(d, rel, fill, stroke, undefined, allRef);
    // Station 0 (Reference-only) -> its 4 verts carry the Reference bit, 2.
    for (let c = 0; c < 4; c++) expect(b.allReference[0 * 4 + c]).toBe(2);
    // Station 1 (has a Normal shot) -> 0.
    for (let c = 0; c < 4; c++) expect(b.allReference[1 * 4 + c]).toBe(0);
    // Station 3 (isolated, no legs) -> 3.
    for (let c = 0; c < 4; c++) expect(b.allReference[3 * 4 + c]).toBe(3);
  });

  it('allReference defaults to 0 for every vertex when no flags are passed', () => {
    const d = makeFixture();
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildCircleBuffers(d, rel, fill, stroke);
    expect(Array.from(b.allReference)).toEqual(new Array(b.vertexCount).fill(0));
  });
});

describe('depth-gradient per-endpoint colors', () => {
  const stubColor = (shotIdx: number, sectionId: number): Rgba8 => [shotIdx, sectionId, 7, 255];
  // Distinct from/to colors per shot so the A/B split is observable.
  const endpoints = (shotIdx: number, _sectionId: number): readonly [Rgba8, Rgba8] => [
    [shotIdx, 0, 0, 255],
    [shotIdx, 255, 0, 255],
  ];

  it('replicates the from-color and to-color across all 4 verts (round-cap layout)', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildLineBuffers(d, rel, lookup, stubColor, undefined, endpoints);
    // Both endpoint colors ride on every vertex; the fragment picks A(from)/B(to)
    // by the clamped along-fraction, so a rounded cap holds its endpoint color.
    for (let v = 0; v < 4; v++) {
      expect(Array.from(b.colorFrom.slice(v * 4, v * 4 + 4))).toEqual([0, 0, 0, 255]);
      expect(Array.from(b.colorTo.slice(v * 4, v * 4 + 4))).toEqual([0, 255, 0, 255]);
    }
    // Endpoint attribute still selects A(0)/B(1) consistently.
    expect(Array.from(b.endpoint.slice(0, 4))).toEqual([0, 0, 1, 1]);
  });

  it('without the endpoint resolver, from == to == the flat color on every vert', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildLineBuffers(d, rel, lookup, stubColor);
    for (let v = 0; v < 4; v++) {
      expect(Array.from(b.colorFrom.slice(v * 4, v * 4 + 4))).toEqual([0, 1, 7, 255]);
      expect(Array.from(b.colorTo.slice(v * 4, v * 4 + 4))).toEqual([0, 1, 7, 255]);
    }
  });
});

describe('zero-datum sizing (stock circle-radius/-stroke parity)', () => {
  const fill = (g: number): Rgba8 => [g, 0, 0, 255];
  const stroke = (_g: number): Rgba8 => [9, 9, 9, 255];

  it('emits sizeMul 2 for a zero-datum station and 1 for the rest', () => {
    // makeFixture flags global station 0 as zero-datum.
    const d = makeFixture();
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildCircleBuffers(d, rel, fill, stroke);
    // Station 0 (zero-datum) -> all 4 verts scaled 2x (radius + stroke).
    for (let c = 0; c < 4; c++) expect(b.sizeMul[0 * 4 + c]).toBe(2);
    // Stations 1,2,3 (normal) -> 1x.
    for (let g = 1; g < 4; g++) {
      for (let c = 0; c < 4; c++) expect(b.sizeMul[g * 4 + c]).toBe(1);
    }
  });

  it('a survey with no zero-datum stations is all 1x', () => {
    const d = makeFixture();
    d.stationFlags = new Uint8Array([0, 0, 0, 0]);
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildCircleBuffers(d, rel, fill, stroke);
    expect(Array.from(b.sizeMul)).toEqual(new Array(b.vertexCount).fill(1));
  });

  it('sizeMul tracks the surviving station under an exclusion', () => {
    // Exclude station 0 (the zero-datum one): the compacted output has no 2x.
    const d = makeFixture();
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildCircleBuffers(d, rel, fill, stroke, { stationGlobalIndices: new Set([0]) });
    expect(b.stationCount).toBe(3);
    expect(Array.from(b.sizeMul)).toEqual(new Array(b.vertexCount).fill(1));
  });

  it('the circle shaders scale radius + stroke by the size multiplier', () => {
    // Quad extent, disc radius, and stroke radius all fold in `sizeMul` so a 2x
    // (zero-datum) station is not clipped and its stroke band doubles. Highlight
    // takes PRECEDENCE over the 2x: a highlighted station uses the configured
    // hover/select ratio, and only an unhighlighted one falls back to a_size_mul.
    // The fragment recomputes both radii from the `v_size_mul` varying.
    expect(CIRCLE_VERTEX_SRC).toContain('float sizeMul = (a_highlight > 1.5) ? u_hover_size_mul');
    expect(CIRCLE_VERTEX_SRC).toContain(': a_size_mul;');
    expect(CIRCLE_VERTEX_SRC).toContain('(u_radius + u_stroke_width) * sizeMul');
    expect(CIRCLE_FRAGMENT_SRC).toContain('u_radius * v_size_mul');
    expect(CIRCLE_FRAGMENT_SRC).toContain('u_stroke_width * v_size_mul');
  });
});

describe('anchored/immovable dim flag (stock opacity parity)', () => {
  const stubColor = (shotIdx: number, sectionId: number): Rgba8 => [shotIdx, sectionId, 7, 255];
  const fill = (g: number): Rgba8 => [g, 0, 0, 255];
  const stroke = (_g: number): Rgba8 => [9, 9, 9, 255];

  it('buildLineBuffers marks all 4 verts of a dimmed shot, 0 otherwise', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    // Dim shot global 0 only.
    const b = buildLineBuffers(d, rel, lookup, stubColor, undefined, undefined, (sgi) => sgi === 0);
    expect(Array.from(b.dim.slice(0, 4))).toEqual([1, 1, 1, 1]);
    expect(Array.from(b.dim.slice(4, 8))).toEqual([0, 0, 0, 0]);
  });

  it('buildCircleBuffers marks all 4 verts of a dimmed station, 0 otherwise', () => {
    const d = makeFixture();
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildCircleBuffers(d, rel, fill, stroke, undefined, undefined, (g) => g === 2);
    for (let c = 0; c < 4; c++) expect(b.dim[2 * 4 + c]).toBe(1);
    for (const g of [0, 1, 3]) for (let c = 0; c < 4; c++) expect(b.dim[g * 4 + c]).toBe(0);
  });

  it('dim defaults to 0 for every vertex when no predicate is passed', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const line = buildLineBuffers(d, rel, lookup, stubColor);
    const circle = buildCircleBuffers(d, rel, fill, stroke);
    expect(Array.from(line.dim)).toEqual(new Array(line.vertexCount).fill(0));
    expect(Array.from(circle.dim)).toEqual(new Array(circle.vertexCount).fill(0));
  });

  it('computeLineDim / computeCircleDim reproduce the build path dim arrays (parity)', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const dimForShot = (sgi: number): boolean => sgi === 1;
    const dimForStation = (g: number): boolean => g === 0 || g === 3;
    // No exclusion.
    const line = buildLineBuffers(d, rel, lookup, stubColor, undefined, undefined, dimForShot);
    const circle = buildCircleBuffers(d, rel, fill, stroke, undefined, undefined, dimForStation);
    expect(Array.from(computeLineDim(d, lookup, dimForShot))).toEqual(Array.from(line.dim));
    expect(Array.from(computeCircleDim(d, dimForStation))).toEqual(Array.from(circle.dim));
    // Under an exclusion the cheap recompute stays in lock-step vertex-for-vertex.
    const exclude = { stationGlobalIndices: new Set([1]), shotGlobalIndices: new Set([0]) };
    const lineEx = buildLineBuffers(d, rel, lookup, stubColor, exclude, undefined, dimForShot);
    const circleEx = buildCircleBuffers(d, rel, fill, stroke, exclude, undefined, dimForStation);
    expect(Array.from(computeLineDim(d, lookup, dimForShot, exclude))).toEqual(Array.from(lineEx.dim));
    expect(Array.from(computeCircleDim(d, dimForStation, exclude))).toEqual(Array.from(circleEx.dim));
  });

  it('the shaders dim by 0.25 and let dim WIN over the virtual cases', () => {
    // Line vertex opacity multiplier: the full dim 0.25 case must PRECEDE the
    // gas-dim (0.45) and virtual cases in the ternary so a shot that is both
    // dimmed and virtual resolves to 0.25 (stock case order). The fragment
    // applies v_alpha_mul to the color. #27 splits the single 0.5 virtual case: a
    // jump keeps 0.5 so existing surveys look unchanged, a reference is fainter
    // because it is not swum. #395 adds a third a_dim level (0.5 = beyond gas
    // range) mapping to 0.45, between full-dim (1.0) and none (0). The gas-dim
    // level must never BRIGHTEN a fainter per-kind base (reference at 0.28), so
    // it clamps against the per-kind alpha via min() rather than replacing it
    // outright (Codex review on #397).
    expect(LINE_VERTEX_SRC).toContain(
      'v_alpha_mul = (a_dim > 0.75) ? 0.25 : ((a_dim > 0.25) ? min(0.45, (isRef ? 0.28 : (isJump ? 0.5 : 1.0))) : (isRef ? 0.28 : (isJump ? 0.5 : 1.0)));',
    );
    expect(LINE_FRAGMENT_SRC).toContain('col.a * v_alpha_mul * alpha');
    // Circle fragment folds the dim factor into the final alpha (same 3-level map).
    // Stations have no per-kind base alpha (always 1.0 outside dim), so a flat
    // replace here cannot brighten anything - unlike the line vertex above.
    expect(CIRCLE_FRAGMENT_SRC).toContain('(v_dim > 0.75) ? 0.25 : ((v_dim > 0.25) ? 0.45 : 1.0)');
  });

  it('a reference leg beyond gas range keeps its fainter base alpha, never brightens', () => {
    // Reference base is 0.28; gas-dim is 0.45. min(0.45, 0.28) = 0.28, so the
    // gas-dim branch for a reference leg must literally contain the reference
    // constant, not the bare 0.45 gas-dim constant (proves the clamp, not just
    // its presence).
    expect(LINE_VERTEX_SRC).toContain('min(0.45, (isRef ? 0.28 : (isJump ? 0.5 : 1.0)))');
    // A normal shot's per-kind base (1.0) is brighter than gas-dim, so it still
    // dims down to 0.45 as before.
    expect(LINE_VERTEX_SRC).not.toContain('a_dim > 0.25) ? 0.45 :');
  });
});

describe('round caps (shot-shot junction shaders)', () => {
  it('the line vertex extends the quad along the segment past both endpoints', () => {
    // The along-axis extension (dir * capSign * outset) is what gives the fragment
    // capsule room to round the caps; vertex count is unchanged (still 4/segment).
    expect(LINE_VERTEX_SRC).toContain('float capSign = (a_endpoint < 0.5) ? -1.0 : 1.0;');
    expect(LINE_VERTEX_SRC).toContain('normal * a_side * outset + dir * capSign * outset');
    // The fragment gets signed perpendicular, along-axis position, and length.
    expect(LINE_VERTEX_SRC).toContain('v_perp = a_side * outset;');
    expect(LINE_VERTEX_SRC).toContain('v_len = segLen;');
  });

  it('the line fragment uses a capsule (segment) SDF so ends round with the AA feather', () => {
    // Distance to the SEGMENT [0, v_len], not the infinite centerline: in the body
    // it collapses to abs(perp) (butt-cap parity); past an endpoint it rounds.
    expect(LINE_FRAGMENT_SRC).toContain('float alongClamp = clamp(v_along, 0.0, v_len);');
    expect(LINE_FRAGMENT_SRC).toContain('length(vec2(v_along - alongClamp, v_perp))');
    expect(LINE_FRAGMENT_SRC).toContain('clamp(min(dist + u_aa, outset - dist) / u_aa, 0.0, 1.0)');
    // Endpoint color clamp: from->to mixed by the CLAMPED along-fraction.
    expect(LINE_FRAGMENT_SRC).toContain('clamp(v_along / v_len, 0.0, 1.0)');
    expect(LINE_FRAGMENT_SRC).toContain('mix(v_color_from, v_color_to, t)');
  });
});

describe('segment highlight flag (Move Segment whole-segment hover)', () => {
  const stubColor = (shotIdx: number, sectionId: number): Rgba8 => [shotIdx, sectionId, 7, 255];
  const fill = (g: number): Rgba8 => [g, 0, 0, 255];
  const stroke = (_g: number): Rgba8 => [9, 9, 9, 255];

  it('buildLineBuffers marks all 4 verts of a highlighted shot, 0 otherwise', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    // highlightForShot is the 9th positional arg (after dim + hide).
    const b = buildLineBuffers(d, rel, lookup, stubColor, undefined, undefined, undefined, undefined, (sgi) => sgi === 0);
    expect(Array.from(b.highlight.slice(0, 4))).toEqual([1, 1, 1, 1]);
    expect(Array.from(b.highlight.slice(4, 8))).toEqual([0, 0, 0, 0]);
  });

  it('buildCircleBuffers marks every station of a highlighted section, 0 otherwise', () => {
    const d = makeFixture();
    const rel = projectStationsRel(d, null, 0, 0);
    // Whole connected-set semantics: highlight keys off section membership.
    const hlStation = (g: number): boolean => d.stationSectionIds[g]! === 1;
    const b = buildCircleBuffers(d, rel, fill, stroke, undefined, undefined, undefined, undefined, hlStation);
    for (const g of [0, 1, 2]) for (let c = 0; c < 4; c++) expect(b.highlight[g * 4 + c]).toBe(1);
    for (let c = 0; c < 4; c++) expect(b.highlight[3 * 4 + c]).toBe(0);
  });

  it('highlight defaults to 0 for every vertex when no predicate is passed', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const line = buildLineBuffers(d, rel, lookup, stubColor);
    const circle = buildCircleBuffers(d, rel, fill, stroke);
    expect(Array.from(line.highlight)).toEqual(new Array(line.vertexCount).fill(0));
    expect(Array.from(circle.highlight)).toEqual(new Array(circle.vertexCount).fill(0));
  });

  it('computeLineDim / computeCircleDim reproduce the build path highlight arrays (parity)', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const hlShot = (_sgi: number, sid: number): boolean => sid === 1;
    const hlStation = (g: number): boolean => d.stationSectionIds[g]! === 1;
    const line = buildLineBuffers(d, rel, lookup, stubColor, undefined, undefined, undefined, undefined, hlShot);
    const circle = buildCircleBuffers(d, rel, fill, stroke, undefined, undefined, undefined, undefined, hlStation);
    // computeLine/CircleDim double as the per-vertex flag builders for any predicate.
    expect(Array.from(computeLineDim(d, lookup, hlShot))).toEqual(Array.from(line.highlight));
    expect(Array.from(computeCircleDim(d, hlStation))).toEqual(Array.from(circle.highlight));
  });

  it('the shaders widen highlighted features by graded level via configured uniforms (C2)', () => {
    // Line: level 2 (hover) / level 1 (selection) pick the configured width-ratio
    // uniforms (default 2x / 1.5x), else 1x. The fragment recomputes its outset
    // from the same varying so the AA stays correct.
    expect(LINE_VERTEX_SRC).toContain('in float a_highlight;');
    expect(LINE_VERTEX_SRC).toContain('u_half_width * widthMul');
    expect(LINE_VERTEX_SRC).toContain('uniform float u_hover_width_mul;');
    expect(LINE_VERTEX_SRC).toContain('uniform float u_select_width_mul;');
    expect(LINE_VERTEX_SRC).toContain(
      '(a_highlight > 1.5) ? u_hover_width_mul : ((a_highlight > 0.5) ? u_select_width_mul : 1.0)',
    );
    // Circle: highlight takes PRECEDENCE over zero-datum sizing - level 2 / level 1
    // pick the configured radius-ratio uniforms (default 1.5x / 1.25x), and only an
    // unhighlighted station uses a_size_mul (zero-datum 2x). Radius + stroke grow
    // together via the size multiplier.
    expect(CIRCLE_VERTEX_SRC).toContain('in float a_highlight;');
    expect(CIRCLE_VERTEX_SRC).toContain('uniform float u_hover_size_mul;');
    expect(CIRCLE_VERTEX_SRC).toContain('uniform float u_select_size_mul;');
    expect(CIRCLE_VERTEX_SRC).toContain('(a_highlight > 1.5) ? u_hover_size_mul');
    expect(CIRCLE_VERTEX_SRC).toContain('(a_highlight > 0.5) ? u_select_size_mul');
    expect(CIRCLE_VERTEX_SRC).toContain(': a_size_mul;');
  });

  it('build/compute carry graded emphasis levels (selection 1, hover 2)', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    // Section 1 at hover level (2), station shots as a numeric-returning predicate.
    const hlShot = (_sgi: number, sid: number): number => (sid === 1 ? 2 : 0);
    const hlStation = (g: number): number => (d.stationSectionIds[g]! === 1 ? 1 : 0);
    const line = buildLineBuffers(d, rel, lookup, stubColor, undefined, undefined, undefined, undefined, hlShot);
    const circle = buildCircleBuffers(d, rel, fill, stroke, undefined, undefined, undefined, undefined, hlStation);
    // Levels flow through verbatim (not clamped to 0/1).
    expect(Array.from(line.highlight.slice(0, 4))).toEqual([2, 2, 2, 2]);
    for (const g of [0, 1, 2]) for (let c = 0; c < 4; c++) expect(circle.highlight[g * 4 + c]).toBe(1);
    // computeLine/CircleDim reproduce the same levels for the cheap refresh path.
    expect(Array.from(computeLineDim(d, lookup, hlShot))).toEqual(Array.from(line.highlight));
    expect(Array.from(computeCircleDim(d, hlStation))).toEqual(Array.from(circle.highlight));
  });
});

describe('SurveyCustomLayer first-frames GL-error probe', () => {
  /** Private-method accessor: the GL draw path stays untested, but the pure
   *  frame-accounting around gl.getError() is exercised with a fake context. */
  type Probe = { checkFrameError(gl: WebGL2RenderingContext, data: CustomRenderData): void };
  const probe = (l: SurveyCustomLayer): Probe => l as unknown as Probe;

  const fakeData = (): CustomRenderData =>
    ({
      line: { segmentCount: 2, byteLength: 100 },
      circle: { stationCount: 4, byteLength: 40 },
    }) as unknown as CustomRenderData;

  /** Minimal GL stub: only NO_ERROR + a scripted getError() are touched here. */
  const fakeGl = (getError: () => number): WebGL2RenderingContext =>
    ({ NO_ERROR: 0, getError }) as unknown as WebGL2RenderingContext;

  afterEach(() => vi.restoreAllMocks());

  it('checks getError() only until the first N clean frames, then stops', () => {
    const info = vi.spyOn(console, 'info').mockImplementation(() => {});
    const getError = vi.fn(() => 0);
    const layer = new SurveyCustomLayer('id', null);
    const gl = fakeGl(getError);
    const data = fakeData();
    for (let i = 0; i < 8; i++) probe(layer).checkFrameError(gl, data);
    // 5 clean frames validated, then it stops calling getError().
    expect(getError).toHaveBeenCalledTimes(5);
    expect(layer.isDisabled()).toBe(false);
    // The first clean frame logs 'first frame drawn OK' exactly once (ungated).
    expect(info).toHaveBeenCalledTimes(1);
    expect(info.mock.calls[0]?.[0]).toContain('[customRender] first frame drawn OK');
  });

  it('self-disables and notifies the callback on a GL error', () => {
    vi.spyOn(console, 'info').mockImplementation(() => {});
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const onSelfDisable = vi.fn();
    const layer = new SurveyCustomLayer('id', null, onSelfDisable);
    // First frame clean, second frame raises INVALID_OPERATION (0x502).
    let call = 0;
    const gl = fakeGl(() => (call++ === 0 ? 0 : 0x502));
    const data = fakeData();
    probe(layer).checkFrameError(gl, data);
    probe(layer).checkFrameError(gl, data);
    expect(layer.isDisabled()).toBe(true);
    expect(layer.getDisabledReason()).toBe('gl-error-0x502');
    expect(onSelfDisable).toHaveBeenCalledOnce();
    expect(onSelfDisable).toHaveBeenCalledWith('gl-error-0x502');
    const lastWarn = warn.mock.calls[warn.mock.calls.length - 1];
    expect(lastWarn?.[0]).toContain('[customRender] GL error 0x502');
  });
});

describe('per-section line range map', () => {
  const stubColor = (shotIdx: number, sectionId: number): Rgba8 => [shotIdx, sectionId, 7, 255];

  it('records [segStart, segCount] per section in declaration order', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildLineBuffers(d, rel, lookup, stubColor);
    // Section 1 owns the 2 valid shots (segments 0,1); section 2's only shot is
    // invalid, so it contributes 0 segments starting after section 1.
    expect(Array.from(b.sectionRanges)).toEqual([0, 2, 2, 0]);
  });

  it('tracks ranges under a single-shot exclusion (compacted)', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildLineBuffers(d, rel, lookup, stubColor, { shotGlobalIndices: new Set([0]) });
    // Shot 0 excluded -> section 1 keeps 1 segment (shot 1); section 2 still 0.
    expect(Array.from(b.sectionRanges)).toEqual([0, 1, 1, 0]);
  });

  it('a fully excluded section has a zero-count range', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildLineBuffers(d, rel, lookup, stubColor, { sectionIds: new Set([1]) });
    expect(Array.from(b.sectionRanges)).toEqual([0, 0, 0, 0]);
  });
});

describe('drag hide flag (per-vertex, no rebuild)', () => {
  const stubColor = (shotIdx: number, sectionId: number): Rgba8 => [shotIdx, sectionId, 7, 255];
  const fill = (g: number): Rgba8 => [g, 0, 0, 255];
  const stroke = (_g: number): Rgba8 => [9, 9, 9, 255];

  it('buildLineBuffers marks all 4 verts of a hidden shot, 0 otherwise', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildLineBuffers(d, rel, lookup, stubColor, undefined, undefined, undefined, (sgi) => sgi === 0);
    expect(Array.from(b.hide.slice(0, 4))).toEqual([1, 1, 1, 1]);
    expect(Array.from(b.hide.slice(4, 8))).toEqual([0, 0, 0, 0]);
  });

  it('buildCircleBuffers marks all 4 verts of a hidden station, 0 otherwise', () => {
    const d = makeFixture();
    const rel = projectStationsRel(d, null, 0, 0);
    const b = buildCircleBuffers(d, rel, fill, stroke, undefined, undefined, undefined, (g) => g === 2);
    for (let c = 0; c < 4; c++) expect(b.hide[2 * 4 + c]).toBe(1);
    for (const g of [0, 1, 3]) for (let c = 0; c < 4; c++) expect(b.hide[g * 4 + c]).toBe(0);
  });

  it('hide defaults to 0 for every vertex when no predicate is passed', () => {
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const line = buildLineBuffers(d, rel, lookup, stubColor);
    const circle = buildCircleBuffers(d, rel, fill, stroke);
    expect(Array.from(line.hide)).toEqual(new Array(line.vertexCount).fill(0));
    expect(Array.from(circle.hide)).toEqual(new Array(circle.vertexCount).fill(0));
  });

  it('computeLineDim / computeCircleDim double as the hide-flag builders', () => {
    // The controller reuses these to recompute ONLY the hide arrays on drag
    // begin/end; the flag they produce must match the build path's inline hide.
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const hideForShot = (sgi: number): boolean => sgi === 1;
    const hideForStation = (g: number): boolean => g === 0 || g === 3;
    const line = buildLineBuffers(d, rel, lookup, stubColor, undefined, undefined, undefined, hideForShot);
    const circle = buildCircleBuffers(d, rel, fill, stroke, undefined, undefined, undefined, hideForStation);
    expect(Array.from(computeLineDim(d, lookup, hideForShot))).toEqual(Array.from(line.hide));
    expect(Array.from(computeCircleDim(d, hideForStation))).toEqual(Array.from(circle.hide));
  });

  it('the shaders collapse a hidden vertex to a clipped point', () => {
    expect(LINE_VERTEX_SRC).toContain('if (a_hide > 0.5) {');
    expect(LINE_VERTEX_SRC).toContain('in float a_hide;');
    expect(CIRCLE_VERTEX_SRC).toContain('if (a_hide > 0.5) {');
    expect(CIRCLE_VERTEX_SRC).toContain('in float a_hide;');
  });

  it('a full build + hide flag draws the same geometry as an exclusion rebuild', () => {
    // Drag-begin marks moving features hidden WITHOUT omitting them; the drawn
    // (non-hidden) verts must match what the old exclusion-rebuild produced.
    const d = makeFixture();
    const lookup = buildStationLookup(d);
    const rel = projectStationsRel(d, null, 0, 0);
    const hidden = new Set([0]); // hide shot global 0
    const full = buildLineBuffers(d, rel, lookup, stubColor, undefined, undefined, undefined, (sgi) =>
      hidden.has(sgi),
    );
    const excluded = buildLineBuffers(d, rel, lookup, stubColor, { shotGlobalIndices: hidden });
    // The hidden segment (0) collapses; the surviving segment (1, verts 4..7) in
    // the full+hide build equals the excluded build's only segment (verts 0..3).
    expect(Array.from(full.hide.slice(0, 4))).toEqual([1, 1, 1, 1]);
    expect(Array.from(full.hide.slice(4, 8))).toEqual([0, 0, 0, 0]);
    expect(excluded.segmentCount).toBe(1);
    expect(Array.from(full.posA.slice(8, 16))).toEqual(Array.from(excluded.posA.slice(0, 8)));
    expect(Array.from(full.colorFrom.slice(16, 32))).toEqual(Array.from(excluded.colorFrom.slice(0, 16)));
  });
});

describe('color-only fast path (recomputeLine/CircleColors byte-identity)', () => {
  // A deterministic PRNG so the property sweep is reproducible.
  function mulberry32(seed: number): () => number {
    let a = seed >>> 0;
    return () => {
      a |= 0;
      a = (a + 0x6d2b79f5) | 0;
      let t = Math.imul(a ^ (a >>> 15), 1 | a);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }

  /** Random but valid survey: each section owns N stations (indices 0..N-1) and
   *  up to N-1 shots, with the occasional dropped endpoint so invalid shots are
   *  exercised (the color recompute must skip them in lock-step with the build). */
  function randomSurvey(rng: () => number): SurveyData {
    const d = emptySurveyData();
    const sectionCount = 1 + Math.floor(rng() * 4);
    const sections = [];
    const stationSectionIds: number[] = [];
    const stationIndices: number[] = [];
    const stationCoords: number[] = [];
    const stationFlags: number[] = [];
    const shotIds: number[] = [];
    const shotSectionIds: number[] = [];
    const shotFlags: number[] = [];
    let shotOffset = 0;
    for (let s = 0; s < sectionCount; s++) {
      const id = s + 1;
      const nStations = 2 + Math.floor(rng() * 5);
      // Drop a random interior station sometimes to force an invalid shot.
      const dropIdx = rng() < 0.3 ? 1 + Math.floor(rng() * (nStations - 1)) : -1;
      for (let i = 0; i < nStations; i++) {
        if (i === dropIdx) continue;
        stationSectionIds.push(id);
        stationIndices.push(i);
        stationCoords.push(rng() * 100, rng() * 100, -rng() * 10);
        stationFlags.push(rng() < 0.2 ? STATION_FLAG_IS_ZERO_DATUM : 0);
      }
      const nShots = nStations - 1;
      for (let i = 0; i < nShots; i++) {
        shotIds.push(i);
        shotSectionIds.push(id);
        shotFlags.push(rng() < 0.25 ? shotKindToFlags('Reference') : 0);
      }
      sections.push({
        id, name: `S${id}`, surveyor: null, date: null, color: '#abcdef',
        direction: 'in' as never, quality: 0, next_shot_id: nShots, shotOffset, shotCount: nShots,
      });
      shotOffset += nShots;
    }
    d.sections = sections;
    d.stationCount = stationSectionIds.length;
    d.stationSectionIds = new Uint32Array(stationSectionIds);
    d.stationIndices = new Uint32Array(stationIndices);
    d.stationCoords = new Float64Array(stationCoords);
    d.stationFlags = new Uint8Array(stationFlags);
    d.shotCount = shotIds.length;
    d.shotIds = new Uint32Array(shotIds);
    d.shotSectionIds = new Uint32Array(shotSectionIds);
    d.shotFlags = new Uint8Array(shotFlags);
    return d;
  }

  it('recomputeLineColors rewrites colorFrom/colorTo to a full-rebuild byte-for-byte', () => {
    for (let seed = 1; seed <= 40; seed++) {
      const rng = mulberry32(seed);
      const d = randomSurvey(rng);
      const lookup = buildStationLookup(d);
      const rel = projectStationsRel(d, null, 0, 0);
      // Two arbitrary resolver "modes" (flat A -> depth-endpoint B), plus stubs
      // whose output changes with the mode so a stale byte would surface.
      const colorA = (sgi: number, sid: number): Rgba8 => [sgi & 255, sid & 255, 1, 255];
      const colorB = (sgi: number, sid: number): Rgba8 => [(sgi * 3) & 255, (sid * 7) & 255, 2, 200];
      const endsB = (sgi: number, sid: number): readonly [Rgba8, Rgba8] => [
        [(sgi * 5) & 255, 0, 0, 255],
        [(sid * 11) & 255, 255, 0, 128],
      ];
      // Build at A (flat), then color-only recompute to B (depth endpoints).
      const buf = buildLineBuffers(d, rel, lookup, colorA);
      const posBefore = Float32Array.from(buf.posA);
      const refBefore = Float32Array.from(buf.reference);
      const idxBefore = Uint32Array.from(buf.indices);
      recomputeLineColors(buf, d, lookup, colorB, endsB);
      const full = buildLineBuffers(d, rel, lookup, colorB, undefined, endsB);
      expect(Array.from(buf.colorFrom)).toEqual(Array.from(full.colorFrom));
      expect(Array.from(buf.colorTo)).toEqual(Array.from(full.colorTo));
      // Non-color buffers must be untouched by the color-only rewrite.
      expect(Array.from(buf.posA)).toEqual(Array.from(posBefore));
      expect(Array.from(buf.reference)).toEqual(Array.from(refBefore));
      expect(Array.from(buf.indices)).toEqual(Array.from(idxBefore));
    }
  });

  it('recomputeCircleColors rewrites fill/stroke to a full-rebuild byte-for-byte', () => {
    for (let seed = 101; seed <= 140; seed++) {
      const rng = mulberry32(seed);
      const d = randomSurvey(rng);
      const rel = projectStationsRel(d, null, 0, 0);
      const fillA = (g: number): Rgba8 => [g & 255, 0, 0, 255];
      const strokeA = (_g: number): Rgba8 => [1, 1, 1, 255];
      const fillB = (g: number): Rgba8 => [(g * 9) & 255, 3, 4, 222];
      const strokeB = (g: number): Rgba8 => [(g * 13) & 255, 2, 2, 255];
      const allRef = computeStationAllReference(d, buildStationLookup(d));
      const buf = buildCircleBuffers(d, rel, fillA, strokeA, undefined, allRef);
      const centerBefore = Float32Array.from(buf.center);
      const sizeBefore = Float32Array.from(buf.sizeMul);
      const allRefBefore = Float32Array.from(buf.allReference);
      recomputeCircleColors(buf, d, fillB, strokeB);
      const full = buildCircleBuffers(d, rel, fillB, strokeB, undefined, allRef);
      expect(Array.from(buf.fill)).toEqual(Array.from(full.fill));
      expect(Array.from(buf.stroke)).toEqual(Array.from(full.stroke));
      // Geometry / size / all-reference untouched.
      expect(Array.from(buf.center)).toEqual(Array.from(centerBefore));
      expect(Array.from(buf.sizeMul)).toEqual(Array.from(sizeBefore));
      expect(Array.from(buf.allReference)).toEqual(Array.from(allRefBefore));
    }
  });
});

describe('planColorOnlyUpdate gate', () => {
  const theme = { stationDefault: '#fff' } as never;
  const baseCtx = (over: Partial<Record<string, unknown>> = {}): DisplayColorContext =>
    ({
      themeColors: theme,
      showDepthGradient: false,
      showIssueSeverity: false,
      depthRange: { min: 0, max: 1 },
      issueSeverity: undefined,
      overlay: null,
      commentTint: false,
      ...over,
    }) as unknown as DisplayColorContext;

  it('fires when a color global changed but geometry is byte-stable', () => {
    const d = makeFixture();
    const [ax, ay] = computeMercatorAnchor(d, null);
    // Depth-gradient toggle: color-only, geometry identical.
    expect(planColorOnlyUpdate(d, d, baseCtx(), baseCtx({ showDepthGradient: true }), null, null, ax, ay)).toBe(true);
  });

  it('does not fire when the color globals are unchanged (a no-op / geometry edit)', () => {
    const d = makeFixture();
    const [ax, ay] = computeMercatorAnchor(d, null);
    expect(planColorOnlyUpdate(d, d, baseCtx(), baseCtx(), null, null, ax, ay)).toBe(false);
  });

  it('falls back when geometry ALSO changed (combined edit)', () => {
    const prev = makeFixture();
    const next = makeFixture();
    next.stationCoords = Float64Array.from(prev.stationCoords);
    next.stationCoords[0] = 5; // move station 0's x
    const [ax, ay] = computeMercatorAnchor(prev, null);
    // A color global changed AND a station moved -> not color-only.
    expect(planColorOnlyUpdate(prev, next, baseCtx(), baseCtx({ commentTint: true }), null, null, ax, ay)).toBe(false);
  });

  it('falls back when a shot reference flag changed (feeds reference/all-reference)', () => {
    const prev = makeFixture();
    const next = makeFixture();
    next.shotFlags = Uint8Array.from([shotKindToFlags('Reference'), 0, 0]);
    const [ax, ay] = computeMercatorAnchor(prev, null);
    expect(planColorOnlyUpdate(prev, next, baseCtx(), baseCtx({ showIssueSeverity: true }), null, null, ax, ay)).toBe(false);
  });

  it('falls back on an anchor shift even when only color changed', () => {
    const d = makeFixture();
    const [ax, ay] = computeMercatorAnchor(d, null);
    // A wrong cached anchor (as if the bbox extent moved) forces a full rebuild.
    expect(planColorOnlyUpdate(d, d, baseCtx(), baseCtx({ showDepthGradient: true }), null, null, ax + 1, ay)).toBe(false);
  });
});

// colorCtxNoop referenced to keep the fixture's intent explicit for future
// color-mode tests; the resolvers are exercised via the controller elsewhere.
void colorCtxNoop;
