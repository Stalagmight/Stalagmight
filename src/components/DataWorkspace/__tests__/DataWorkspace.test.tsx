/**
 * The Data workspace shell over a synthetic 3-section survey (merge, closure,
 * zero datum, frozen shot, Jump shot, warnings): interleaved rows, gutter
 * names, rail glyphs, link chips, hover card, edit dispatch, context menu
 * actions, the Back to Data chip and Open in Data.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, renderHook, screen, within } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () => Array.from({ length: count }, (_, index) => ({ index, key: index, start: 33 + index * 33, size: 33 })),
    getTotalSize: () => count * 33,
    scrollToIndex: vi.fn(),
  }),
}));

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({ invokeIpc: invokeIpcMock }));
const blocked = vi.hoisted(() => ({ value: false }));
vi.mock('../../../hooks/useEditingBlocked', () => ({
  useEditingBlocked: () => blocked.value,
  isEditingBlockedNow: () => blocked.value,
}));

import '../../../i18n';
import { DataWorkspace } from '../DataWorkspace';
import { BackToDataChip } from '../BackToDataChip';
import { DataMapPaneHeader } from '../DataMapPaneHeader';
import { OpenInDataButton } from '../OpenInDataButton';
import { useDataWorkspaceStore } from '../store';
import { resetDataWorkspaceViewPersistenceForTests } from '../useDataWorkspaceViewPersistence';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { defaultConfig, type Config } from '../../../types/config';
import { makeCtx, makeData, makeProject, WARNINGS } from './dataFixtures';
import { DataSectionHeader } from '../DataSectionHeader';
import { sectionStatsOf, useDataWorkspaceModel } from '../useDataWorkspaceModel';
import { openRowInSurvey, useDataNavigation, type DataNav } from '../useDataNavigation';

class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

const actions = {
  updateShot: vi.fn(() => Promise.resolve({})),
  updateStationComment: vi.fn(() => Promise.resolve({})),
  updateStationName: vi.fn(() => Promise.resolve({})),
  editShotCredit: vi.fn(() => Promise.resolve({})),
  executeBatch: vi.fn((_commands: unknown[]) => Promise.resolve({})),
  setShotKind: vi.fn(() => Promise.resolve({})),
};

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(() => Promise.resolve(undefined));
  // The persisted-view baseline is module state: forget it, as a fresh app
  // start would, or the previous test's map pane state leaks into this one.
  resetDataWorkspaceViewPersistenceForTests();
  blocked.value = false;
  for (const fn of Object.values(actions)) fn.mockClear();
  useProjectStore.setState({
    config: structuredClone(defaultConfig) as Config,
    project: makeProject(),
    surveyData: makeData(),
    surveyDataAdjusted: null,
    warnings: WARNINGS,
    frozenShots: [{ section_id: 3, shot_index: 0 }],
    ...actions,
  } as never);
  useUIStore.setState({ toasts: [], geometryMode: 'measured', selection: null, multiSelection: [], appMode: 'data', requestPanToShot: null });
  useDataWorkspaceStore.setState({
    ...useDataWorkspaceStore.getInitialState(),
    tintMode: 'section',
    showSectionHeaders: true,
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

const rowIds = () =>
  [...document.querySelectorAll<HTMLElement>('[data-testid^="data-row-"]')]
    .map((el) => el.dataset.testid!)
    .filter((id) => /^data-row-(st|sh|sec)-[\d-]+$/.test(id));

/** The gutter name element of a row ("st-1-1", "sh-1-2"). */
const rowNameEl = (part: string) => screen.getByTestId(['data', 'row', 'name', part].join('-'));

describe('DataWorkspace rows and gutter', () => {
  it('interleaves every section: header, station 0, shot 1, station 1, ...', () => {
    render(<DataWorkspace />);
    expect(rowIds().slice(0, 11)).toEqual([
      'data-row-sec-1',
      'data-row-st-1-0',
      'data-row-sh-1-1',
      'data-row-st-1-1',
      'data-row-sh-1-2',
      'data-row-st-1-2',
      'data-row-sh-1-3',
      'data-row-st-1-3',
      'data-row-sh-1-4',
      'data-row-st-1-4',
      'data-row-sec-2',
    ]);
    expect(rowIds()).toHaveLength(24);
  });

  it('names rows by their canonical IDs only: stations with their name, shots with from -> to', () => {
    render(<DataWorkspace />);
    expect(rowNameEl('st-1-1')).toHaveTextContent('1:1A1');
    expect(rowNameEl('sh-1-2')).toHaveTextContent('1:21→2');
    expect(rowNameEl('st-3-3')).toHaveTextContent('3:3');
    // The section is named by its header, not repeated on every row.
    expect(rowNameEl('st-3-3')).not.toHaveTextContent('Downstre');
    expect(rowNameEl('sh-1-2')).not.toHaveTextContent('Main');
  });

  it('draws the rail: dot, zero datum, junction ring, closure, dashed Jump line, branch elbow', () => {
    render(<DataWorkspace />);
    const gutter = (id: string) => screen.getByTestId(id);
    expect(gutter('data-row-st-1-0').querySelector('.dw-rail-dot')).not.toBeNull();
    expect(gutter('data-row-st-1-0').querySelector('.dw-rail-zero')).not.toBeNull();
    expect(gutter('data-row-st-1-2').querySelector('.dw-rail-ring')).not.toBeNull();
    expect(gutter('data-row-st-1-4').querySelector('.dw-rail-closure')).not.toBeNull();
    expect(gutter('data-row-sh-2-2').querySelector('.dw-rail-line-dashed')).not.toBeNull();
    expect(gutter('data-row-sh-1-1').querySelector('.dw-rail-line-solid')).not.toBeNull();
    expect(gutter('data-row-sec-2').querySelector('.dw-rail-elbow')).not.toBeNull();
  });

  it('hides the rail while sorted', () => {
    useDataWorkspaceStore.setState({ sort: { colKey: 'length', dir: 'asc' } });
    render(<DataWorkspace />);
    expect(document.querySelector('.dw-rail')).toBeNull();
  });

  it('does not load the persisted view itself (SingleSystemView does, in every mode)', () => {
    render(<DataWorkspace />);
    expect(useDataWorkspaceStore.getState().viewLoaded).toBe(false);
  });

  it('shows the counts in the status bar', () => {
    render(<DataWorkspace />);
    expect(screen.getByTestId('data-status-counts')).toHaveTextContent('21 rows · 12 stations · 9 shots · 3 sections');
    expect(screen.queryByTestId('data-status-filtered')).toBeNull();
  });

  it('collapsing a section hides its rows and the status bar says so', () => {
    render(<DataWorkspace />);
    act(() => useDataWorkspaceStore.getState().toggleSectionCollapsed(2));
    expect(screen.queryByTestId('data-row-st-2-1')).toBeNull();
    expect(screen.getByTestId('data-status-collapsed')).toHaveTextContent('1 section collapsed');
    expect(screen.queryByTestId('data-status-filtered')).toBeNull();
  });
});

