import { describe, expect, it } from 'vitest';
import { applyFilters, compileCondition, conditionProblem, filtersActive, opsForValueType } from '../filter';
import { EMPTY_FILTERS, EMPTY_QUICK_FILTERS } from '../../store';
import { DATA_COLUMN_BY_KEY } from '../../columnsMeta';
import type { DataColumnDef, DataModelCtx, DataRow, FilterCondition, FilterState } from '../../types';
import { makeColumns, makeCtx, makeData, makeRows, warning, type SectionSeed } from './fixtures';

const SECTIONS: SectionSeed[] = [
  {
    id: 1,
    name: 'Main',
    shots: [
      { length: 10, azimuth: 0, comment: 'sump' },
      { length: 35, azimuth: 90 },
      { length: 5, azimuth: 180, kind: 'Jump' },
      { length: 40, azimuth: 5 },
    ],
    // depths 0, 10, 60, 55, 20
    z: [0, 10, 60, 55, 20],
  },
  {
    id: 2,
    name: 'Side',
    shots: [{ length: 12 }, { length: 50, comment: 'Big room' }],
    stationComments: { 1: 'Junction' },
  },
];

function setup(ctxOpts: Parameters<typeof makeCtx>[1] = {}) {
  const data = makeData(SECTIONS);
  const rows = makeRows(data, { headers: true });
  const ctx = makeCtx(data, ctxOpts);
  const columns = makeColumns();
  return { data, rows, ctx, columns };
}

let condId = 0;
const cond = (colKey: FilterCondition['colKey'], op: FilterCondition['op'], value = ''): FilterCondition => ({
  id: `c${++condId}`,
  colKey,
  op,
  value,
});

function kept(
  rows: readonly DataRow[],
  columns: readonly DataColumnDef[],
  ctx: DataModelCtx,
  patch: Partial<FilterState>,
): string[] {
  const pass = applyFilters(rows, columns, ctx, { ...EMPTY_FILTERS, ...patch });
  return rows.filter((_, i) => pass[i] === 1).map((r) => r.key);
}

const quick = (patch: Partial<typeof EMPTY_QUICK_FILTERS>) => ({ quick: { ...EMPTY_QUICK_FILTERS, ...patch } });

describe('applyFilters - basics', () => {
  it('passes every entity row and never a header with no filters', () => {
    const { rows, ctx, columns } = setup();
    const out = kept(rows, columns, ctx, {});
    expect(out).toHaveLength(rows.filter((r) => r.kind !== 'section').length);
    expect(out.some((k) => k.startsWith('sec:'))).toBe(false);
  });

  it('row type keeps one kind', () => {
    const { rows, ctx, columns } = setup();
    expect(kept(rows, columns, ctx, { rowTypes: 'stations' }).every((k) => k.startsWith('st:'))).toBe(true);
    expect(kept(rows, columns, ctx, { rowTypes: 'shots' })).toEqual([
      'sh:1:100001',
      'sh:1:100002',
      'sh:1:100003',
      'sh:1:100004',
      'sh:2:200001',
      'sh:2:200002',
    ]);
  });

  it('sections keep only the chosen sections; an empty list keeps nothing', () => {
    const { rows, ctx, columns } = setup();
    expect(kept(rows, columns, ctx, { sectionIds: [2] })).toEqual([
      'st:2:0',
      'sh:2:200001',
      'st:2:1',
      'sh:2:200002',
      'st:2:2',
    ]);
    expect(kept(rows, columns, ctx, { sectionIds: [] })).toEqual([]);
  });

  it('filtersActive reflects each kind of filter', () => {
    expect(filtersActive(EMPTY_FILTERS)).toBe(false);
    expect(filtersActive({ ...EMPTY_FILTERS, rowTypes: 'shots' })).toBe(true);
    expect(filtersActive({ ...EMPTY_FILTERS, sectionIds: [1] })).toBe(true);
    expect(filtersActive({ ...EMPTY_FILTERS, ...quick({ frozen: true }) })).toBe(true);
    expect(filtersActive({ ...EMPTY_FILTERS, conditions: [cond('length', 'gt', '1')] })).toBe(true);
  });
});

