import { describe, expect, it } from 'vitest';
import { buildDataView } from '../view';
import { sortRows } from '../sort';
import { EMPTY_FILTERS, EMPTY_QUICK_FILTERS, EMPTY_SEARCH } from '../../store';
import { ROW_NAME_KEY, type DataColumnDef, type DataEntityRow, type SearchState } from '../../types';
import { rowNameSearchText } from '../search';
import { ROW_ID_MODE, rowIdLabel } from '../rowIds';
import { rowName } from '../ctx';
import { keys, makeColumns, makeCtx, makeData, makeRows, viewState, type SectionSeed } from './fixtures';

const SECTIONS: SectionSeed[] = [
  {
    id: 1,
    name: 'Main',
    shots: [
      { length: 10, azimuth: 0, comment: 'sump entrance' },
      { length: 35, azimuth: 90 },
      { length: 5, azimuth: 180 },
    ],
    stationComments: { 2: 'Sump two' },
    stationNames: { 0: 'ENT' },
  },
  {
    id: 2,
    name: 'Side',
    shots: [{ length: 12, comment: 'sump' }, { length: 50 }],
  },
];

function setup(opts: { headers?: boolean; ctx?: Parameters<typeof makeCtx>[1] } = {}) {
  const data = makeData(SECTIONS);
  const rows = makeRows(data, { headers: opts.headers ?? true });
  const ctx = makeCtx(data, opts.ctx);
  const columns = makeColumns();
  return { data, rows, ctx, columns };
}

const search = (query: string, patch: Partial<SearchState> = {}): SearchState => ({
  ...EMPTY_SEARCH,
  open: true,
  query,
  ...patch,
});

describe('buildDataView - filters and collapse', () => {
  it('no state: every row in survey order, counts, survey order', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(rows, columns, ctx, viewState());
    expect(keys(v.rows)).toEqual(keys(rows));
    expect(v.totalCount).toBe(12);
    expect(v.filteredCount).toBe(12);
    expect(v.surveyOrder).toBe(true);
    expect(v.matches).toEqual([]);
    expect(v.revealedRowKeys.size).toBe(0);
  });

  it('keeps a header only when its section shows a row', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(rows, columns, ctx, viewState({ filters: { ...EMPTY_FILTERS, sectionIds: [2] } }));
    expect(keys(v.rows)).toEqual(['sec:2', 'st:2:0', 'sh:2:200001', 'st:2:1', 'sh:2:200002', 'st:2:2']);
    expect(v.filteredCount).toBe(5);
    expect(v.totalCount).toBe(12);
  });

  it('a collapsed section shows only its header; filteredCount is unchanged', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(rows, columns, ctx, viewState({ collapsedSections: [1] }));
    expect(keys(v.rows).slice(0, 2)).toEqual(['sec:1', 'sec:2']);
    expect(v.filteredCount).toBe(12);
  });

  it('collapse is ignored when there are no headers to expand from', () => {
    const { rows, ctx, columns } = setup({ headers: false });
    const v = buildDataView(rows, columns, ctx, viewState({ collapsedSections: [1] }));
    expect(v.rows).toHaveLength(12);
  });

  it('collapse is ignored while sorted (the headers are gone)', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(rows, columns, ctx, viewState({ collapsedSections: [1], sort: { colKey: 'length', dir: 'asc' } }));
    expect(v.rows).toHaveLength(12);
  });

  it('a collapsed section whose rows all fail the filters loses its header', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(
      rows,
      columns,
      ctx,
      viewState({ collapsedSections: [1], filters: { ...EMPTY_FILTERS, sectionIds: [2] } }),
    );
    expect(keys(v.rows)[0]).toBe('sec:2');
  });

  it('navigation reveals show hidden rows and list them as revealed', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(
      rows,
      columns,
      ctx,
      viewState({ filters: { ...EMPTY_FILTERS, sectionIds: [2] }, revealRowKeys: ['st:1:1', 'sh:2:200001'] }),
    );
    expect(keys(v.rows)).toEqual([
      'sec:1',
      'st:1:1',
      'sec:2',
      'st:2:0',
      'sh:2:200001',
      'st:2:1',
      'sh:2:200002',
      'st:2:2',
    ]);
    expect([...v.revealedRowKeys]).toEqual(['st:1:1']);
  });
});