describe('Find in row names', () => {
  const nameOf = (part: string) => screen.getByTestId(['data', 'row', 'name', part].join('-'));
  const find = (query: string) =>
    useDataWorkspaceStore.setState({ search: { ...useDataWorkspaceStore.getState().search, open: true, query } });

  it('marks a match inside the shown ID like a cell match', () => {
    find('3:1');
    render(<DataWorkspace />);
    const marks = nameOf('st-3-1').querySelectorAll('mark.sg-mark');
    expect([...marks].map((m) => m.textContent)).toEqual(['3:1']);
    expect(nameOf('st-1-1').querySelector('mark, .dw-name-match')).toBeNull();
  });

  it('highlights the whole ID when the match is in the full section name', () => {
    find('downstream');
    render(<DataWorkspace />);
    const hit = nameOf('sh-3-2').querySelector('.dw-name-match');
    expect(hit).not.toBeNull();
    expect(hit).toHaveTextContent('3:2');
    // The first match is the current one: the strong look.
    expect(nameOf('st-3-0').querySelector('.dw-name-match-current')).not.toBeNull();
    expect(nameOf('sh-3-2').querySelector('.dw-name-match-current')).toBeNull();
    // Rows of other sections do not match.
    expect(nameOf('st-1-1').querySelector('mark, .dw-name-match')).toBeNull();
  });
});

describe('DataWorkspace empty state', () => {
  const narrowTo = (query: string, rowScope: 'visible' | 'all' = 'visible') =>
    useDataWorkspaceStore.setState({
      search: { ...useDataWorkspaceStore.getState().search, open: true, query, onlyMatchingRows: true, rowScope },
    });

  it('says when "Show only rows with matches" empties the view and offers Show all rows', () => {
    narrowTo('zzzz');
    render(<DataWorkspace />);
    expect(screen.getByTestId('data-empty-message')).toHaveTextContent('No rows contain “zzzz”.');
    expect(screen.queryByTestId('data-empty-clear-filters')).toBeNull();
    fireEvent.click(screen.getByTestId('data-empty-show-all-rows'));
    expect(useDataWorkspaceStore.getState().search.onlyMatchingRows).toBe(false);
    expect(screen.queryByTestId('data-empty')).toBeNull();
  });

  it('offers both when the filters and the search narrowing contribute', () => {
    useDataWorkspaceStore.getState().setFilters({ sectionIds: [1] });
    narrowTo('zzzz');
    render(<DataWorkspace />);
    expect(screen.getByTestId('data-empty-message')).toHaveTextContent('No rows the filters show contain “zzzz”.');
    expect(screen.getByTestId('data-empty-show-all-rows')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('data-empty-clear-filters'));
    expect(useDataWorkspaceStore.getState().filters.sectionIds).toBeNull();
  });

  it('searching all rows, the filters do not matter: only Show all rows', () => {
    useDataWorkspaceStore.getState().setFilters({ sectionIds: [1] });
    narrowTo('zzzz', 'all');
    render(<DataWorkspace />);
    expect(screen.getByTestId('data-empty-message')).toHaveTextContent('No rows contain “zzzz”.');
    expect(screen.queryByTestId('data-empty-clear-filters')).toBeNull();
  });

  it('filters that let nothing through: only Clear filters', () => {
    useDataWorkspaceStore.getState().setFilters({ sectionIds: [] });
    narrowTo('zzzz');
    render(<DataWorkspace />);
    expect(screen.getByTestId('data-empty-message')).toHaveTextContent('No rows match the current filters.');
    expect(screen.queryByTestId('data-empty-show-all-rows')).toBeNull();
    expect(screen.getByTestId('data-empty-clear-filters')).toBeInTheDocument();
  });
});

