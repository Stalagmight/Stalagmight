import { describe, expect, it } from 'vitest';
import { buildDataRows } from '../rows';
import { buildDistanceIndex, depthExtent, rowTint, tintLegend, TYPE_TINT, type TintInputs } from '../tints';
import { COLORS, makeCtx, ref } from '../../__tests__/dataFixtures';
import type { DataRow } from '../../types';

const ctx = makeCtx();
const rows = buildDataRows(ctx.data, { showSectionHeaders: true });
const row = (key: string): DataRow => rows.find((r) => r.key === key)!;
const base = (mode: TintInputs['mode'], extra: Partial<TintInputs> = {}): TintInputs => ({ mode, ctx, ...extra });

describe('rowTint', () => {
  it('none tints nothing, section tints by section colour', () => {
    expect(rowTint(row('st:1:1'), base('none'))).toBeNull();
    expect(rowTint(row('sec:1'), base('none'))).toBeNull();
    expect(rowTint(row('sh:2:201'), base('section'))).toBe(COLORS[2]);
  });

  it('gives header rows their section colour in every active mode', () => {
    for (const mode of ['type', 'issues', 'depth', 'distance'] as const) {
      expect(rowTint(row('sec:3'), base(mode))).toBe(COLORS[3]);
    }
  });

  it('type separates stations from shots with the entity tokens', () => {
    expect(rowTint(row('st:1:1'), base('type'))).toBe(TYPE_TINT.station);
    expect(rowTint(row('sh:1:101'), base('type'))).toBe(TYPE_TINT.shot);
  });

  it('issues colours by score and leaves clean rows untinted', () => {
    const shotScore = new Float32Array(ctx.data.shotCount);
    const stationScore = new Float32Array(ctx.data.stationCount);
    const r = row('sh:3:303');
    if (r.kind !== 'shot') throw new Error('shot');
    shotScore[r.g] = 3;
    const inputs = base('issues', { issueScores: { shotScore, stationScore }, issueColor: (s) => `score-${s}` });
    expect(rowTint(r, inputs)).toBe('score-3');
    expect(rowTint(row('sh:1:101'), inputs)).toBeNull();
  });

  it('depth uses the shot mean depth and the station z', () => {
    const inputs = base('depth', { depthColor: (d) => `d${d}` });
    expect(rowTint(row('sh:1:101'), inputs)).toBe('d1.5');
    expect(rowTint(row('st:1:3'), inputs)).toBe('d2');
  });

  it('distance averages a shot over its endpoints and skips unreachable rows', () => {
    const index = buildDistanceIndex({
      station_refs: [ref(1, 0), ref(1, 1), ref(1, 2)],
      distances: [0, 10, null],
      max_reachable: 10,
    });
    const inputs = base('distance', { distance: { index, color: (d) => `m${d}` } });
    expect(rowTint(row('st:1:1'), inputs)).toBe('m10');
    expect(rowTint(row('sh:1:101'), inputs)).toBe('m5');
    expect(rowTint(row('sh:1:102'), inputs)).toBeNull();
    expect(rowTint(row('st:2:1'), inputs)).toBeNull();
  });
});

describe('depthExtent / tintLegend', () => {
  it('spans station z and every shot depth', () => {
    expect(depthExtent(ctx)).toEqual({ min: 0, max: 4.5 });
  });

  const fmt = {
    t: (k: string) => k,
    metres: (v: number) => `${v} m`,
    depthRange: { min: 0, max: 4 },
    distanceMax: 20,
  };

  it('describes ramps with min / max labels and stops', () => {
    const depth = tintLegend('depth', base('depth', { depthColor: () => '#000000' }), fmt);
    expect(depth).toMatchObject({ kind: 'gradient', min: '0 m', max: '4 m' });
    expect(depth!.stops).toHaveLength(9);
    const issues = tintLegend('issues', base('issues', { issueColor: () => '#ff0000' }), fmt);
    expect(issues).toMatchObject({ kind: 'gradient', min: 'dataWorkspace.tint.legend.clear' });
  });

  it('gives swatches for type and nothing for none / section', () => {
    expect(tintLegend('type', base('type'), fmt)!.swatches).toHaveLength(2);
    expect(tintLegend('none', base('none'), fmt)).toBeNull();
    expect(tintLegend('section', base('section'), fmt)).toBeNull();
  });
});
