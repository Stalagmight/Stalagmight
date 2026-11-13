/**
 * The Data workspace shell over a synthetic 3-section survey (merge, closure,
 * zero datum, frozen shot, Jump shot, warnings): interleaved rows, gutter
 * names, rail glyphs, link chips, hover card, edit dispatch, context menu
 * actions, the Back to Data chip and Open in Data.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, within } from '@testing-library/react';

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
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { defaultConfig, type Config } from '../../../types/config';
import { makeCtx, makeData, makeProject, WARNINGS } from './dataFixtures';
import { DataSectionHeader } from '../DataSectionHeader';
import { sectionStatsOf } from '../useDataWorkspaceModel';
import type { DataNav } from '../useDataNavigation';

class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

const actions = {
  updateShot: vi.fn(() => Promise.resolve({})),
  updateStationComment: vi.fn(() => Promise.resolve({})),
  executeBatch: vi.fn((_commands: unknown[]) => Promise.resolve({})),
  setShotKind: vi.fn(() => Promise.resolve({})),
};

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(() => Promise.resolve(undefined));
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
  useUIStore.setState({ toasts: [], geometryMode: 'measured', selection: null, appMode: 'data' });
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

  it('names stations "Label:idx" and shots "Label:pos" with from -> to', () => {
    render(<DataWorkspace />);
    expect(screen.getByTestId('data-row-name-st-1-1')).toHaveTextContent('Main:1A1');
    expect(screen.getByTestId('data-row-name-sh-1-2')).toHaveTextContent('Main:21→2');
    expect(screen.getByTestId('data-row-name-st-3-3')).toHaveTextContent('Downstre…sump 1A:3');
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

  it('shows the counts in the status bar', () => {
    render(<DataWorkspace />);
    expect(screen.getByTestId('data-status-counts')).toHaveTextContent('21 rows · 12 stations · 9 shots · 3 sections');
    expect(screen.queryByTestId('data-status-filtered')).toBeNull();
  });

  it('collapsing a section hides its rows and the status bar says so', () => {
    render(<DataWorkspace />);
    act(() => useDataWorkspaceStore.getState().toggleSectionCollapsed(2));
    expect(screen.queryByTestId('data-row-st-2-1')).toBeNull();
    expect(screen.getByTestId('data-status-filtered')).toHaveTextContent('Showing 16 of 21');
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
    fireEvent.click(screen.getByTestId('data-section-continues-2'));
    expect(nav.goToRow).toHaveBeenCalledWith('st:1:2');
  });
});

describe('DataWorkspace cells', () => {
  it('renders link chips that jump to the partner row', () => {
    render(<DataWorkspace />);
    const links = screen.getByTestId('data-links-st-1-4');
    expect(within(links).getByText('Closes to Downstre…sump 1A:3')).toBeInTheDocument();
    expect(screen.getByTestId('data-chip-misclosure-st-1-4-7')).toHaveTextContent('Δ 0.7 m · 1.3 %');
    expect(screen.getByTestId('data-chip-zero-st-1-0')).toHaveTextContent('Zero datum');
    expect(screen.getByTestId('data-chip-branch-st-2-0-1-2')).toHaveTextContent('Continues from Main:2');

    fireEvent.click(screen.getByTestId('data-chip-merge-st-1-2-2-0'));
    expect(useDataWorkspaceStore.getState().focusRequest).toMatchObject({ rowKey: 'st:2:0', align: 'center' });
    expect(useDataWorkspaceStore.getState().revealRowKeys).toEqual([]);
  });

  it('reveals a filtered-out target before focusing it', () => {
    render(<DataWorkspace />);
    act(() => useDataWorkspaceStore.getState().toggleSectionCollapsed(3));
    fireEvent.click(screen.getByTestId('data-chip-closure-st-1-4-7-3-3'));
    expect(useDataWorkspaceStore.getState().revealRowKeys).toEqual(['st:3:3']);
    expect(useDataWorkspaceStore.getState().focusRequest).toMatchObject({ rowKey: 'st:3:3' });
  });

  it('shows the issue icons, frozen lock and the J/R glyph', () => {
    render(<DataWorkspace />);
    expect(screen.getByTestId('data-issues-sh-3-3')).toHaveClass('text-error');
    expect(screen.getByTestId('data-issues-sh-3-2')).toHaveClass('text-warning');
    expect(screen.getByTestId('data-issues-sh-3-1')).toHaveClass('text-text-secondary');
    expect(screen.getByTestId('data-kind-sh-2-2')).toHaveTextContent('J');
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
    expect(card).toHaveTextContent('Main');
    expect(within(screen.getByTestId('data-hover-merges-st-1-2')).getByText('Branch B:0')).toBeInTheDocument();
    // The partner row is highlighted while the junction is hovered.
    expect(screen.getByTestId('data-grid-row-11')).toHaveClass('dw-row-partner');

    fireEvent.click(screen.getByTestId('data-hover-show-on-map-st-1-2'));
    expect(useDataWorkspaceStore.getState().mapPaneOpen).toBe(true);
    expect(useUIStore.getState().selection).toEqual({ type: 'station', stationRef: { section_id: 1, station_index: 2 } });
    expect(useUIStore.getState().requestCenterOnStation).toEqual({ section_id: 1, station_index: 2 });
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

  it('pastes into many cells as ONE batch', () => {
    render(<DataWorkspace />);
    fireEvent.mouseDown(screen.getByTestId('data-cell-sh-1-1-length'));
    const grid = document.querySelector('[role="grid"]')!;
    fireEvent.paste(grid, { clipboardData: { getData: () => '3\n\n4' } });
    expect(actions.executeBatch).toHaveBeenCalledTimes(1);
    expect(actions.executeBatch.mock.calls[0]![0]).toEqual([
      { type: 'update_shot', section_id: 1, shot_id: 101, updates: { length: 3 } },
      { type: 'update_shot', section_id: 1, shot_id: 102, updates: { length: 4 } },
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

describe('Focus requests', () => {
  it('a pending focus request beats the remembered active row on entry', () => {
    useDataWorkspaceStore.setState({ activeRowKey: 'st:1:1' });
    useDataWorkspaceStore.getState().requestFocus({ rowKey: 'st:3:2', align: 'center' });
    render(<DataWorkspace />);
    expect(useDataWorkspaceStore.getState().activeRowKey).toBe('st:3:2');
  });
});

describe('Map follow and map selection', () => {
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
    expect(screen.getByTestId('back-to-data')).toHaveTextContent('Back to Data· Main:2');
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
    expect(screen.getByTestId('data-map-pane-open-in-survey')).toHaveTextContent('Branch B:1');
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