describe('buildDataView - row name search', () => {
  const nameMatches = (v: ReturnType<typeof buildDataView>) =>
    v.matches.filter((m) => m.colKey === ROW_NAME_KEY).map((m) => [m.rowKey, m.start, m.end]);

  it('finds a section by name through its rows, read-only, with the Section column hidden', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(rows, columns, ctx, viewState({ search: search('side'), hiddenColumns: ['section'] }));
    // The gutter shows only the ID ("2:0"): the hit is in the full name after it.
    expect(nameMatches(v)).toEqual([
      ['st:2:0', 4, 8],
      ['sh:2:200001', 4, 8],
      ['st:2:1', 4, 8],
      ['sh:2:200002', 4, 8],
      ['st:2:2', 4, 8],
    ]);
    expect(v.matches.every((m) => m.colKey !== ROW_NAME_KEY || m.lockReason === 'computed')).toBe(true);
    expect(v.matchIndex.get('st:2:1')!.get(ROW_NAME_KEY)).toHaveLength(1);
    // Never a revealed column: the gutter is always shown.
    expect(v.revealedColumns.size).toBe(0);
  });

  it('matches the row ID, else the full section name, once per row', () => {
    const { rows, ctx, columns } = setup();
    ctx.sectionLabels.set(2, 'Si…ge');
    ctx.sectionNames.set(2, 'Side passage 2');
    const row = rows.find((r) => r.key === 'st:2:1') as DataEntityRow;
    // The canonical ID, a line break, the full name; the short label is gone.
    expect(rowNameSearchText(ctx, row)).toBe('2:1\nSide passage 2');
    const v = buildDataView(rows, columns, ctx, viewState({ search: search('passage'), hiddenColumns: ['section'] }));
    expect(nameMatches(v).find(([k]) => k === 'st:2:1')).toEqual(['st:2:1', 9, 16]);
    // "2" is in both the ID and the full name: one match, in the ID.
    const both = buildDataView(rows, columns, ctx, viewState({ search: search('2'), hiddenColumns: ['section'] }));
    expect(nameMatches(both).filter(([k]) => k === 'st:2:1')).toEqual([['st:2:1', 0, 1]]);
    // The short label is not searched any more.
    const label = buildDataView(rows, columns, ctx, viewState({ search: search('Si…'), hiddenColumns: ['section'] }));
    expect(nameMatches(label)).toEqual([]);
  });

  it('follows ROW_ID_MODE: the canonical ID by default, the bare index on request', () => {
    const { rows } = setup();
    const st = rows.find((r) => r.key === 'st:2:1') as DataEntityRow;
    const sh = rows.find((r) => r.key === 'sh:2:200002') as DataEntityRow;
    expect(ROW_ID_MODE).toBe('canonical');
    expect([rowIdLabel(st), rowIdLabel(sh)]).toEqual(['2:1', '2:2']);
    expect([rowIdLabel(st, 'index'), rowIdLabel(sh, 'index')]).toEqual(['1', '2']);
  });

  it('"Copy row name" copies the canonical ID; a header row its section name', () => {
    const { rows, ctx } = setup();
    ctx.sectionNames.set(2, 'Side passage');
    const st = rows.find((r) => r.key === 'st:2:1')!;
    const header = rows.find((r) => r.key === 'sec:2')!;
    expect(rowName(ctx, st)).toBe('2:1');
    expect(rowName(ctx, header)).toBe('Side passage');
  });

  it('comes first in its row, before the cell matches', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(rows, columns, ctx, viewState({ search: search('side') }));
    expect(v.matches.filter((m) => m.rowKey === 'st:2:0').map((m) => m.colKey)).toEqual([ROW_NAME_KEY, 'section']);
  });

  it("is searched under the 'visible' and 'all' column scopes, not 'custom'", () => {
    const { rows, ctx, columns } = setup();
    const scoped = (patch: Partial<SearchState>) =>
      nameMatches(buildDataView(rows, columns, ctx, viewState({ search: search('main', patch), hiddenColumns: ['section'] })));
    expect(scoped({ columnScope: 'all' }).length).toBeGreaterThan(0);
    expect(scoped({ columnScope: 'custom', columns: ['comment'] })).toEqual([]);
  });

  it('respects the row scope: a hidden row is searched only under "all rows"', () => {
    const { rows, ctx, columns } = setup();
    const filters = { ...EMPTY_FILTERS, sectionIds: [1] };
    const visible = buildDataView(rows, columns, ctx, viewState({ filters, search: search('side'), hiddenColumns: ['section'] }));
    expect(nameMatches(visible)).toEqual([]);
    const all = buildDataView(
      rows,
      columns,
      ctx,
      viewState({ filters, search: search('side', { rowScope: 'all' }), hiddenColumns: ['section'] }),
    );
    expect(nameMatches(all)).toHaveLength(5);
    expect(all.revealedRowKeys.has('st:2:1')).toBe(true);
  });
});