describe('applyFilters - quick filters', () => {
  it('issues: shots with an UNRESOLVED warning and the stations next to them', () => {
    const { rows, ctx, columns } = setup({ warnings: [warning(1, 100003), warning(2, 200001, true)] });
    expect(kept(rows, columns, ctx, quick({ issues: true }))).toEqual(['st:1:2', 'sh:1:100003', 'st:1:3']);
  });

  it('comments: any row kind with a non-empty comment, judged on the row itself', () => {
    const { rows, ctx, columns } = setup();
    expect(kept(rows, columns, ctx, quick({ comments: true }))).toEqual(['sh:1:100001', 'st:2:1', 'sh:2:200002']);
  });

  it('links: stations with a merge or closure and the shots touching them', () => {
    const { data, rows, ctx, columns } = setup();
    data.stationMergedWith.set(2, [{ section_id: 2, station_index: 0 } as never]);
    data.stationClosuresWith.set(6, [{} as never]); // st:2:1 (global station 6)
    expect(kept(rows, columns, ctx, quick({ links: true }))).toEqual([
      'sh:1:100002',
      'st:1:2',
      'sh:1:100003',
      'sh:2:200001',
      'st:2:1',
      'sh:2:200002',
    ]);
  });

  it('frozen: frozen shots (keyed by shot index) and their stations', () => {
    const { rows, ctx, columns } = setup({ frozen: ['1:0', 'bogus'] });
    expect(kept(rows, columns, ctx, quick({ frozen: true }))).toEqual(['st:1:0', 'sh:1:100001', 'st:1:1']);
  });

  it('virtual shots: jumps and references and their stations', () => {
    const { rows, ctx, columns } = setup();
    expect(kept(rows, columns, ctx, quick({ virtualShots: true }))).toEqual(['st:1:2', 'sh:1:100003', 'st:1:3']);
  });

  it('quick filters AND together', () => {
    const { rows, ctx, columns } = setup({ warnings: [warning(1, 100003)] });
    expect(kept(rows, columns, ctx, quick({ issues: true, virtualShots: true }))).toEqual([
      'st:1:2',
      'sh:1:100003',
      'st:1:3',
    ]);
    expect(kept(rows, columns, ctx, quick({ issues: true, comments: true }))).toEqual([]);
  });
});

describe('applyFilters - cross-kind conditions', () => {
  it('a shot-only condition keeps the matching shots and the stations bracketing them', () => {
    const { rows, ctx, columns } = setup();
    expect(kept(rows, columns, ctx, { conditions: [cond('length', 'gt', '30')] })).toEqual([
      'st:1:1',
      'sh:1:100002',
      'st:1:2',
      'st:1:3',
      'sh:1:100004',
      'st:1:4',
      'st:2:1',
      'sh:2:200002',
      'st:2:2',
    ]);
  });

  it('a station-only condition keeps the matching stations and the shots touching them', () => {
    const { rows, ctx, columns } = setup();
    expect(kept(rows, columns, ctx, { conditions: [cond('depth', 'gt', '50')] })).toEqual([
      'sh:1:100002',
      'st:1:2',
      'sh:1:100003',
      'st:1:3',
      'sh:1:100004',
    ]);
  });

  it('shot and station conditions together: each row needs both, through its neighbours', () => {
    const { rows, ctx, columns } = setup();
    const conditions = [cond('length', 'gt', '30'), cond('depth', 'gt', '50')];
    expect(kept(rows, columns, ctx, { conditions })).toEqual(['sh:1:100002', 'st:1:2', 'st:1:3', 'sh:1:100004']);
  });

  it('several shot-only conditions must hold on the SAME neighbouring shot', () => {
    const { rows, ctx, columns } = setup();
    // st:1:1 sits between a north shot (az 0, 10 m) and a long shot (35 m, az 90):
    // each condition holds on a neighbour, but no single neighbour holds both.
    const conditions = [cond('length', 'gt', '30'), cond('azimuth', 'lt', '10')];
    expect(kept(rows, columns, ctx, { conditions })).toEqual([
      'st:1:3',
      'sh:1:100004',
      'st:1:4',
      'st:2:1',
      'sh:2:200002',
      'st:2:2',
    ]);
  });

  it('row type only hides a kind afterwards: Stations + a shot condition = the bracketing stations', () => {
    const { rows, ctx, columns } = setup();
    expect(kept(rows, columns, ctx, { rowTypes: 'stations', conditions: [cond('length', 'gt', '30')] })).toEqual([
      'st:1:1',
      'st:1:2',
      'st:1:3',
      'st:1:4',
      'st:2:1',
      'st:2:2',
    ]);
  });

  it('a shared-column condition judges every row directly', () => {
    const { rows, ctx, columns } = setup();
    expect(kept(rows, columns, ctx, { conditions: [cond('comment', 'contains', 'SUMP')] })).toEqual(['sh:1:100001']);
    expect(kept(rows, columns, ctx, { conditions: [cond('type', 'eq', 'station')] })).toHaveLength(8);
  });

  it('a section filter limits the projection to the chosen sections', () => {
    const { rows, ctx, columns } = setup();
    expect(
      kept(rows, columns, ctx, { sectionIds: [2], conditions: [cond('length', 'gt', '30')] }),
    ).toEqual(['st:2:1', 'sh:2:200002', 'st:2:2']);
  });

  it('an invalid condition is ignored rather than hiding everything', () => {
    const { rows, ctx, columns } = setup();
    expect(kept(rows, columns, ctx, { conditions: [cond('length', 'gt', 'abc')] })).toHaveLength(14);
    expect(kept(rows, columns, ctx, { conditions: [cond('nope' as never, 'gt', '1')] })).toHaveLength(14);
  });
});