describe('DataWorkspace keyboard focus', () => {
  const grid = () => document.querySelector<HTMLElement>('[role="grid"]')!;

  it('puts the keyboard in the grid on entry', () => {
    render(<DataWorkspace />);
    expect(grid()).toContainElement(document.activeElement as HTMLElement);
  });

  it('never pulls the keyboard out of the find box when rows come back', () => {
    render(<DataWorkspace />);
    act(() => useDataWorkspaceStore.getState().setSearch({ open: true }));
    const input = screen.getByTestId('data-find-input');
    input.focus();
    fireEvent.change(input, { target: { value: 'zzzz' } });
    act(() => useDataWorkspaceStore.getState().setSearch({ query: 'zzzz', onlyMatchingRows: true }));
    expect(screen.getByTestId('data-empty')).toBeInTheDocument();
    act(() => useDataWorkspaceStore.getState().setSearch({ query: 'squeeze' }));
    expect(screen.queryByTestId('data-empty')).toBeNull();
    expect(document.activeElement).toBe(input);
  });

  it('does not take the keyboard from the find box on entry either', () => {
    useDataWorkspaceStore.setState({ search: { ...useDataWorkspaceStore.getState().search, open: true, query: 'zzzz', onlyMatchingRows: true } });
    render(<DataWorkspace />);
    const input = screen.getByTestId('data-find-input');
    input.focus();
    act(() => useDataWorkspaceStore.getState().setSearch({ query: 'squeeze' }));
    expect(document.activeElement).toBe(input);
  });
});

describe('DataSectionHeader', () => {
  it('shows the counts and length, toggles collapse and jumps to the parent', () => {
    const nav = { goToRow: vi.fn() } as unknown as DataNav;
    const ctx = makeCtx();
    render(
      <DataSectionHeader sectionId={2} ctx={ctx} stats={sectionStatsOf(ctx.data).get(2)} collapsed={false} nav={nav} />,
    );
    expect(screen.getByTestId('data-section-header-2')).toHaveTextContent('Branch B');
    expect(screen.getByTestId('data-section-counts-2')).toHaveTextContent('3 stations · 2 shots · 10.50 m');
    fireEvent.click(screen.getByTestId('data-section-toggle-2'));
    expect(useDataWorkspaceStore.getState().collapsedSections).toEqual([2]);
    expect(within(screen.getByTestId('data-section-continues-2')).getByText('Main:2')).toHaveClass('font-semibold');
    fireEvent.click(screen.getByTestId('data-section-continues-2'));
    expect(nav.goToRow).toHaveBeenCalledWith('st:1:2');
  });
});

describe('DataWorkspace cells', () => {
  it('shows the issue icons and frozen lock at the end of the gutter name, and the J/R glyph', () => {
    render(<DataWorkspace />);
    expect(screen.getByTestId('data-issues-sh-3-3')).toHaveClass('text-error');
    expect(screen.getByTestId('data-issues-sh-3-2')).toHaveClass('text-warning');
    expect(screen.getByTestId('data-issues-sh-3-1')).toHaveClass('text-text-secondary');
    expect(screen.queryByTestId('data-issues-sh-1-1')).toBeNull();
    // In the gutter, after the "Show on map" pin; no Issues column any more.
    const gutter = screen.getByTestId('data-row-sh-3-3');
    expect(gutter.lastElementChild!.lastElementChild).toBe(screen.getByTestId('data-issues-sh-3-3'));
    expect(screen.getByTestId('data-issues-sh-3-3').previousElementSibling).toBe(screen.getByTestId('data-row-map-sh-3-3'));
    expect(screen.queryByTestId('data-grid-header-issues')).toBeNull();
    expect(screen.getByTestId('data-kind-sh-2-2')).toHaveTextContent('J');
  });

  it('a click on a shot issue opens Survey with the resolver, as the old column did', () => {
    const resolve = vi.spyOn(useUIStore.getState(), 'triggerResolveWarning');
    render(<DataWorkspace />);
    fireEvent.click(screen.getByTestId('data-issues-sh-3-3'));
    expect(useUIStore.getState().appMode).toBe('survey');
    expect(useUIStore.getState().selection).toEqual({ type: 'shot', sectionId: 3, shotId: 303 });
    expect(resolve).toHaveBeenCalledWith(3, 303);
    resolve.mockRestore();
  });

  it('a find match in the issue text outlines the icon', () => {
    useDataWorkspaceStore.setState({ search: { ...useDataWorkspaceStore.getState().search, open: true, query: 'depth jump' } });
    render(<DataWorkspace />);
    expect(screen.getByTestId('data-issues-sh-3-3')).toHaveClass('dw-match-outline', 'dw-match-outline-current');
    expect(screen.getByTestId('data-issues-sh-3-2')).not.toHaveClass('dw-match-outline');
  });

  it('cycles the shot kind on click', () => {
    render(<DataWorkspace />);
    fireEvent.click(screen.getByTestId('data-kind-sh-2-1'));
    expect(actions.setShotKind).toHaveBeenCalledWith(2, 201, 'Jump');
  });
});