describe('buildDataView - search', () => {
  it('finds matches in display order with lock reasons and an index', () => {
    const { rows, ctx, columns } = setup({ ctx: { frozen: ['2:0'] } });
    const v = buildDataView(
      rows,
      columns,
      ctx,
      viewState({ search: search('sump'), hiddenColumns: [] }),
    );
    expect(v.matches.map((m) => [m.rowKey, m.colKey, m.start, m.end])).toEqual([
      ['sh:1:100001', 'comment', 0, 4],
      ['st:1:2', 'comment', 0, 4],
      ['sh:2:200001', 'comment', 0, 4],
    ]);
    expect(v.matches.every((m) => m.lockReason === null)).toBe(true);
    expect(v.matchIndex.get('st:1:2')!.get('comment')).toEqual([1]);
  });

  it('several matches in a cell are listed by offset; columns follow the column order', () => {
    const { rows, ctx, columns } = setup();
    const cells = (v: ReturnType<typeof buildDataView>, rowKey: string) =>
      v.matches.filter((m) => m.rowKey === rowKey).map((m) => [m.colKey, m.start]);
    const byOffset = buildDataView(rows, columns, ctx, viewState({ search: search('e') }));
    expect(cells(byOffset, 'sh:1:100001')).toEqual([
      ['comment', 5],
      ['comment', 12],
    ]);
    const defaultOrder = buildDataView(rows, columns, ctx, viewState({ search: search('s') }));
    expect(cells(defaultOrder, 'st:1:2')).toEqual([
      ['type', 0],
      ['comment', 0],
    ]);
    const reordered = buildDataView(rows, columns, ctx, viewState({ search: search('s'), columnOrder: ['comment'] }));
    expect(cells(reordered, 'st:1:2')).toEqual([
      ['comment', 0],
      ['type', 0],
    ]);
  });

  it('takes the lock reason from the column (computed column, frozen shot)', () => {
    const { rows, ctx, columns } = setup({ ctx: { frozen: ['1:1'] } });
    const v = buildDataView(rows, columns, ctx, viewState({ search: search('35') }));
    expect(v.matches.map((m) => [m.rowKey, m.colKey, m.lockReason])).toEqual([['sh:1:100002', 'length', 'frozen']]);
    const v2 = buildDataView(rows, columns, ctx, viewState({ search: search('Main') }));
    expect(new Set(v2.matches.map((m) => m.lockReason))).toEqual(new Set(['computed']));
  });

  it('never matches section headers and searches nothing while the find bar is closed', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(rows, columns, ctx, viewState({ search: search('Main') }));
    expect(v.matches.some((m) => m.rowKey.startsWith('sec:'))).toBe(false);
    const closed = buildDataView(rows, columns, ctx, viewState({ search: { ...search('sump'), open: false } }));
    expect(closed.matches).toEqual([]);
  });

  it('an invalid regex finds nothing', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(rows, columns, ctx, viewState({ search: search('su(mp', { regex: true }) }));
    expect(v.matches).toEqual([]);
    expect(v.rows).toHaveLength(rows.length);
  });

  it('column scope visible skips hidden columns; all reveals hidden columns holding a match', () => {
    const { rows, ctx, columns } = setup();
    const hidden = { hiddenColumns: ['comment' as const] };
    const visible = buildDataView(rows, columns, ctx, viewState({ ...hidden, search: search('sump') }));
    expect(visible.matches).toEqual([]);
    const all = buildDataView(rows, columns, ctx, viewState({ ...hidden, search: search('sump', { columnScope: 'all' }) }));
    expect(all.matches).toHaveLength(3);
    expect([...all.revealedColumns]).toEqual(['comment']);
    const noHit = buildDataView(rows, columns, ctx, viewState({ hiddenColumns: ['x'], search: search('sump', { columnScope: 'all' }) }));
    expect(noHit.revealedColumns.size).toBe(0);
  });

  it('custom column scope searches exactly the chosen columns, hidden or not', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(
      rows,
      columns,
      ctx,
      viewState({ hiddenColumns: ['comment'], search: search('s', { columnScope: 'custom', columns: ['comment'] }) }),
    );
    expect(new Set(v.matches.map((m) => m.colKey))).toEqual(new Set(['comment']));
    expect([...v.revealedColumns]).toEqual(['comment']);
  });

  it('row scope visible searches only rows on screen', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(
      rows,
      columns,
      ctx,
      viewState({ filters: { ...EMPTY_FILTERS, sectionIds: [2] }, search: search('sump') }),
    );
    expect(v.matches.map((m) => m.rowKey)).toEqual(['sh:2:200001']);
    expect(v.revealedRowKeys.size).toBe(0);
  });

  it('row scope all reveals filtered-out and collapsed rows that match', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(
      rows,
      columns,
      ctx,
      viewState({
        filters: { ...EMPTY_FILTERS, sectionIds: [2] },
        collapsedSections: [2],
        search: search('sump', { rowScope: 'all' }),
      }),
    );
    expect(keys(v.rows)).toEqual(['sec:1', 'sh:1:100001', 'st:1:2', 'sec:2', 'sh:2:200001']);
    expect([...v.revealedRowKeys].sort()).toEqual(['sh:1:100001', 'sh:2:200001', 'st:1:2']);
    expect(v.filteredCount).toBe(5);
  });

  it('navigation-revealed rows are searched in the visible scope', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(
      rows,
      columns,
      ctx,
      viewState({ filters: { ...EMPTY_FILTERS, sectionIds: [2] }, revealRowKeys: ['st:1:2'], search: search('sump') }),
    );
    expect(v.matches.map((m) => m.rowKey)).toEqual(['st:1:2', 'sh:2:200001']);
  });

  it('only matching rows narrows to rows with a match and keeps their headers', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(
      rows,
      columns,
      ctx,
      viewState({ search: search('sump', { onlyMatchingRows: true }), revealRowKeys: ['st:2:2'] }),
    );
    expect(keys(v.rows)).toEqual(['sec:1', 'sh:1:100001', 'st:1:2', 'sec:2', 'sh:2:200001', 'st:2:2']);
    expect(v.searchNarrowing).toBe(true);
    // no query: the option does nothing
    const idle = buildDataView(rows, columns, ctx, viewState({ search: search('', { onlyMatchingRows: true }) }));
    expect(idle.rows).toHaveLength(rows.length);
    expect(idle.searchNarrowing).toBe(false);
  });

  it('only matching rows with no match shows nothing (no orphan headers)', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(rows, columns, ctx, viewState({ search: search('zzz', { onlyMatchingRows: true }), collapsedSections: [1] }));
    expect(v.rows).toEqual([]);
  });

  it('matches follow the sorted order', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(
      rows,
      columns,
      ctx,
      viewState({ search: search('sump'), sort: { colKey: 'comment', dir: 'desc' } }),
    );
    expect(v.matches.map((m) => m.rowKey)).toEqual(['st:1:2', 'sh:1:100001', 'sh:2:200001']);
  });
});