describe('compileCondition - operators', () => {
  const run = (c: FilterCondition, ctxOpts: Parameters<typeof makeCtx>[1] = {}, sections = SECTIONS) => {
    const data = makeData(sections);
    const rows = makeRows(data);
    const ctx = makeCtx(data, ctxOpts);
    const col = makeColumns().find((x) => x.key === c.colKey)!;
    const pred = compileCondition(c, col, ctx);
    if (!pred) return null;
    return rows.filter((r) => r.kind !== 'section' && pred(r as never)).map((r) => r.key);
  };
  const LENGTHS: SectionSeed[] = [{ id: 7, name: 'L', shots: [{ length: 10.04 }, { length: 10.4 }, { length: 9.96 }] }];

  it('numeric = compares at the displayed precision, or the typed one when finer', () => {
    expect(run(cond('length', 'eq', '10'), { precision: 1 }, LENGTHS)).toEqual(['sh:7:700001', 'sh:7:700003']);
    expect(run(cond('length', 'eq', '10.04'), { precision: 1 }, LENGTHS)).toEqual(['sh:7:700001']);
    expect(run(cond('length', 'eq', '10'), { precision: 2 }, LENGTHS)).toEqual([]);
  });

  it('numeric != also keeps rows with no value; < <= > >= skip them', () => {
    const out = run(cond('length', 'neq', '10'), { precision: 1 }, LENGTHS)!;
    expect(out).toContain('sh:7:700002');
    expect(out).toContain('st:7:0'); // stations have no length
    expect(run(cond('length', 'lt', '10'), { precision: 2 }, LENGTHS)).toEqual(['sh:7:700003']);
    // 9.96 shows as 10.0 at one decimal: not less than 10.
    expect(run(cond('length', 'lt', '10'), { precision: 1 }, LENGTHS)).toEqual([]);
    expect(run(cond('length', 'lte', '10.04'), {}, LENGTHS)).toEqual(['sh:7:700001', 'sh:7:700003']);
    expect(run(cond('length', 'gt', '10.04'), {}, LENGTHS)).toEqual(['sh:7:700002']);
    expect(run(cond('length', 'gte', '10.4'), {}, LENGTHS)).toEqual(['sh:7:700002']);
    // 10.04 shows as 10.0 at one decimal: not greater than 10, but >= 10.
    expect(run(cond('length', 'gt', '10'), { precision: 1 }, LENGTHS)).toEqual(['sh:7:700002']);
    expect(run(cond('length', 'gte', '10'), { precision: 1 }, LENGTHS)).toEqual([
      'sh:7:700001',
      'sh:7:700002',
      'sh:7:700003',
    ]);
  });

  it('compares at the precision each cell shows, rounding like the cell text', () => {
    // 3.25 shows as "3.3": it is "= 3.3" and "> 3.2", not "= 3.2"
    const HALF: SectionSeed[] = [{ id: 7, name: 'L', shots: [{ length: 3.25 }, { length: 12.75 }] }];
    expect(run(cond('length', 'eq', '3.2'), { precision: 1 }, HALF)).toEqual([]);
    expect(run(cond('length', 'eq', '3.3'), { precision: 1 }, HALF)).toEqual(['sh:7:700001']);
    expect(run(cond('length', 'gt', '3.2'), { precision: 1 }, HALF)).toEqual(['sh:7:700001', 'sh:7:700002']);
    expect(run(cond('length', 'lt', '12.8'), { precision: 1 }, HALF)).toEqual(['sh:7:700001']);
  });

  it('a grouped number in a comma-decimal locale is an integer target', () => {
    const BIG: SectionSeed[] = [{ id: 7, name: 'L', shots: [{ length: 1000.04 }, { length: 1000.4 }] }];
    // de-DE: "1.000" is one thousand (not 1 at three decimals); the cells show "1000"
    expect(run(cond('length', 'eq', '1.000'), { precision: 0, locale: 'de-DE' }, BIG)).toEqual(['sh:7:700001', 'sh:7:700002']);
    expect(run(cond('length', 'eq', '1'), { precision: 0, locale: 'de-DE' }, BIG)).toEqual([]);
  });

  it('numeric values parse with the locale', () => {
    expect(run(cond('length', 'gt', '10,1'), { locale: 'de-DE' }, LENGTHS)).toEqual(['sh:7:700002']);
    expect(run(cond('length', 'gt', '10,1'), { locale: 'en-US' }, LENGTHS)).toBeNull();
  });

  it('text operators ignore case; = ignores surrounding spaces', () => {
    expect(run(cond('comment', 'contains', 'ROOM'))).toEqual(['sh:2:200002']);
    expect(run(cond('comment', 'notContains', 'room'))).toHaveLength(13);
    expect(run(cond('comment', 'eq', '  junction '))).toEqual(['st:2:1']);
    expect(run(cond('comment', 'neq', 'junction'))).toHaveLength(13);
  });

  it('empty / not empty look at the value', () => {
    expect(run(cond('comment', 'notEmpty'))).toEqual(['sh:1:100001', 'st:2:1', 'sh:2:200002']);
    expect(run(cond('comment', 'empty'))).toHaveLength(11);
    // a shot-only column is empty on every station
    expect(run(cond('length', 'empty'))!.every((k) => k.startsWith('st:'))).toBe(true);
  });

  it('enum = matches the shown text case-insensitively', () => {
    expect(run(cond('kind', 'eq', 'jump'))).toEqual(['sh:1:100003']);
  });

  it('date operators compare ISO days', () => {
    const data = makeData(SECTIONS);
    const ctx = makeCtx(data);
    const rows = makeRows(data).filter((r) => r.kind === 'shot');
    const dates = ['2024-01-05', '2024-03-01', '', '2023-12-31', '2024-03-01T10:00', '2025-01-01'];
    const meta = DATA_COLUMN_BY_KEY.get('date')!;
    const dateCol: DataColumnDef = {
      ...meta,
      value: (row) => (row.kind === 'shot' ? dates[rows.indexOf(row)] || null : null),
      text: (row) => (row.kind === 'shot' ? (dates[rows.indexOf(row)] ?? '') : ''),
      lockReason: () => null,
    };
    const pick = (op: FilterCondition['op'], value: string) => {
      const pred = compileCondition(cond('date', op, value), dateCol, ctx)!;
      return rows.filter((r) => pred(r)).map((r) => rows.indexOf(r));
    };
    expect(pick('eq', '2024-03-01')).toEqual([1, 4]);
    expect(pick('lt', '2024-03-01')).toEqual([0, 3]);
    expect(pick('lte', '2024-03-01')).toEqual([0, 1, 3, 4]);
    expect(pick('gt', '2024-03-01')).toEqual([5]);
    expect(pick('gte', '2024-03-01')).toEqual([1, 4, 5]);
    expect(pick('neq', '2024-03-01')).toEqual([0, 2, 3, 5]);
    expect(pick('empty', '')).toEqual([2]);
  });
});

