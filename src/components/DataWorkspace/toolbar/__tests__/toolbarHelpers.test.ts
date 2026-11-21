import { describe, expect, it } from 'vitest';
import { exactMatchIndex, firstAtOrAfter, firstFromRow, reconcileCurrent, type MatchOrder } from '../matchNav';
import { findShortcutOf } from '../useFindShortcuts';
import { toggleSection } from '../SectionsPicker';
import { settleOutcome } from '../useReplace';
import { patternKey } from '../useFindNavigation';
import { sameFilters } from '../revealPolicy';
import { EMPTY_FILTERS } from '../../store';
import type { ReplacePlan } from '../../model/replace';
import type { DataCellEdit, DataColumnKey, SearchMatch } from '../../types';

const m = (rowKey: string, colKey: DataColumnKey, start: number): SearchMatch => ({
  rowKey,
  colKey,
  start,
  end: start + 1,
  lockReason: null,
});

// rows r0..r5 in display order; survey order differs for r9 (not shown)
const rows = ['r0', 'r1', 'r2', 'r3', 'r4', 'r5'];
const order: MatchOrder = {
  rowPos: (k) => (rows.includes(k) ? rows.indexOf(k) : undefined),
  colPos: (c) => ['name', 'length', 'comment'].indexOf(c),
  surveyOrder: (k) => (k === 'r9' ? 2.5 : rows.indexOf(k)),
};
const matches = [m('r1', 'comment', 0), m('r1', 'comment', 5), m('r3', 'name', 0), m('r3', 'comment', 2), m('r5', 'length', 0)];

describe('matchNav', () => {
  it('exactMatchIndex finds the same spot', () => {
    expect(exactMatchIndex(matches, { rowKey: 'r3', colKey: 'comment', start: 2 })).toBe(3);
    expect(exactMatchIndex(matches, { rowKey: 'r3', colKey: 'comment', start: 3 })).toBe(-1);
  });

  it('firstAtOrAfter: same row by column then offset, later rows, wrap', () => {
    expect(firstAtOrAfter(matches, { rowKey: 'r1', colKey: 'comment', start: 1 }, order)).toBe(1);
    expect(firstAtOrAfter(matches, { rowKey: 'r1', colKey: 'comment', start: 6 }, order)).toBe(2);
    expect(firstAtOrAfter(matches, { rowKey: 'r3', colKey: 'length', start: 0 }, order)).toBe(3);
    expect(firstAtOrAfter(matches, { rowKey: 'r2', colKey: 'name', start: 0 }, order)).toBe(2);
    expect(firstAtOrAfter(matches, { rowKey: 'r5', colKey: 'comment', start: 0 }, order)).toBe(0);
    expect(firstAtOrAfter([], { rowKey: 'r1', colKey: 'name', start: 0 }, order)).toBe(-1);
  });

  it('firstAtOrAfter: a row that left the view continues from its survey position', () => {
    expect(firstAtOrAfter(matches, { rowKey: 'r9', colKey: 'name', start: 0 }, order)).toBe(2);
  });

  it('firstFromRow starts at the active row', () => {
    expect(firstFromRow(matches, 'r2', order)).toBe(2);
    expect(firstFromRow(matches, null, order)).toBe(0);
    expect(firstFromRow(matches, 'gone', order)).toBe(0);
    expect(firstFromRow(matches, 'r5', order)).toBe(4);
  });

  it('reconcileCurrent: anchor > new pattern > same match > next one', () => {
    const base = { matches, activeRowKey: 'r2', order };
    expect(
      reconcileCurrent({ ...base, previous: null, anchor: { rowKey: 'r1', colKey: 'comment', start: 3 }, patternChanged: true }),
    ).toBe(1);
    expect(reconcileCurrent({ ...base, previous: { rowKey: 'r1', colKey: 'comment', start: 0 }, anchor: null, patternChanged: true })).toBe(2);
    expect(reconcileCurrent({ ...base, previous: { rowKey: 'r3', colKey: 'comment', start: 2 }, anchor: null, patternChanged: false })).toBe(3);
    expect(reconcileCurrent({ ...base, previous: { rowKey: 'r3', colKey: 'name', start: 4 }, anchor: null, patternChanged: false })).toBe(3);
    expect(reconcileCurrent({ ...base, matches: [], previous: null, anchor: null, patternChanged: true })).toBe(-1);
  });

  it('patternKey changes with the pattern, not the scope', () => {
    const s = { query: 'a', matchCase: false, wholeWord: false, regex: false, open: true };
    expect(patternKey(s)).toBe(patternKey({ ...s }));
    expect(patternKey(s)).not.toBe(patternKey({ ...s, regex: true }));
    expect(patternKey(s)).not.toBe(patternKey({ ...s, query: 'b' }));
    expect(patternKey(s)).not.toBe(patternKey({ ...s, open: false }));
  });
});

