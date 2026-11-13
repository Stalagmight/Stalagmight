import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import '../../../../i18n';

vi.mock('../../model/edits', () => ({
  buildEditCommands: vi.fn((edits: { row: { key: string; kind: string; sectionId: number }; value: unknown }[]) => ({
    commands: edits.map((e) => ({ type: 'set_station_comment', station_ref: { key: e.row.key }, comment: e.value })),
    skipped: [],
  })),
}));

vi.mock('../../../../hooks/useEditingBlocked', () => ({
  // The Data mode's editability is the modes work's (isMapReadOnly); the
  // toolbar reads the shell's ctx.editingBlocked, set per test.
  isEditingBlockedNow: () => false,
  useEditingBlocked: () => false,
}));

import { useDataWorkspaceStore } from '../../store';
import { DATA_DEFAULT_HIDDEN } from '../../columnsMeta';
import { useUIStore } from '../../../../store/uiStore';
import { useProjectStore } from '../../../../store/projectStore';
import { buildEditCommands } from '../../model/edits';
import { Harness, makeModel, resetStore } from './harness';

const store = () => useDataWorkspaceStore.getState();
const executeBatch = vi.fn(async () => ({ success: true, warnings: [], error: null, section_ids: [] }));

beforeEach(() => {
  resetStore();
  useUIStore.setState({ appMode: 'data', activeDialog: null, toasts: [] });
  useProjectStore.setState({ executeBatch } as never);
  executeBatch.mockClear();
  vi.mocked(buildEditCommands).mockClear();
});

afterEach(() => {
  vi.useRealTimers();
});

function renderToolbar(ctxOpts: Parameters<typeof makeModel>[0] = {}) {
  const model = makeModel(ctxOpts);
  return { model, ...render(<Harness model={model} />) };
}

async function typeQuery(text: string) {
  const input = screen.getByTestId('data-find-input') as HTMLInputElement;
  act(() => input.focus());
  fireEvent.change(input, { target: { value: text } });
  await waitFor(() => expect(store().search.query).toBe(text));
  return input as HTMLInputElement;
}