describe('conditionProblem / opsForValueType', () => {
  const number = { valueType: 'number' as const };
  it('flags missing values, bad numbers and bad dates', () => {
    expect(conditionProblem({ op: 'gt', value: ' ' }, number, 'en-US')).toBe('needsValue');
    expect(conditionProblem({ op: 'gt', value: 'abc' }, number, 'en-US')).toBe('notANumber');
    expect(conditionProblem({ op: 'gt', value: '1,5' }, number, 'de-DE')).toBeNull();
    expect(conditionProblem({ op: 'empty', value: '' }, number, 'en-US')).toBeNull();
    expect(conditionProblem({ op: 'eq', value: '01/02/2024' }, { valueType: 'date' }, 'en-US')).toBe('notADate');
    expect(conditionProblem({ op: 'eq', value: '2024-02-01' }, { valueType: 'date' }, 'en-US')).toBeNull();
    expect(conditionProblem({ op: 'contains', value: 'x' }, { valueType: 'text' }, 'en-US')).toBeNull();
  });

  it('offers type-appropriate operators', () => {
    expect(opsForValueType('number')).not.toContain('contains');
    expect(opsForValueType('text')).toContain('contains');
    expect(opsForValueType('text')).not.toContain('gt');
    expect(opsForValueType('date')).toContain('lt');
    expect(opsForValueType('enum')).toEqual(['eq', 'neq', 'empty', 'notEmpty']);
  });
});
