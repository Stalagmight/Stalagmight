import { describe, expect, it } from 'vitest';
import { buildDataRows } from '../rows';
import { buildRail, linkedRowKeys, stationLinks } from '../links';
import { COLORS, makeCtx, ref } from '../../__tests__/dataFixtures';
import type { DataRow } from '../../types';

const ctx = makeCtx();
const g = (sectionId: number, index: number) => {
  for (let i = 0; i < ctx.data.stationCount; i++) {
    if (ctx.data.stationSectionIds[i] === sectionId && ctx.data.stationIndices[i] === index) return i;
  }
  throw new Error('station');
};

describe('stationLinks', () => {
  it('reports merges, the junction and the zero datum', () => {
    const j = stationLinks(ctx, 1, 2, g(1, 2));
    expect(j.merges).toEqual([ref(2, 0)]);
    expect(j.junction).toBe(true);
    expect(j.branchFrom).toBeNull();
    expect(stationLinks(ctx, 1, 0, g(1, 0)).zeroDatum).toBe(true);
  });

  it('finds the parent of a branch section start', () => {
    const start = stationLinks(ctx, 2, 0, g(2, 0));
    expect(start.branchFrom).toEqual(ref(1, 2));
    expect(start.junction).toBe(true);
  });

  it('carries the closure partner and its misclosure', () => {
    const c = stationLinks(ctx, 1, 4, g(1, 4));
    expect(c.closures).toEqual([
      { closureId: '7', partner: ref(3, 3), via: null, transitive: false, misclosure: 0.7, errorPct: 1.25 },
    ]);
    expect(linkedRowKeys(c)).toEqual(['st:3:3']);
    expect(linkedRowKeys(stationLinks(ctx, 1, 2, g(1, 2)))).toEqual(['st:2:0']);
  });

  it('is empty for a plain station', () => {
    const plain = stationLinks(ctx, 1, 3, g(1, 3));
    expect(plain).toMatchObject({ merges: [], closures: [], zeroDatum: false, junction: false });
  });
});

describe('branch parentage', () => {
  it('two sections starting at one point: only the later continues from the earlier', () => {
    const data = makeCtx().data;
    const g10 = g(1, 0);
    const g20 = g(2, 0);
    const merged = new Map(data.stationMergedWith);
    merged.set(g10, [ref(2, 0)]);
    merged.set(g20, [ref(1, 2), ref(1, 0)]);
    const c = makeCtx({ data: { ...data, stationMergedWith: merged } });
    expect(stationLinks(c, 1, 0, g10).branchFrom).toBeNull();
    expect(stationLinks(c, 2, 0, g20).branchFrom).toEqual(ref(1, 0));
  });
});

describe('buildRail', () => {
  const all = buildDataRows(ctx.data, { showSectionHeaders: true });
  const byKey = (rail: ReturnType<typeof buildRail>, rows: DataRow[], key: string) => rail![rows.findIndex((r) => r.key === key)]!;

  it('is absent when the view is sorted', () => {
    expect(buildRail(all, ctx, false)).toBeNull();
  });

  it('draws dots, junction rings, zero datum and closure markers in the section colour', () => {
    const rail = buildRail(all, ctx, true);
    expect(byKey(rail, all, 'st:1:0')).toMatchObject({ node: 'dot', zeroDatum: true, color: COLORS[1], above: null });
    expect(byKey(rail, all, 'st:1:2')).toMatchObject({ node: 'ring' });
    expect(byKey(rail, all, 'st:1:4')).toMatchObject({ closure: true, below: null });
    expect(byKey(rail, all, 'st:1:1')).toMatchObject({
      above: { style: 'solid', edge: 'edge' },
      below: { style: 'solid', edge: 'edge' },
    });
  });

  it('dashes Jump shots and solid-lines Normal ones', () => {
    const rail = buildRail(all, ctx, true);
    expect(byKey(rail, all, 'sh:2:202')).toMatchObject({ kind: 'shot', above: { style: 'dashed' }, below: { style: 'dashed' } });
    expect(byKey(rail, all, 'st:2:1')).toMatchObject({ above: { style: 'solid' }, below: { style: 'dashed' } });
  });

  it('elbows a branch section in from its parent on the header row', () => {
    const rail = buildRail(all, ctx, true);
    expect(byKey(rail, all, 'sec:2')).toMatchObject({ kind: 'header', elbow: { parentColor: COLORS[1], down: true } });
    expect(byKey(rail, all, 'st:2:0')).toMatchObject({ above: { style: 'solid', edge: 'edge' }, elbow: null });
  });

  it('elbows on station 0 itself without headers', () => {
    const rows = buildDataRows(ctx.data, { showSectionHeaders: false });
    const rail = buildRail(rows, ctx, true);
    expect(byKey(rail, rows, 'st:2:0')).toMatchObject({ above: null, elbow: { parentColor: COLORS[1], down: false } });
  });

  it('marks rows omitted by a filter', () => {
    const rows = buildDataRows(ctx.data, { showSectionHeaders: false }).filter(
      (r) => !['sh:1:102', 'st:1:2', 'sh:1:103'].includes(r.key) && r.sectionId === 1,
    );
    // Main: st0 sh1 st1 | (sh2 st2 sh3 omitted) | st3 sh4 st4
    const rail = buildRail(rows, ctx, true);
    expect(byKey(rail, rows, 'st:1:1')).toMatchObject({ below: { edge: 'edge' } });
    expect(byKey(rail, rows, 'st:1:3')).toMatchObject({ above: { edge: 'gap' } });
    const tail = rows.filter((r) => r.key !== 'st:1:0' && r.key !== 'sh:1:101');
    const rail2 = buildRail(tail, ctx, true);
    expect(byKey(rail2, tail, 'st:1:1')).toMatchObject({ above: { edge: 'cut' } });
    const head = rows.slice(0, 2); // st0, sh1 - the run continues below
    expect(buildRail(head, ctx, true)![1]).toMatchObject({ below: { edge: 'cut' } });
  });
});