describe('buildDataView / sortRows - sort', () => {
  it('drops headers, sorts numerically, keeps blanks last in both directions, ties stable', () => {
    const { rows, ctx, columns } = setup();
    const asc = buildDataView(rows, columns, ctx, viewState({ sort: { colKey: 'length', dir: 'asc' } }));
    expect(asc.surveyOrder).toBe(false);
    expect(asc.rows.some((r) => r.kind === 'section')).toBe(false);
    expect(keys(asc.rows).slice(0, 5)).toEqual(['sh:1:100003', 'sh:1:100001', 'sh:2:200001', 'sh:1:100002', 'sh:2:200002']);
    // the stations (no length) follow in survey order
    expect(keys(asc.rows).slice(5)).toEqual(['st:1:0', 'st:1:1', 'st:1:2', 'st:1:3', 'st:2:0', 'st:2:1', 'st:2:2']);
    const desc = buildDataView(rows, columns, ctx, viewState({ sort: { colKey: 'length', dir: 'desc' } }));
    expect(keys(desc.rows).slice(0, 5)).toEqual(['sh:2:200002', 'sh:1:100002', 'sh:2:200001', 'sh:1:100001', 'sh:1:100003']);
    expect(keys(desc.rows).slice(5)).toEqual(keys(asc.rows).slice(5));
  });

  it('text sorts with numeric collation, case-insensitively', () => {
    const { ctx } = setup();
    const values = ['A10', 'a2', null, 'A1', '', 'b'];
    const rows = values.map((_, i) => ({ kind: 'station', key: `k${i}`, sectionId: 1, sectionOrder: 0, order: i, stationIndex: i, g: i }) as DataEntityRow);
    const col = { ...makeColumns()[0]!, value: (r: DataEntityRow) => values[r.order] ?? null } as DataColumnDef;
    expect(sortRows(rows, col, 'asc', ctx).map((r) => values[r.order])).toEqual(['A1', 'a2', 'A10', 'b', null, '']);
    expect(sortRows(rows, col, 'desc', ctx).map((r) => values[r.order])).toEqual(['b', 'A10', 'a2', 'A1', null, '']);
  });

  it('puts numbers before text and NaN with the blanks', () => {
    const { ctx } = setup();
    const values: (number | string | null)[] = ['x', 3, Number.NaN, 1, null];
    const rows = values.map((_, i) => ({ kind: 'station', key: `k${i}`, sectionId: 1, sectionOrder: 0, order: i, stationIndex: i, g: i }) as DataEntityRow);
    const col = { ...makeColumns()[0]!, value: (r: DataEntityRow) => values[r.order] ?? null } as DataColumnDef;
    expect(sortRows(rows, col, 'asc', ctx).map((r) => r.order)).toEqual([3, 1, 0, 2, 4]);
    expect(sortRows(rows, col, 'desc', ctx).map((r) => r.order)).toEqual([1, 3, 0, 2, 4]);
  });

  it('an unknown sort column leaves survey order', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(rows, columns, ctx, viewState({ sort: { colKey: 'nope' as never, dir: 'asc' } }));
    expect(v.surveyOrder).toBe(true);
    expect(keys(v.rows)).toEqual(keys(rows));
  });

  it('sorting keeps the filter and the reveals', () => {
    const { rows, ctx, columns } = setup();
    const v = buildDataView(
      rows,
      columns,
      ctx,
      viewState({
        filters: { ...EMPTY_FILTERS, quick: { ...EMPTY_QUICK_FILTERS, comments: true } },
        revealRowKeys: ['sh:2:200002'],
        sort: { colKey: 'length', dir: 'desc' },
      }),
    );
    expect(keys(v.rows)).toEqual(['sh:2:200002', 'sh:2:200001', 'sh:1:100001', 'st:1:2']);
    expect([...v.revealedRowKeys]).toEqual(['sh:2:200002']);
  });
});