describe('DataWorkspace hover card', () => {
  it('opens after a delay with merges, closures and actions', () => {
    vi.useFakeTimers();
    render(<DataWorkspace />);
    fireEvent.mouseEnter(screen.getByTestId('data-row-name-st-1-2').parentElement!);
    expect(screen.queryByTestId('data-hover-card-st-1-2')).toBeNull();
    act(() => {
      vi.advanceTimersByTime(400);
    });
    const card = screen.getByTestId('data-hover-card-st-1-2');
    // The title names the section; the ID row adds the chips' "Label:index".
    expect(card).toHaveTextContent('Main');
    expect(card).toHaveTextContent('1:2 (Main:2)');
    // The linked station's name is bold, as in the chips.
    expect(within(screen.getByTestId('data-hover-merges-st-1-2')).getByText('Branch B:0')).toHaveClass('font-semibold');
    // The partner row is highlighted while the junction is hovered.
    expect(screen.getByTestId('data-grid-row-11')).toHaveClass('dw-row-partner');

    fireEvent.click(screen.getByTestId('data-hover-show-on-map-st-1-2'));
    expect(useDataWorkspaceStore.getState().mapPaneOpen).toBe(true);
    expect(useUIStore.getState().selection).toEqual({ type: 'station', stationRef: { section_id: 1, station_index: 2 } });
    expect(useUIStore.getState().requestCenterOnStation).toEqual({ section_id: 1, station_index: 2 });
  });

  const trigger = (part: string) => screen.getByTestId(`data-row-name-${part}`).parentElement!;
  const wait = (ms = 400) =>
    act(() => {
      vi.advanceTimersByTime(ms);
    });

  it('switches to the next row name the pointer moves to', () => {
    vi.useFakeTimers();
    render(<DataWorkspace />);
    fireEvent.mouseEnter(trigger('st-1-2'));
    wait();
    expect(screen.getByTestId('data-hover-card-st-1-2')).toBeInTheDocument();
    for (const [from, to] of [
      ['st-1-2', 'sh-1-3'],
      ['sh-1-3', 'st-1-3'],
      ['st-1-3', 'st-1-1'],
    ] as const) {
      fireEvent.mouseLeave(trigger(from));
      fireEvent.mouseEnter(trigger(to));
      wait();
      expect(screen.queryByTestId(`data-hover-card-${from}`)).toBeNull();
      expect(screen.getByTestId(`data-hover-card-${to}`)).toBeInTheDocument();
      expect(document.querySelectorAll('.dw-hover-card')).toHaveLength(1);
    }
  });

  it('closes on a press in the grid and when the context menu opens, until hovered again', () => {
    vi.useFakeTimers();
    render(<DataWorkspace />);
    fireEvent.mouseEnter(trigger('st-1-2'));
    wait();
    fireEvent.mouseDown(screen.getByTestId('data-cell-sh-1-3-length'));
    expect(screen.queryByTestId('data-hover-card-st-1-2')).toBeNull();

    fireEvent.mouseLeave(trigger('st-1-2'));
    wait();
    fireEvent.mouseEnter(trigger('st-1-2'));
    wait();
    expect(screen.getByTestId('data-hover-card-st-1-2')).toBeInTheDocument();
    fireEvent.contextMenu(screen.getByTestId('data-row-name-st-1-2'));
    expect(screen.getByTestId('data-grid-context-menu')).toBeInTheDocument();
    expect(screen.queryByTestId('data-hover-card-st-1-2')).toBeNull();
  });

  it('a right-click before the open delay ran out cancels the pending card', () => {
    vi.useFakeTimers();
    render(<DataWorkspace />);
    fireEvent.mouseEnter(trigger('st-1-2'));
    wait(100);
    fireEvent.contextMenu(screen.getByTestId('data-row-name-st-1-2'));
    wait();
    expect(screen.getByTestId('data-grid-context-menu')).toBeInTheDocument();
    expect(screen.queryByTestId('data-hover-card-st-1-2')).toBeNull();
  });

  it('keeps the card open for a press inside it', () => {
    vi.useFakeTimers();
    render(<DataWorkspace />);
    fireEvent.mouseEnter(trigger('st-1-2'));
    wait();
    fireEvent.mouseDown(screen.getByTestId('data-hover-show-on-map-st-1-2'));
    expect(screen.getByTestId('data-hover-card-st-1-2')).toBeInTheDocument();
  });

  it('lists a shot warnings, the ignored one marked', () => {
    vi.useFakeTimers();
    render(<DataWorkspace />);
    fireEvent.mouseEnter(screen.getByTestId('data-row-name-sh-3-3').parentElement!);
    act(() => {
      vi.advanceTimersByTime(400);
    });
    const list = screen.getByTestId('data-hover-warnings-sh-3-3');
    expect(list).toHaveTextContent('Depth jump too large');
    expect(list).toHaveTextContent('Long shot (ignored)');
  });
});

describe('DataWorkspace editing', () => {
  const typeInto = (testId: string, draft: string) => {
    fireEvent.doubleClick(screen.getByTestId(testId));
    const editor = screen.getByTestId('data-grid-cell-editor');
    fireEvent.change(editor, { target: { value: draft } });
    fireEvent.keyDown(editor, { key: 'Enter' });
  };

  it('commits a shot length as one update_shot', () => {
    render(<DataWorkspace />);
    typeInto('data-cell-sh-1-1-length', '9.5');
    expect(actions.updateShot).toHaveBeenCalledWith(1, 101, { length: 9.5 });
  });

  it('commits a station comment through updateStationComment', () => {
    render(<DataWorkspace />);
    typeInto('data-cell-st-1-3-comment', 'Dome');
    expect(actions.updateStationComment).toHaveBeenCalledWith({ section_id: 1, station_index: 3 }, 'Dome');
  });

  it('commits a single station name through updateStationName, not a batch', () => {
    useDataWorkspaceStore.setState({ hiddenColumns: [] });
    render(<DataWorkspace />);
    typeInto('data-cell-st-1-3-name', 'B7');
    expect(actions.updateStationName).toHaveBeenCalledWith({ section_id: 1, station_index: 3 }, 'B7');
    expect(actions.executeBatch).not.toHaveBeenCalled();
  });

  const pasteAt = (testId: string, text: string) => {
    fireEvent.mouseDown(screen.getByTestId(testId));
    fireEvent.mouseUp(window);
    fireEvent.paste(document.querySelector('[role="grid"]')!, { clipboardData: { getData: () => text } });
  };

  it('pastes an external column into consecutive shots as ONE batch, past the station rows', () => {
    render(<DataWorkspace />);
    // Copied from a spreadsheet: one value per shot, no blank lines for the
    // station rows interleaved between them.
    pasteAt('data-cell-sh-1-1-length', '3\n4\n5');
    expect(actions.executeBatch).toHaveBeenCalledTimes(1);
    expect(actions.executeBatch.mock.calls[0]![0]).toEqual([
      { type: 'update_shot', section_id: 1, shot_id: 101, updates: { length: 3 } },
      { type: 'update_shot', section_id: 1, shot_id: 102, updates: { length: 4 } },
      { type: 'update_shot', section_id: 1, shot_id: 103, updates: { length: 5 } },
    ]);
  });

  it('keeps a Comment paste positional: stations and shots both take a line', () => {
    render(<DataWorkspace />);
    pasteAt('data-cell-sh-1-1-comment', 'a\nb\nc');
    expect(actions.executeBatch).toHaveBeenCalledTimes(1);
    expect(actions.executeBatch.mock.calls[0]![0]).toEqual([
      { type: 'update_shot', section_id: 1, shot_id: 101, updates: { comment: 'a' } },
      { type: 'set_station_comment', station_ref: { section_id: 1, station_index: 1 }, comment: 'b' },
      { type: 'update_shot', section_id: 1, shot_id: 102, updates: { comment: 'c' } },
    ]);
  });

  it("pastes the grid's own copy 1:1, its station gaps included", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal('navigator', { ...navigator, clipboard: { writeText, readText: vi.fn().mockRejectedValue(new Error('denied')) } });
    render(<DataWorkspace />);
    fireEvent.mouseDown(screen.getByTestId('data-cell-sh-1-1-length'));
    fireEvent.mouseUp(window);
    fireEvent.mouseDown(screen.getByTestId('data-cell-sh-1-2-length'), { shiftKey: true });
    fireEvent.mouseUp(window);
    const grid = document.querySelector('[role="grid"]')!;
    await act(async () => {
      fireEvent.keyDown(grid, { key: 'c', ctrlKey: true });
    });
    const copied = writeText.mock.calls[0]![0] as string;
    expect(copied.split('\n')).toHaveLength(3); // shot 1, station 1 (blank), shot 2
    pasteAt('data-cell-sh-1-3-length', copied);
    expect(actions.executeBatch.mock.calls[0]![0]).toEqual([
      { type: 'update_shot', section_id: 1, shot_id: 103, updates: { length: 8.25 } },
      { type: 'update_shot', section_id: 1, shot_id: 104, updates: { length: 6.5 } },
    ]);
  });

  it('never edits while blocked, and says why', () => {
    blocked.value = true;
    render(<DataWorkspace />);
    expect(screen.getByTestId('data-readonly-banner')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('data-kind-sh-2-1'));
    expect(actions.setShotKind).not.toHaveBeenCalled();
  });
});