describe('findShortcutOf', () => {
  const k = (key: string, mods: Partial<Record<'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey', boolean>> & { code?: string } = {}) =>
    findShortcutOf({ key, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...mods });
  it('maps Ctrl/Cmd+F, Ctrl/Cmd+H, F3 and Shift+F3', () => {
    expect(k('f', { ctrlKey: true })).toBe('find');
    expect(k('F', { metaKey: true })).toBe('find');
    expect(k('h', { ctrlKey: true })).toBe('replace');
    expect(k('F3')).toBe('next');
    expect(k('F3', { shiftKey: true })).toBe('prev');
  });
  it('reads the physical key on non-Latin layouts, and Ctrl/Cmd+Alt+F opens replace', () => {
    expect(k('а', { ctrlKey: true, code: 'KeyF' })).toBe('find');
    expect(k('р', { metaKey: true, code: 'KeyH' })).toBe('replace');
    expect(k('ƒ', { metaKey: true, altKey: true, code: 'KeyF' })).toBe('replace');
    expect(k('f', { ctrlKey: true, altKey: true, code: 'KeyF' })).toBe('replace');
    expect(k('h', { ctrlKey: true, altKey: true, code: 'KeyH' })).toBeNull();
  });
  it('ignores other combinations', () => {
    expect(k('f')).toBeNull();
    expect(k('f', { ctrlKey: true, shiftKey: true })).toBeNull();
    expect(k('F3', { ctrlKey: true })).toBeNull();
    expect(k('g', { ctrlKey: true })).toBeNull();
  });
});

describe('sameFilters', () => {
  it('compares by content', () => {
    expect(sameFilters(EMPTY_FILTERS, { ...EMPTY_FILTERS })).toBe(true);
    expect(sameFilters(EMPTY_FILTERS, { ...EMPTY_FILTERS, rowTypes: 'all', quick: { ...EMPTY_FILTERS.quick } })).toBe(true);
    expect(sameFilters({ ...EMPTY_FILTERS, sectionIds: [1, 2] }, { ...EMPTY_FILTERS, sectionIds: [1, 2] })).toBe(true);
    expect(sameFilters({ ...EMPTY_FILTERS, sectionIds: [1, 2] }, { ...EMPTY_FILTERS, sectionIds: [2, 1] })).toBe(false);
    expect(sameFilters(EMPTY_FILTERS, { ...EMPTY_FILTERS, sectionIds: [] })).toBe(false);
    expect(sameFilters(EMPTY_FILTERS, { ...EMPTY_FILTERS, quick: { ...EMPTY_FILTERS.quick, frozen: true } })).toBe(false);
    const c = { id: 'a', colKey: 'length' as const, op: 'gt' as const, value: '3' };
    expect(sameFilters({ ...EMPTY_FILTERS, conditions: [c] }, { ...EMPTY_FILTERS, conditions: [{ ...c }] })).toBe(true);
    expect(sameFilters({ ...EMPTY_FILTERS, conditions: [c] }, { ...EMPTY_FILTERS, conditions: [{ ...c, value: '4' }] })).toBe(false);
  });
});

describe('toggleSection', () => {
  const all = [1, 2, 3];
  it('null means all; toggling builds a list and returns to null when complete', () => {
    expect(toggleSection(null, 2, all)).toEqual([1, 3]);
    expect(toggleSection([1, 3], 2, all)).toBeNull();
    expect(toggleSection([1], 1, all)).toEqual([]);
    expect(toggleSection([], 3, all)).toEqual([3]);
  });
});

describe('settleOutcome', () => {
  it('moves matches of edits buildEditCommands skipped into locked / invalid', () => {
    const e1 = { colKey: 'comment' } as DataCellEdit;
    const e2 = { colKey: 'length' } as DataCellEdit;
    const e3 = { colKey: 'length' } as DataCellEdit;
    const plan: ReplacePlan = {
      edits: [e1, e2, e3],
      editMatches: [2, 1, 3],
      editTexts: [],
      replacedMatches: 6,
      lockedMatches: 4,
      lockedCells: 2,
      invalid: [{ rowKey: 'x', colKey: 'length', text: 'q', matches: 1 }],
      unchangedCells: 0,
      roundedCells: 0,
    };
    expect(settleOutcome(plan, [])).toEqual({ replaced: 6, locked: 4, invalid: 1, conflict: 0 });
    expect(settleOutcome(plan, [{ edit: e2, reason: 'frozen' }, { edit: e3, reason: 'invalid' }])).toEqual({
      replaced: 2,
      locked: 5,
      invalid: 2,
      conflict: 0,
    });
    // An averaged angle dropped because its legs were replaced too.
    expect(settleOutcome(plan, [{ edit: e1, reason: 'conflict' }])).toEqual({
      replaced: 4,
      locked: 4,
      invalid: 1,
      conflict: 2,
    });
  });
});