describe('find box', () => {
  it('debounces the query, then counts the matches and selects the first', async () => {
    renderToolbar();
    const input = screen.getByTestId('data-find-input') as HTMLInputElement;
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: 'sump' } });
    // the input is instant; the store is not yet
    expect(input.value).toBe('sump');
    expect(store().search.query).toBe('');
    await waitFor(() => expect(screen.getByTestId('data-find-count')).toHaveTextContent('1 of 3'));
    expect(store().currentMatch).toBe(0);
    expect(store().focusRequest?.rowKey).toBe('sh:1:100001');
  });

  it('shows "No results" and an invalid-pattern error', async () => {
    renderToolbar();
    await typeQuery('zzz');
    expect(screen.getByTestId('data-find-count')).toHaveTextContent('No results');
    fireEvent.click(screen.getByTestId('data-find-regex'));
    await typeQuery('su(mp');
    expect(screen.getByTestId('data-find-error')).toHaveTextContent('Invalid pattern');
  });

  it('toggles match case / whole word / regex by click and Alt+C/W/R', async () => {
    renderToolbar();
    const input = await typeQuery('Sump');
    await waitFor(() => expect(screen.getByTestId('data-find-count')).toHaveTextContent('of 3'));
    const caseBtn = screen.getByTestId('data-find-match-case');
    fireEvent.click(caseBtn);
    expect(caseBtn).toHaveAttribute('aria-pressed', 'true');
    await waitFor(() => expect(screen.getByTestId('data-find-count')).toHaveTextContent('1 of 1'));
    fireEvent.keyDown(input, { key: 'c', code: 'KeyC', altKey: true });
    expect(store().search.matchCase).toBe(false);
    fireEvent.keyDown(input, { key: 'w', code: 'KeyW', altKey: true });
    expect(store().search.wholeWord).toBe(true);
    expect(screen.getByTestId('data-find-whole-word')).toHaveAttribute('aria-pressed', 'true');
    fireEvent.keyDown(input, { key: 'r', code: 'KeyR', altKey: true });
    expect(store().search.regex).toBe(true);
  });

  it('Enter / Shift+Enter and the buttons move through the matches, wrapping', async () => {
    renderToolbar();
    const input = await typeQuery('sump');
    await waitFor(() => expect(screen.getByTestId('data-find-count')).toHaveTextContent('1 of 3'));
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(screen.getByTestId('data-find-count')).toHaveTextContent('2 of 3');
    expect(store().focusRequest).toMatchObject({ rowKey: 'st:1:1', colKey: 'comment', align: 'center' });
    fireEvent.keyDown(input, { key: 'Enter', shiftKey: true });
    fireEvent.keyDown(input, { key: 'Enter', shiftKey: true });
    expect(screen.getByTestId('data-find-count')).toHaveTextContent('3 of 3');
    fireEvent.click(screen.getByTestId('data-find-next'));
    expect(screen.getByTestId('data-find-count')).toHaveTextContent('1 of 3');
    fireEvent.click(screen.getByTestId('data-find-prev'));
    expect(screen.getByTestId('data-find-count')).toHaveTextContent('3 of 3');
  });

  it('starts from the active row when the pattern changes', async () => {
    renderToolbar();
    act(() => store().setActiveRowKey('sh:2:200001'));
    await typeQuery('sump');
    await waitFor(() => expect(screen.getByTestId('data-find-count')).toHaveTextContent('3 of 3'));
  });

  it('scrolls to the match without taking focus from the find box', async () => {
    renderToolbar();
    const input = await typeQuery('sump');
    await waitFor(() => expect(store().focusRequest?.rowKey).toBe('sh:1:100001'));
    expect(document.activeElement).toBe(input);
    expect(store().focusRequest?.takeFocus).toBe(false);
  });

  it('Escape right after editing does not let the pending query reopen find', async () => {
    renderToolbar();
    const input = await typeQuery('s');
    fireEvent.change(input, { target: { value: '' } });
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(store().search.open).toBe(false);
    await new Promise((r) => setTimeout(r, 250));
    expect(store().search.open).toBe(false);
  });

  it('Escape clears the query, then closes find', async () => {
    renderToolbar();
    const input = await typeQuery('sump');
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(input.value).toBe('');
    expect(store().search.query).toBe('');
    expect(store().search.open).toBe(true);
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(store().search.open).toBe(false);
  });

  it('search options: row and column scope, only matching rows', async () => {
    renderToolbar();
    act(() => store().setFilters({ sectionIds: [2] }));
    await typeQuery('sump');
    await waitFor(() => expect(screen.getByTestId('data-find-count')).toHaveTextContent('1 of 1'));
    fireEvent.click(screen.getByTestId('data-find-options'));
    const menu = screen.getByTestId('data-find-options-menu');
    fireEvent.click(within(menu).getByTestId('data-find-rows-all'));
    expect(store().search.rowScope).toBe('all');
    await waitFor(() => expect(screen.getByTestId('data-find-count')).toHaveTextContent('of 3'));
    // revealed rows get a chip with Hide
    expect(screen.getByTestId('data-chip-revealed')).toHaveTextContent('2 rows shown by search');
    fireEvent.click(within(menu).getByTestId('data-find-cols-custom'));
    expect(store().search.columnScope).toBe('custom');
    expect(store().search.columns.length).toBeGreaterThan(0);
    fireEvent.click(within(menu).getByTestId('data-find-col-comment'));
    expect(store().search.columns).not.toContain('comment');
    await waitFor(() => expect(screen.getByTestId('data-find-count')).toHaveTextContent('No results'));
    fireEvent.click(within(menu).getByTestId('data-find-only-matching'));
    expect(store().search.onlyMatchingRows).toBe(true);
    expect(screen.getByTestId('data-find-options')).toHaveAttribute('data-active', 'true');
  });
});