describe('DataWorkspace context menu and shortcuts', () => {
  it('offers Show on map / Open in Survey / Copy / Filter / Collapse', () => {
    render(<DataWorkspace />);
    fireEvent.contextMenu(screen.getByTestId('data-cell-sh-1-2-length'));
    fireEvent.click(screen.getByText('Filter to this section'));
    expect(useDataWorkspaceStore.getState().filters.sectionIds).toEqual([1]);

    fireEvent.contextMenu(screen.getByTestId('data-cell-sh-1-2-length'));
    fireEvent.click(screen.getByText('Open in Survey'));
    expect(useUIStore.getState().appMode).toBe('survey');
    expect(useDataWorkspaceStore.getState().returnToData).toEqual({ rowKey: 'sh:1:102' });
    expect(useUIStore.getState().selection).toEqual({ type: 'shot', sectionId: 1, shotId: 102 });
  });

  it('offers Collapse section only where it works (survey order, headers on)', () => {
    const { unmount } = render(<DataWorkspace />);
    fireEvent.contextMenu(screen.getByTestId('data-cell-sh-1-2-length'));
    expect(screen.getByTestId('data-ctx-toggle-section')).toHaveTextContent('Collapse section');
    unmount();

    useDataWorkspaceStore.setState({ sort: { colKey: 'length', dir: 'asc' } });
    const sorted = render(<DataWorkspace />);
    fireEvent.contextMenu(screen.getByTestId('data-cell-sh-1-2-length'));
    expect(screen.getByTestId('data-ctx-show-on-map')).toBeInTheDocument();
    expect(screen.queryByTestId('data-ctx-toggle-section')).toBeNull();
    sorted.unmount();

    useDataWorkspaceStore.setState({ sort: null, showSectionHeaders: false });
    render(<DataWorkspace />);
    fireEvent.contextMenu(screen.getByTestId('data-cell-sh-1-2-length'));
    expect(screen.getByTestId('data-ctx-show-on-map')).toBeInTheDocument();
    expect(screen.queryByTestId('data-ctx-toggle-section')).toBeNull();
    expect(useDataWorkspaceStore.getState().collapsedSections).toEqual([]);
  });

  it('Alt+M shows the active row on the map', () => {
    render(<DataWorkspace />);
    fireEvent.mouseDown(screen.getByTestId('data-cell-sh-1-3-length'));
    fireEvent.keyDown(screen.getByTestId('data-workspace'), { key: 'm', code: 'KeyM', altKey: true });
    expect(useDataWorkspaceStore.getState().mapPaneOpen).toBe(true);
    expect(useUIStore.getState().requestCenterOnShot).toEqual({ sectionId: 1, shotId: 103 });
  });

  it('header menu sorts, hides and adds a filter condition', () => {
    render(<DataWorkspace />);
    fireEvent.contextMenu(screen.getByTestId('data-grid-header-length'));
    fireEvent.click(screen.getByTestId('data-header-menu-sort-desc'));
    expect(useDataWorkspaceStore.getState().sort).toEqual({ colKey: 'length', dir: 'desc' });
    fireEvent.contextMenu(screen.getByTestId('data-grid-header-length'));
    fireEvent.click(screen.getByTestId('data-header-menu-filter'));
    expect(useDataWorkspaceStore.getState().filters.conditions).toMatchObject([{ colKey: 'length', op: 'gte', value: '' }]);
    fireEvent.contextMenu(screen.getByTestId('data-grid-header-length'));
    fireEvent.click(screen.getByTestId('data-header-menu-hide'));
    expect(useDataWorkspaceStore.getState().hiddenColumns).toContain('length');
  });
});