describe('buildDataView - performance', () => {
  it('filters + searches 100k entity rows x 12 searchable columns within budget', () => {
    const shotsPer = 500;
    const sections: SectionSeed[] = Array.from({ length: 100 }, (_, s) => ({
      id: s + 1,
      name: `Section ${s + 1}`,
      shots: Array.from({ length: shotsPer }, (_, i) => ({
        length: (i % 97) + 0.5,
        azimuth: (i * 7) % 360,
        comment: i % 50 === 0 ? `note ${i} sump` : undefined,
      })),
    }));
    const data = makeData(sections);
    const rows = makeRows(data, { headers: true });
    const ctx = makeCtx(data);
    const base = makeColumns();
    // 12 searchable columns: the fixture set plus cheap clones.
    const columns = [...base, ...base.slice(3, 6)];
    expect(rows.length).toBeGreaterThan(100_000);
    const t0 = performance.now();
    const v = buildDataView(
      rows,
      columns,
      ctx,
      viewState({ search: search('sump'), filters: { ...EMPTY_FILTERS, rowTypes: 'all' } }),
    );
    const plain = performance.now() - t0;
    expect(v.matches.length).toBe(1000);
    const t1 = performance.now();
    buildDataView(rows, columns, ctx, viewState({ search: search('s.mp|\\d+5', { regex: true, rowScope: 'all' }) }));
    const regex = performance.now() - t1;
    // Generous CI budget; the target on a typical machine is < 150 ms.
    if (process.env.DATA_VIEW_PERF_LOG) console.log({ plain, regex });
    expect(plain).toBeLessThan(2000);
    expect(regex).toBeLessThan(3000);
  });
});