describe('replace', () => {
  it('Replace all asks first with exact counts, then runs ONE batch and toasts a summary', async () => {
    renderToolbar({ frozen: ['1:0'] });
    act(() => store().setSearch({ replaceOpen: true, replacement: 'pool', columnScope: 'all' }));
    await typeQuery('sump');
    await waitFor(() => expect(screen.getByTestId('data-find-count')).toHaveTextContent('of 3'));
    fireEvent.click(screen.getByTestId('data-replace-all'));
    expect(screen.getByTestId('data-replace-confirm-text')).toHaveTextContent('Replace 3 matches in 3 cells?');
    fireEvent.click(screen.getByTestId('data-replace-confirm-ok'));
    await waitFor(() => expect(executeBatch).toHaveBeenCalledTimes(1));
    const commands = (executeBatch.mock.calls[0] as unknown as [{ comment: unknown }[]])[0];
    expect(commands.map((c) => c.comment)).toEqual(['pool one', 'pool pool', 'pool']);
    await waitFor(() =>
      expect(useUIStore.getState().toasts.map((x) => x.message)).toContain('Replaced 3'),
    );
  });

  it('a pending Replace all confirmation is dropped when the matches change', async () => {
    renderToolbar();
    act(() => store().setSearch({ replaceOpen: true, replacement: 'x' }));
    await typeQuery('sump');
    await waitFor(() => expect(screen.getByTestId('data-find-count')).toHaveTextContent('of 3'));
    fireEvent.click(screen.getByTestId('data-replace-all'));
    expect(screen.getByTestId('data-replace-confirm')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('data-find-match-case'));
    await waitFor(() => expect(screen.queryByTestId('data-replace-confirm')).toBeNull());
    expect(executeBatch).not.toHaveBeenCalled();
  });

  it('states the read-only matches it will skip', async () => {
    // length of the frozen shot (index 0) is locked
    renderToolbar({ frozen: ['1:0'] });
    act(() => store().setSearch({ replaceOpen: true, replacement: '2', columnScope: 'custom', columns: ['length'] }));
    await typeQuery('1');
    await waitFor(() => expect(screen.getByTestId('data-find-count')).toHaveTextContent('of 2'));
    expect(screen.getByTestId('data-find-locked')).toHaveTextContent('1 read-only');
    fireEvent.click(screen.getByTestId('data-replace-all'));
    expect(screen.getByTestId('data-replace-confirm-text')).toHaveTextContent(
      'Replace 1 match in 1 cell? 1 match in a read-only cell will be skipped.',
    );
    fireEvent.click(screen.getByTestId('data-replace-confirm-cancel'));
    expect(screen.queryByTestId('data-replace-confirm')).toBeNull();
    expect(executeBatch).not.toHaveBeenCalled();
  });

  it('Replace replaces only the current match and Skip moves on', async () => {
    renderToolbar();
    act(() => store().setSearch({ replaceOpen: true, replacement: 'X' }));
    await typeQuery('sump');
    await waitFor(() => expect(screen.getByTestId('data-find-count')).toHaveTextContent('1 of 3'));
    fireEvent.click(screen.getByTestId('data-replace-skip'));
    expect(screen.getByTestId('data-find-count')).toHaveTextContent('2 of 3');
    fireEvent.click(screen.getByTestId('data-replace-one'));
    await waitFor(() => expect(executeBatch).toHaveBeenCalledTimes(1));
    const edits = vi.mocked(buildEditCommands).mock.calls[0]![0];
    expect(edits.map((e) => [e.row.key, e.value])).toEqual([['st:1:1', 'X pool']]);
  });

  it('is disabled with the reason while the data is read-only', async () => {
    renderToolbar({ editingBlocked: true });
    act(() => store().setSearch({ replaceOpen: true }));
    expect(await screen.findByTestId('data-replace-blocked')).toHaveTextContent('read-only');
    expect(screen.getByTestId('data-replace-input')).toBeDisabled();
    expect(screen.queryByTestId('data-replace-all')).toBeNull();
  });
});

describe('keyboard shortcuts', () => {
  it('Ctrl+F focuses find, Ctrl+H opens replace, F3 / Shift+F3 navigate', async () => {
    renderToolbar();
    fireEvent.keyDown(window, { key: 'f', ctrlKey: true });
    expect(document.activeElement).toBe(screen.getByTestId('data-find-input'));
    expect(store().search.open).toBe(true);
    await typeQuery('sump');
    await waitFor(() => expect(screen.getByTestId('data-find-count')).toHaveTextContent('1 of 3'));
    fireEvent.keyDown(window, { key: 'F3' });
    expect(screen.getByTestId('data-find-count')).toHaveTextContent('2 of 3');
    fireEvent.keyDown(window, { key: 'F3', shiftKey: true });
    expect(screen.getByTestId('data-find-count')).toHaveTextContent('1 of 3');
    fireEvent.keyDown(window, { key: 'h', metaKey: true });
    expect(store().search.replaceOpen).toBe(true);
    await waitFor(() => expect(document.activeElement).toBe(screen.getByTestId('data-replace-input')));
  });

  it('stays out of the way behind a dialog and outside Data mode', () => {
    renderToolbar();
    useUIStore.setState({ activeDialog: 'settings' });
    const e1 = new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, cancelable: true });
    window.dispatchEvent(e1);
    expect(e1.defaultPrevented).toBe(false);
    useUIStore.setState({ activeDialog: null, appMode: 'survey' });
    const e2 = new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, cancelable: true });
    window.dispatchEvent(e2);
    expect(e2.defaultPrevented).toBe(false);
    expect(store().search.open).toBe(false);
  });
});

