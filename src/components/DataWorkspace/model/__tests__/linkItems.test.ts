/**
 * A station's links as the chips list them, the text find searches for them
 * (the ROW_LINKS_KEY pseudo-column), and where those matches sit.
 */
import { describe, expect, it } from 'vitest';
import { buildDataRows } from '../rows';
import { buildDataView } from '../view';
import { itemsHit, linkItemLabel, linksSearchText, stationLinkItems, type LinkItem } from '../linkItems';
import { planReplace } from '../replace';
import { EMPTY_SEARCH } from '../../store';
import { ROW_LINKS_KEY, ROW_NAME_KEY, type DataRow, type SearchState, type StationRow } from '../../types';
import { COLUMNS, makeCtx } from '../../__tests__/dataFixtures';
import { viewState } from './fixtures';

const ctx = makeCtx();
const rows = buildDataRows(ctx.data, { showSectionHeaders: true });
const station = (key: string): StationRow => {
  const r = rows.find((x: DataRow) => x.key === key);
  if (!r || r.kind !== 'station') throw new Error(`no station ${key}`);
  return r;
};
const text = (key: string) => linksSearchText(stationLinkItems(ctx, station(key)));
const search = (query: string, patch: Partial<SearchState> = {}): SearchState => ({ ...EMPTY_SEARCH, open: true, query, ...patch });

describe('stationLinkItems', () => {
  it('lists zero datum, merges, branch start and closures with their misclosure', () => {
    expect(text('st:1:0')).toBe('Zero datum');
    expect(text('st:1:2')).toBe('⇄ Branch B:0');
    expect(text('st:2:0')).toBe('Continues from Main:2');
    expect(text('st:1:4')).toBe('Closes to Downstre…sump 1A:3 · Δ 0.70 m · 1.3 %');
    expect(text('st:1:3')).toBe('');
  });

  it('keeps the station name apart from the prose (the chips set it bold)', () => {
    const [closure] = stationLinkItems(ctx, station('st:1:4'));
    expect(closure).toMatchObject({ kind: 'closure', before: 'Closes to ', name: 'Downstre…sump 1A:3', after: '' });
    expect(closure!.target).toEqual({ section_id: 3, station_index: 3 });
    const [zero] = stationLinkItems(ctx, station('st:1:0'));
    expect(zero).toMatchObject({ kind: 'zero', name: null, target: null });
    const [branch] = stationLinkItems(ctx, station('st:2:0'));
    expect(branch).toMatchObject({ before: 'Continues from ', name: 'Main:2' });
  });

  it('maps match ranges in the search text to the chips they fall in', () => {
    const items: LinkItem[] = ['⇄ A:1', 'Closes to B:2'].map((label, i) => ({
      key: `k${i}`,
      kind: 'merge',
      before: label,
      name: null,
      after: '',
      misclosure: '',
      target: null,
      closure: null,
    }));
    const all = linksSearchText(items);
    expect(all).toBe('⇄ A:1\nCloses to B:2');
    expect([...itemsHit(items, [[2, 5]])]).toEqual([0]);
    expect([...itemsHit(items, [[all.indexOf('B:2'), all.length]])]).toEqual([1]);
    expect([...itemsHit(items, [[4, 10]])]).toEqual([0, 1]);
    expect(linkItemLabel(items[1]!)).toBe('Closes to B:2');
  });
});

describe('find in the links (ROW_LINKS_KEY)', () => {
  const linkMatches = (v: ReturnType<typeof buildDataView>) =>
    v.matches.filter((m) => m.colKey === ROW_LINKS_KEY).map((m) => [m.rowKey, m.start, m.end]);

  it('finds a station by its links, never editable, under the visible / all scopes', () => {
    const v = buildDataView(rows, COLUMNS, ctx, viewState({ search: search('branch b:0') }));
    expect(linkMatches(v)).toEqual([['st:1:2', 2, 12]]);
    expect(v.matches.find((m) => m.colKey === ROW_LINKS_KEY)!.lockReason).toBe('computed');
    expect(v.revealedColumns.size).toBe(0);
    expect(linkMatches(buildDataView(rows, COLUMNS, ctx, viewState({ search: search('branch b:0', { columnScope: 'all' }) })))).toHaveLength(1);
    expect(
      linkMatches(buildDataView(rows, COLUMNS, ctx, viewState({ search: search('branch b:0', { columnScope: 'custom', columns: ['comment'] }) }))),
    ).toEqual([]);
  });

  it('lists a station links match after the row name and the columns before the shot-only block', () => {
    // st:1:2 is in "Main", merges with "Branch B:0" and has the comment
    // "Junction" (after the shot-only block).
    const v = buildDataView(rows, COLUMNS, ctx, viewState({ search: search('Main|Branch|Junction', { regex: true }), hiddenColumns: [] }));
    const st = v.matches.filter((m) => m.rowKey === 'st:1:2').map((m) => m.colKey);
    // Row name ("Main"), the section column (before the block), the links
    // (at the block), the comment (after it).
    expect(st).toEqual([ROW_NAME_KEY, 'section', ROW_LINKS_KEY, 'comment']);
  });

  it('a replace counts links matches as locked and never plans them', () => {
    const v = buildDataView(rows, COLUMNS, ctx, viewState({ search: search('branch b:0') }));
    const model = {
      rowByKey: new Map(rows.map((r) => [r.key, r])),
      columnByKey: new Map(COLUMNS.map((c) => [c.key, c])),
    } as unknown as Parameters<typeof planReplace>[1];
    const links = v.matches.filter((m) => m.colKey === ROW_LINKS_KEY);
    const plan = planReplace(links, model, ctx, { ...search('branch b:0'), replacement: 'x' });
    expect(plan.edits).toEqual([]);
    expect(plan.lockedMatches).toBe(1);
  });
});