describe('Header menu on a search-revealed column', () => {
  const openFindRevealingComment = (columnScope: 'all' | 'custom', columns: ('comment' | 'length')[] = []) =>
    useDataWorkspaceStore.setState({
      hiddenColumns: ['comment'],
      search: { ...useDataWorkspaceStore.getState().search, open: true, query: 'squeeze', columnScope, columns },
    });

  it('offers "Hide search-revealed columns" (scope back to visible) instead of a no-op Hide', () => {
    openFindRevealingComment('all');
    render(<DataWorkspace />);
    fireEvent.contextMenu(screen.getByTestId('data-grid-header-comment'));
    expect(screen.queryByTestId('data-header-menu-hide')).toBeNull();
    const item = screen.getByTestId('data-header-menu-hide-revealed');
    expect(item).toHaveTextContent('Hide search-revealed columns');
    expect(item.getAttribute('title')).toMatch(/searches all columns/);
    fireEvent.click(item);
    expect(useDataWorkspaceStore.getState().search.columnScope).toBe('visible');
    expect(screen.queryByTestId('data-grid-header-comment')).toBeNull();
  });

  it('takes the column out of a custom search scope', () => {
    openFindRevealingComment('custom', ['comment', 'length']);
    render(<DataWorkspace />);
    fireEvent.contextMenu(screen.getByTestId('data-grid-header-comment'));
    fireEvent.click(screen.getByTestId('data-header-menu-hide-revealed'));
    expect(useDataWorkspaceStore.getState().search).toMatchObject({ columnScope: 'custom', columns: ['length'] });
    expect(screen.queryByTestId('data-grid-header-comment')).toBeNull();
  });
});

describe('Focus requests', () => {
  it('a first entry starts on station 0 of the first section, never on its header band', () => {
    render(<DataWorkspace />);
    expect(useDataWorkspaceStore.getState().activeRowKey).toBe('st:1:0');
    // In the first shown column that applies to a station row (Comment: Depth
    // is hidden by default), never a blank shot cell.
    expect(document.querySelector('[data-active-cell="true"]')).toHaveAttribute('data-testid', 'data-grid-cell-1-comment');
    expect(screen.getByTestId('data-grid-selection-overlay')).not.toHaveAttribute('data-band-focus');
  });

  it("a header row's gutter is the rail only (the header content names the section); its outline spans the row", () => {
    render(<DataWorkspace />);
    const gutter = screen.getByTestId('data-grid-gutter-10');
    expect(within(gutter).queryByTestId(['data', 'row', 'name', 'sec-2'].join('-'))).toBeNull();
    expect(gutter.querySelector('.dw-rail-elbow')).not.toBeNull();
    // The header content starts in the gutter, right after the rail.
    const content = screen.getByTestId('data-grid-group-content-10');
    expect(gutter.contains(content)).toBe(true);
    expect(within(content).getByTestId('data-section-header-2')).toHaveTextContent('Branch B');
    fireEvent.mouseDown(screen.getByTestId('data-grid-group-10'));
    fireEvent.mouseUp(window);
    expect(gutter).toHaveClass('sg-gutter-active');
    // Arrow onto the band from the row above: one cell selected, the row outlined.
    fireEvent.mouseDown(screen.getByTestId('data-grid-cell-9-length'));
    fireEvent.mouseUp(window);
    fireEvent.keyDown(document.querySelector('.sg-grid [tabindex="0"]')!, { key: 'ArrowDown' });
    expect(screen.getByTestId('data-grid-selection-overlay')).toHaveAttribute('data-band-focus', 'true');
  });

  it('a pending focus request beats the remembered active row on entry', () => {
    useDataWorkspaceStore.setState({ activeRowKey: 'st:1:1' });
    useDataWorkspaceStore.getState().requestFocus({ rowKey: 'st:3:2', align: 'center' });
    render(<DataWorkspace />);
    expect(useDataWorkspaceStore.getState().activeRowKey).toBe('st:3:2');
  });

  it('does not replay an old request when Data is entered again', () => {
    useDataWorkspaceStore.getState().requestFocus({ rowKey: 'st:3:2' });
    const first = render(<DataWorkspace />);
    expect(useDataWorkspaceStore.getState().activeRowKey).toBe('st:3:2');
    act(() => useDataWorkspaceStore.getState().setActiveRowKey('st:1:1'));
    first.unmount();
    render(<DataWorkspace />);
    expect(useDataWorkspaceStore.getState().activeRowKey).toBe('st:1:1');
  });

  it('lands a section header request on the section first row while sorted, never reveals a header', () => {
    useDataWorkspaceStore.setState({ sort: { colKey: 'length', dir: 'asc' } });
    useDataWorkspaceStore.getState().requestFocus({ rowKey: 'sec:2' });
    render(<DataWorkspace />);
    const firstOfSection = rowIds().find((id) => /^data-row-(st|sh)-2-/.test(id));
    expect(firstOfSection).toBeDefined();
    expect(useDataWorkspaceStore.getState().activeRowKey).toMatch(/^(st|sh):2:/);
    expect(useDataWorkspaceStore.getState().revealRowKeys).toEqual([]);
  });

  it('lands a section header request on station 0 with headers off', () => {
    useDataWorkspaceStore.setState({ showSectionHeaders: false });
    useDataWorkspaceStore.getState().requestFocus({ rowKey: 'sec:2' });
    render(<DataWorkspace />);
    expect(useDataWorkspaceStore.getState().activeRowKey).toBe('st:2:0');
  });

  it('drops a section header request when no row of the section passes the filters', () => {
    useDataWorkspaceStore.setState({ activeRowKey: 'st:1:1' });
    useDataWorkspaceStore.getState().setFilters({ sectionIds: [1] });
    useDataWorkspaceStore.getState().requestFocus({ rowKey: 'sec:3' });
    render(<DataWorkspace />);
    expect(useDataWorkspaceStore.getState().revealRowKeys).toEqual([]);
    expect(useDataWorkspaceStore.getState().activeRowKey).toBe('st:1:1');
  });

  it('reveals a requested row the filters hide, and drops a request for a gone row', () => {
    useDataWorkspaceStore.setState({ collapsedSections: [3] });
    useDataWorkspaceStore.getState().requestFocus({ rowKey: 'sh:3:302' });
    render(<DataWorkspace />);
    expect(useDataWorkspaceStore.getState().revealRowKeys).toEqual(['sh:3:302']);
    act(() => useDataWorkspaceStore.getState().requestFocus({ rowKey: 'st:99:0' }));
    expect(useDataWorkspaceStore.getState().revealRowKeys).toEqual(['sh:3:302']);
  });
});

