/**
 * Pure helpers behind the Data-workspace grid props: find-match segmentation,
 * applicability, Tab order that skips N/A cells, the fill-handle extent and the
 * header sort cycle.
 */
import { describe, expect, it } from 'vitest';
import { markCellClass, splitMarks } from '../gridMarks';
import { cellApplies, columnApplies, isGroupRow } from '../gridApplicable';
import { tabTargetWhere } from '../gridGeometry';
import { fillExtent } from '../gridAutoFit';
import { nextSortDir } from '../GridSortButton';
import type { GridColumn } from '../types';

describe('splitMarks', () => {
  const kinds = (text: string, marks: Parameters<typeof splitMarks>[1]) =>
    splitMarks(text, marks).map((s) => `${s.kind}:${s.text}`);

  it('splits plain / match / plain', () => {
    expect(kinds('shot one', { ranges: [[5, 8]] })).toEqual(['plain:shot ', 'match:one']);
    expect(kinds('abcabc', { ranges: [[0, 1], [3, 4]] })).toEqual(['match:a', 'plain:bc', 'match:a', 'plain:bc']);
  });

  it('draws the current match stronger, inside or outside the ranges', () => {
    expect(kinds('abcabc', { ranges: [[0, 1], [3, 4]], current: [3, 4] })).toEqual([
      'match:a',
      'plain:bc',
      'current:a',
      'plain:bc',
    ]);
    // A current range not repeated in `ranges` still shows.
    expect(kinds('abc', { ranges: [], current: [1, 2] })).toEqual(['plain:a', 'current:b', 'plain:c']);
  });

  it('merges overlaps and sorts unsorted ranges', () => {
    expect(kinds('abcdef', { ranges: [[3, 5], [0, 2], [1, 4]] })).toEqual(['match:abcde', 'plain:f']);
  });

  it('clamps out-of-range and drops empty ranges instead of throwing', () => {
    expect(kinds('abc', { ranges: [[2, 99], [-5, 0], [1, 1]] })).toEqual(['plain:ab', 'match:c']);
    expect(kinds('', { ranges: [[0, 3]] })).toEqual([]);
    // Reversed bounds are normalized.
    expect(kinds('abc', { ranges: [[2, 1]] })).toEqual(['plain:a', 'match:b', 'plain:c']);
  });

  it('splits a merged match around the current one', () => {
    expect(kinds('abcdef', { ranges: [[0, 6]], current: [2, 4] })).toEqual(['match:ab', 'current:cd', 'match:ef']);
  });

  it('markCellClass', () => {
    expect(markCellClass(undefined)).toBe('');
    expect(markCellClass({ ranges: [] })).toBe('');
    expect(markCellClass({ ranges: [[0, 1]] })).toBe(' sg-cell-match');
    expect(markCellClass({ ranges: [[0, 1]], current: [0, 1] })).toBe(' sg-cell-match sg-cell-match-current');
    // A custom-render cell can't hold <mark>s: it is outlined instead.
    expect(markCellClass({ ranges: [[0, 1]] }, true)).toBe(' sg-cell-match sg-cell-match-outline');
    expect(markCellClass({ ranges: [[0, 1]], current: [0, 1] }, true)).toBe(
      ' sg-cell-match sg-cell-match-current sg-cell-match-outline',
    );
    expect(markCellClass({ ranges: [] }, true)).toBe('');
  });
});

interface R {
  kind: 'station' | 'shot' | 'group';
  v: number | null;
}
const col: GridColumn<R> = {
  key: 'v',
  header: 'V',
  getValue: (r) => r.v,
  isApplicable: (r) => r.kind === 'shot',
};

describe('applicability', () => {
  it('isGroupRow treats null / undefined / false as an ordinary row', () => {
    const row: R = { kind: 'group', v: null };
    expect(isGroupRow(undefined, row, 0)).toBe(false);
    expect(isGroupRow(() => undefined, row, 0)).toBe(false);
    expect(isGroupRow(() => null, row, 0)).toBe(false);
    expect(isGroupRow(() => false, row, 0)).toBe(false);
    expect(isGroupRow(() => 'Section A', row, 0)).toBe(true);
    expect(isGroupRow(() => 0, row, 0)).toBe(true);
  });

  it('a column applies by default; a group row has no applicable cell', () => {
    const plain: GridColumn<R> = { key: 'p', header: 'P', getValue: () => 1 };
    expect(columnApplies(plain, { kind: 'station', v: null }, 0)).toBe(true);
    expect(columnApplies(col, { kind: 'station', v: null }, 0)).toBe(false);
    expect(cellApplies(plain, { kind: 'group', v: null }, 0, (r) => (r.kind === 'group' ? 'G' : undefined))).toBe(false);
    expect(cellApplies(col, { kind: 'shot', v: 1 }, 0, (r) => (r.kind === 'group' ? 'G' : undefined))).toBe(true);
  });
});

describe('tabTargetWhere', () => {
  // 3 writable columns, 4 rows; (row 1, col 1) and all of row 2 are refused.
  const writable = [0, 1, 2];
  const accept = (r: number, c: number) => !(r === 2 || (r === 1 && c === 1));

  it('steps over a refused cell within the row', () => {
    expect(tabTargetWhere(1, 0, 1, writable, 4, accept)).toEqual({ row: 1, colIdx: 2 });
    expect(tabTargetWhere(1, 2, -1, writable, 4, accept)).toEqual({ row: 1, colIdx: 0 });
  });

  it('skips a wholly refused row (a group header)', () => {
    expect(tabTargetWhere(1, 2, 1, writable, 4, accept)).toEqual({ row: 3, colIdx: 0 });
    expect(tabTargetWhere(3, 0, -1, writable, 4, accept)).toEqual({ row: 1, colIdx: 2 });
  });

  it('stays put (null) when nothing acceptable remains before the edge', () => {
    expect(tabTargetWhere(3, 2, 1, writable, 4, () => true)).toEqual({ row: 3, colIdx: 2 });
    expect(tabTargetWhere(1, 2, 1, writable, 3, accept)).toBeNull();
  });

  it('behaves exactly like tabTarget when everything is accepted', () => {
    expect(tabTargetWhere(0, 2, 1, writable, 4, () => true)).toEqual({ row: 1, colIdx: 0 });
    expect(tabTargetWhere(0, 0, 1, [], 4, () => true)).toBeNull();
  });
});

describe('fillExtent over interleaved row kinds', () => {
  const rows: R[] = [
    { kind: 'shot', v: 1 },
    { kind: 'station', v: null },
    { kind: 'shot', v: 2 },
    { kind: 'station', v: null },
    { kind: 'shot', v: null },
    { kind: 'station', v: null },
  ];
  it('passes over N/A guide cells and stops at the first applicable gap', () => {
    const applies = (i: number) => rows[i]!.kind === 'shot';
    expect(fillExtent(col, rows, 0, rows.length, applies)).toBe(2);
  });
  it('without the predicate keeps the old stop-at-first-empty rule', () => {
    expect(fillExtent(col, rows, 0, rows.length)).toBe(0);
  });
});

describe('nextSortDir', () => {
  it('cycles asc -> desc -> none -> asc', () => {
    expect(nextSortDir(null)).toBe('asc');
    expect(nextSortDir('asc')).toBe('desc');
    expect(nextSortDir('desc')).toBeNull();
  });
});