describe('filters, chips and menus', () => {
  it('row type control', () => {
    renderToolbar();
    fireEvent.click(screen.getByTestId('data-rowtype-shots'));
    expect(store().filters.rowTypes).toBe('shots');
    expect(screen.getByTestId('data-rowtype-shots')).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByTestId('data-chip-rowtype')).toHaveTextContent('Shots only');
    // a radio group: one tab stop, arrow keys move the choice
    expect(screen.getByTestId('data-rowtype-all')).toHaveAttribute('tabindex', '-1');
    fireEvent.keyDown(screen.getByTestId('data-rowtype-shots'), { key: 'ArrowRight' });
    expect(store().filters.rowTypes).toBe('all');
    fireEvent.keyDown(screen.getByTestId('data-rowtype-all'), { key: 'ArrowLeft' });
    expect(store().filters.rowTypes).toBe('shots');
  });

  it('one chip per active filter, each removable, and Clear all', () => {
    renderToolbar();
    act(() => {
      store().setFilters({ rowTypes: 'stations', sectionIds: [1, 2] });
      store().setQuickFilter('comments', true);
      store().addCondition({ id: 'c1', colKey: 'length', op: 'gt', value: '30' });
      store().addCondition({ id: 'c2', colKey: 'comment', op: 'contains', value: 'sump' });
      store().setSort({ colKey: 'length', dir: 'desc' });
    });
    const chips = screen.getByTestId('data-chips');
    expect(within(chips).getByTestId('data-chip-sections')).toHaveTextContent('Sections: Main, Side');
    expect(within(chips).getByTestId('data-chip-quick-comments')).toHaveTextContent('With comments');
    expect(within(chips).getByTestId('data-chip-condition-c1')).toHaveTextContent('Length (m) > 30');
    expect(within(chips).getByTestId('data-chip-condition-c2')).toHaveTextContent('Comment contains “sump”');
    // the sort has its own bar, not a chip
    expect(within(chips).queryByTestId('data-chip-sort')).toBeNull();
    fireEvent.click(screen.getByTestId('data-chip-condition-c1-remove'));
    expect(store().filters.conditions.map((c) => c.id)).toEqual(['c2']);
    fireEvent.click(screen.getByTestId('data-chip-sections-remove'));
    expect(store().filters.sectionIds).toBeNull();
    fireEvent.click(screen.getByTestId('data-chips-clear-all'));
    expect(store().filters.rowTypes).toBe('all');
    expect(store().filters.conditions).toEqual([]);
    expect(store().sort).toEqual({ colKey: 'length', dir: 'desc' });
    expect(screen.queryByTestId('data-chips')).toBeNull();
  });

  it('a navigation reveal shows a chip whose Hide clears it; a filter change clears it too', () => {
    renderToolbar();
    act(() => {
      store().setFilters({ sectionIds: [2] });
      store().revealAndFocus({ rowKey: 'st:1:1' });
    });
    expect(screen.getByTestId('data-chip-revealed')).toHaveTextContent('1 row shown by navigation');
    fireEvent.click(screen.getByTestId('data-chip-revealed-hide'));
    expect(store().revealRowKeys).toEqual([]);
    act(() => store().revealAndFocus({ rowKey: 'st:1:1' }));
    act(() => store().setQuickFilter('comments', true));
    expect(store().revealRowKeys).toEqual([]);
  });

  it('sections picker: toggle, Only, All', () => {
    renderToolbar();
    fireEvent.click(screen.getByTestId('data-sections-button'));
    fireEvent.click(screen.getByTestId('data-sections-item-1'));
    expect(store().filters.sectionIds).toEqual([2]);
    expect(screen.getByTestId('data-sections-button')).toHaveTextContent('Sections 1/2');
    fireEvent.click(screen.getByTestId('data-sections-item-1'));
    expect(store().filters.sectionIds).toBeNull();
    fireEvent.click(screen.getByTestId('data-sections-only-2'));
    expect(store().filters.sectionIds).toEqual([2]);
    fireEvent.change(screen.getByTestId('data-sections-search'), { target: { value: 'mai' } });
    expect(screen.queryByTestId('data-sections-item-2')).toBeNull();
    fireEvent.click(screen.getByTestId('data-sections-all'));
    expect(store().filters.sectionIds).toBeNull();
  });

  it('filter menu: quick filters and the condition builder (with validation)', () => {
    renderToolbar();
    fireEvent.click(screen.getByTestId('data-filter-button'));
    fireEvent.click(screen.getByTestId('data-filter-quick-issues'));
    expect(store().filters.quick.issues).toBe(true);
    const value = screen.getByTestId('data-filter-add-value');
    fireEvent.change(value, { target: { value: 'abc' } });
    expect(screen.getByTestId('data-filter-add-submit')).toBeDisabled();
    expect(screen.getByTestId('data-filter-add-problem')).toHaveTextContent('Enter a number');
    fireEvent.change(value, { target: { value: '30' } });
    fireEvent.keyDown(value, { key: 'Enter' });
    expect(store().filters.conditions).toMatchObject([{ colKey: 'length', op: 'eq', value: '30' }]);
    expect(screen.getByTestId('data-filter-button')).toHaveTextContent('Filter (2)');
    fireEvent.click(screen.getByTestId('data-filter-clear'));
    expect(store().filters.conditions).toEqual([]);
  });

  it('columns menu: toggle, Show all, Reset to default, count', () => {
    renderToolbar();
    fireEvent.click(screen.getByTestId('data-columns-button'));
    expect(screen.getByTestId('data-columns-count')).toHaveTextContent('30 of 30 shown');
    fireEvent.click(screen.getByTestId('data-columns-col-comment'));
    expect(store().hiddenColumns).toEqual(['comment']);
    expect(screen.getByTestId('data-columns-count')).toHaveTextContent('29 of 30 shown');
    fireEvent.click(screen.getByTestId('data-columns-reset'));
    expect(store().hiddenColumns).toEqual([...DATA_DEFAULT_HIDDEN]);
    expect(screen.getByTestId('data-columns-reset')).toBeDisabled();
    fireEvent.click(screen.getByTestId('data-columns-show-all'));
    expect(store().hiddenColumns).toEqual([]);
  });

  it('tint menu picks a mode and shows its key', () => {
    renderToolbar();
    fireEvent.click(screen.getByTestId('data-tint-button'));
    fireEvent.click(screen.getByTestId('data-tint-type'));
    expect(store().tintMode).toBe('type');
    expect(screen.getByTestId('data-tint-legend')).toHaveTextContent('Station');
    expect(screen.getByTestId('data-tint-button')).toHaveTextContent('Tint: Row type');
    act(() => store().setTintMode('section'));
    expect(screen.getByTestId('data-tint-button')).toHaveTextContent('Tint: Section');
    fireEvent.click(screen.getByTestId('data-tint-none'));
    expect(screen.queryByTestId('data-tint-legend')).toBeNull();
  });

  it('view menu: section headers and collapse / expand all', () => {
    renderToolbar();
    fireEvent.click(screen.getByTestId('data-view-button'));
    fireEvent.click(screen.getByTestId('data-view-collapse-all'));
    expect(store().collapsedSections).toEqual([1, 2]);
    expect(screen.getByTestId('harness-rows')).toHaveTextContent(/^sec:1 sec:2$/);
    fireEvent.click(screen.getByTestId('data-view-expand-all'));
    expect(store().collapsedSections).toEqual([]);
    fireEvent.click(screen.getByTestId('data-view-section-headers'));
    expect(store().showSectionHeaders).toBe(false);
    expect(screen.getByTestId('data-view-collapse-all')).toBeDisabled();
  });

  it('map toggle', () => {
    renderToolbar();
    const btn = screen.getByTestId('data-map-toggle');
    expect(btn).toHaveAttribute('aria-pressed', 'false');
    fireEvent.click(btn);
    expect(store().mapPaneOpen).toBe(true);
    expect(btn).toHaveAttribute('aria-pressed', 'true');
  });
});

describe('sort bar', () => {
  it('says what the rows are sorted by and returns to survey order', () => {
    renderToolbar();
    expect(screen.queryByTestId('data-sort-bar')).toBeNull();
    act(() => store().setSort({ colKey: 'length', dir: 'asc' }));
    expect(screen.getByTestId('data-sort-bar-text')).toHaveTextContent(
      'Sorted by Length (m), ascending. Survey order and the link rail are off.',
    );
    fireEvent.click(screen.getByTestId('data-sort-reset'));
    expect(store().sort).toBeNull();
    expect(screen.queryByTestId('data-sort-bar')).toBeNull();
  });
});