describe('Map follow and map selection', () => {
  it('never echoes a map selection back (multi-selection survives, no pan)', () => {
    vi.useFakeTimers();
    useDataWorkspaceStore.setState({ mapPaneOpen: true, mapFollow: true });
    render(<DataWorkspace />);
    const a = { type: 'shot' as const, sectionId: 1, shotId: 103 };
    const b = { type: 'shot' as const, sectionId: 1, shotId: 104 };
    act(() => useUIStore.setState({ selection: a, multiSelection: [a, b] }));
    act(() => {
      vi.advanceTimersByTime(500);
    });
    expect(useDataWorkspaceStore.getState().activeRowKey).toBe('sh:1:103');
    expect(useUIStore.getState().multiSelection).toHaveLength(2);
    expect(useUIStore.getState().requestPanToShot).toBeNull();
  });

  it('follows the active row: selects it on the map and pans, after a short throttle', () => {
    vi.useFakeTimers();
    useDataWorkspaceStore.setState({ mapPaneOpen: true, mapFollow: true });
    useUIStore.setState({ requestPanToShot: null });
    render(<DataWorkspace />);
    fireEvent.mouseDown(screen.getByTestId('data-cell-sh-1-2-length'));
    act(() => {
      vi.advanceTimersByTime(500);
    });
    expect(useUIStore.getState().selection).toEqual({ type: 'shot', sectionId: 1, shotId: 102 });
    expect(useUIStore.getState().requestPanToShot).toEqual({ sectionId: 1, shotId: 102 });
  });

  it('opening the map pane pans to the row the user is already on', () => {
    vi.useFakeTimers();
    useDataWorkspaceStore.setState({ mapPaneOpen: false, mapFollow: true });
    useUIStore.setState({ requestPanToShot: null });
    render(<DataWorkspace />);
    fireEvent.mouseDown(screen.getByTestId('data-cell-sh-1-2-length'));
    act(() => {
      vi.advanceTimersByTime(500);
    });
    // Pane closed: nothing followed yet.
    expect(useUIStore.getState().requestPanToShot).toBeNull();
    act(() => useDataWorkspaceStore.getState().setMapPaneOpen(true));
    act(() => {
      vi.advanceTimersByTime(500);
    });
    expect(useUIStore.getState().requestPanToShot).toEqual({ sectionId: 1, shotId: 102 });
  });

  const armFollow = () => {
    vi.useFakeTimers();
    useDataWorkspaceStore.setState({ mapPaneOpen: false, mapFollow: true });
    useUIStore.setState({ requestPanToShot: null });
    render(<DataWorkspace />);
    fireEvent.mouseDown(screen.getByTestId('data-cell-sh-1-2-length'));
    // Opening the pane arms the follow for once it has settled.
    act(() => useDataWorkspaceStore.getState().setMapPaneOpen(true));
    act(() => {
      vi.advanceTimersByTime(100);
    });
  };
  const settle = () =>
    act(() => {
      vi.advanceTimersByTime(1000);
    });

  it('leaving Data inside the settle window selects and pans nothing', () => {
    armFollow();
    act(() => useUIStore.setState({ appMode: 'survey' }));
    settle();
    expect(useUIStore.getState().selection).toBeNull();
    expect(useUIStore.getState().requestPanToShot).toBeNull();
  });

  it('a project reset inside the settle window drops the pending follow', () => {
    // The hook alone: in the shell the grid at once re-picks the new project's
    // first row (a fresh, legitimate follow). What must not survive is the OLD
    // row's follow, which a next project with the same ids would also match.
    vi.useFakeTimers();
    useDataWorkspaceStore.setState({ mapPaneOpen: false, mapFollow: true, activeRowKey: 'sh:1:102' });
    useUIStore.setState({ requestPanToShot: null });
    renderHook(() => useDataNavigation(useDataWorkspaceModel(), null, () => undefined));
    act(() => useDataWorkspaceStore.getState().setMapPaneOpen(true));
    act(() => {
      vi.advanceTimersByTime(100);
    });
    act(() => useDataWorkspaceStore.getState().resetForProject());
    settle();
    expect(useUIStore.getState().selection).toBeNull();
    expect(useUIStore.getState().requestPanToShot).toBeNull();
  });

  it('closing the pane, or unmounting, inside the settle window drops it too', () => {
    armFollow();
    act(() => useDataWorkspaceStore.getState().setMapPaneOpen(false));
    settle();
    expect(useUIStore.getState().requestPanToShot).toBeNull();
    cleanup();
    armFollow();
    cleanup();
    settle();
    expect(useUIStore.getState().selection).toBeNull();
    expect(useUIStore.getState().requestPanToShot).toBeNull();
  });

  it('"Show on map" on another row keeps that row selected once the pane settles', () => {
    vi.useFakeTimers();
    useDataWorkspaceStore.setState({ mapPaneOpen: false, mapFollow: true });
    render(<DataWorkspace />);
    fireEvent.mouseDown(screen.getByTestId('data-cell-sh-1-2-length'));
    act(() => {
      vi.advanceTimersByTime(500);
    });
    fireEvent.click(screen.getByTestId('data-row-map-st-1-3'));
    act(() => {
      vi.advanceTimersByTime(1000);
    });
    expect(useDataWorkspaceStore.getState().mapPaneOpen).toBe(true);
    expect(useDataWorkspaceStore.getState().activeRowKey).toBe('st:1:3');
    expect(useUIStore.getState().selection).toEqual({ type: 'station', stationRef: { section_id: 1, station_index: 3 } });
  });

  it('the status bar names collapsed sections instead of reporting "Showing 0"', () => {
    render(<DataWorkspace />);
    expect(screen.queryByTestId('data-status-collapsed')).toBeNull();
    const ids = useDataWorkspaceStore.getState();
    act(() => ids.setCollapsedSections([1, 2, 3]));
    expect(screen.getByTestId('data-status-collapsed')).toHaveTextContent('3 sections collapsed');
    // Collapse is not a filter: no "Showing X of Y".
    expect(screen.queryByTestId('data-status-filtered')).toBeNull();
  });

  it('Open in Survey leaves no focus request behind (hover card and map pane header)', () => {
    vi.useFakeTimers();
    render(<DataWorkspace />);
    fireEvent.mouseDown(screen.getByTestId('data-cell-sh-1-3-length'));
    fireEvent.mouseEnter(screen.getByTestId('data-row-name-st-1-2').parentElement!);
    act(() => {
      vi.advanceTimersByTime(400);
    });
    fireEvent.click(screen.getByTestId('data-hover-open-in-survey-st-1-2'));
    expect(useUIStore.getState().appMode).toBe('survey');
    expect(useUIStore.getState().selection).toEqual({ type: 'station', stationRef: { section_id: 1, station_index: 2 } });
    expect(useDataWorkspaceStore.getState().focusRequest).toBeNull();

    // The map pane header's path (store-only, no guard of its own).
    act(() => useUIStore.setState({ appMode: 'data' }));
    act(() => {
      openRowInSurvey('st:3:2');
    });
    expect(useUIStore.getState().selection).toEqual({ type: 'station', stationRef: { section_id: 3, station_index: 2 } });
    expect(useDataWorkspaceStore.getState().focusRequest).toBeNull();
  });

  it('focuses the row of a selection made on the map', () => {
    render(<DataWorkspace />);
    act(() => useUIStore.getState().setSelection({ type: 'station', stationRef: { section_id: 3, station_index: 2 } }));
    expect(useDataWorkspaceStore.getState().focusRequest).toMatchObject({ rowKey: 'st:3:2' });
  });
});

describe('Back to Data, map pane header and Open in Data', () => {
  it('returns to Data at the row and clears itself', () => {
    useUIStore.setState({ appMode: 'survey' });
    useDataWorkspaceStore.setState({ returnToData: { rowKey: 'sh:1:102' } });
    render(<BackToDataChip />);
    // The row ID the gutter shows, with the section for context.
    expect(screen.getByTestId('back-to-data')).toHaveTextContent('Back to Data· 1:2 (Main)');
    fireEvent.click(screen.getByTestId('back-to-data'));
    expect(useUIStore.getState().appMode).toBe('data');
    expect(useDataWorkspaceStore.getState().focusRequest).toMatchObject({ rowKey: 'sh:1:102' });
    expect(useDataWorkspaceStore.getState().returnToData).toBeNull();
  });

  it('disappears when Survey is left another way', () => {
    useUIStore.setState({ appMode: 'survey' });
    useDataWorkspaceStore.setState({ returnToData: { rowKey: 'st:1:1' } });
    render(<BackToDataChip />);
    act(() => useUIStore.getState().setAppMode('plan'));
    expect(useDataWorkspaceStore.getState().returnToData).toBeNull();
  });

  it('map pane header toggles follow, opens the active row in Survey and closes', () => {
    useDataWorkspaceStore.setState({ mapPaneOpen: true, activeRowKey: 'st:2:1' });
    render(<DataMapPaneHeader />);
    const follow = screen.getByTestId('data-map-pane-follow');
    expect(follow).toHaveAttribute('aria-pressed', 'true');
    fireEvent.click(follow);
    expect(useDataWorkspaceStore.getState().mapFollow).toBe(false);
    expect(screen.getByTestId('data-map-pane-open-in-survey')).toHaveTextContent('2:1 (Branch B)');
    fireEvent.click(screen.getByTestId('data-map-pane-open-in-survey'));
    expect(useUIStore.getState().appMode).toBe('survey');
    expect(useDataWorkspaceStore.getState().returnToData).toEqual({ rowKey: 'st:2:1' });
    fireEvent.click(screen.getByTestId('data-map-pane-close'));
    expect(useDataWorkspaceStore.getState().mapPaneOpen).toBe(false);
  });

  it('Open in Data filters to the section at the selected row', () => {
    useUIStore.setState({ appMode: 'survey', selection: { type: 'shot', sectionId: 3, shotId: 302 } });
    render(<OpenInDataButton sectionId={3} />);
    fireEvent.click(screen.getByTestId('data-table-open-in-data'));
    expect(useDataWorkspaceStore.getState().filters.sectionIds).toEqual([3]);
    expect(useDataWorkspaceStore.getState().focusRequest).toMatchObject({ rowKey: 'sh:3:302' });
    expect(useUIStore.getState().appMode).toBe('data');
  });
});
