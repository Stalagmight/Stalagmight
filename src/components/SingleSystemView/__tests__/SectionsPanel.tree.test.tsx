import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

// Every row mounted: the tree logic, not the virtual window, is under test.
vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () => Array.from({ length: count }, (_, index) => ({ index, key: index, start: index * 40, size: 40 })),
    getTotalSize: () => count * 40,
    scrollToIndex: vi.fn(),
    measureElement: vi.fn(),
  }),
}));
vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn().mockResolvedValue(undefined), getErrorMessage: String }));

import '../../../i18n';
import SectionsPanel from '../SectionsPanel';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import type { Line } from '../../../types/ipc/Line';

const lines = [
  { id: 1, name: 'Main', color: '#111111', cave_section_id: 7, shots: [] },
  { id: 2, name: 'Side', color: '#222222', cave_section_id: 7, shots: [] },
  { id: 3, name: 'Loose', color: '#333333', shots: [] },
] as unknown as Line[];

const ok = { success: true, warnings: [], error: null, line_ids: [] };

function setup() {
  useProjectStore.setState({
    project: {
      created_at: 'p1',
      lines,
      cave_sections: [{ id: 7, name: 'Upper level', color: '#ff0000' }],
    } as never,
    surveyData: null,
    updateCaveSection: vi.fn().mockResolvedValue(ok),
  });
  useUIStore.setState({
    appMode: 'survey',
    geometryMode: 'measured',
    selection: null,
    multiSelection: [],
    hiddenSections: [],
    collapsedSections: {},
    pendingSectionRename: null,
    activeDialog: null,
  });
  // A confirmed-local project: editing is open.
  useSpeleoStore.setState({ projectLinkKind: 'local' } as never);
  render(<SectionsPanel lines={lines} />);
}

const header = () => screen.getByTestId('section-header-7');
const ungrouped = () => screen.getByTestId('sections-ungrouped-header');

describe('SectionsPanel tree', () => {
  beforeEach(setup);

  it('is a tree: headers at level 1 with their lines at level 2', () => {
    expect(screen.getByRole('tree')).toHaveAttribute('aria-multiselectable', 'true');
    expect(header()).toHaveAttribute('aria-level', '1');
    expect(header()).toHaveAttribute('aria-expanded', 'true');
    expect(header()).toHaveAttribute('aria-posinset', '1');
    expect(header()).toHaveAttribute('aria-setsize', '2');
    expect(screen.getByTestId('line-row-1')).toHaveAttribute('aria-level', '2');
    expect(screen.getByTestId('line-row-2')).toHaveAttribute('aria-posinset', '2');
    expect(screen.getByTestId('section-stats-7')).toHaveTextContent('2 lines');
    expect(screen.getByTestId('sections-ungrouped-stats')).toHaveTextContent('1 line');
  });

  it('Left collapses, Right expands then steps in, Left on a line goes to its header', () => {
    act(() => header().focus());
    fireEvent.keyDown(header(), { key: 'ArrowLeft' });
    expect(header()).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByTestId('line-row-1')).toBeNull();
    fireEvent.keyDown(header(), { key: 'ArrowRight' });
    expect(header()).toHaveAttribute('aria-expanded', 'true');
    fireEvent.keyDown(header(), { key: 'ArrowRight' });
    expect(screen.getByTestId('line-row-1')).toHaveFocus();
    fireEvent.keyDown(screen.getByTestId('line-row-1'), { key: 'ArrowLeft' });
    expect(header()).toHaveFocus();
  });

  it('Down / End walk the flattened rows; Space toggles a header', () => {
    act(() => header().focus());
    fireEvent.keyDown(header(), { key: 'ArrowDown' });
    expect(screen.getByTestId('line-row-1')).toHaveFocus();
    fireEvent.keyDown(screen.getByTestId('line-row-1'), { key: 'End' });
    expect(screen.getByTestId('line-row-3')).toHaveFocus();
    fireEvent.keyDown(screen.getByTestId('line-row-3'), { key: 'ArrowUp' });
    expect(ungrouped()).toHaveFocus();
    fireEvent.keyDown(ungrouped(), { key: ' ' });
    expect(ungrouped()).toHaveAttribute('aria-expanded', 'false');
    expect(useUIStore.getState().collapsedSections).toEqual({ p1: [-1] });
  });

  it('Space selects a line; Enter on a header frames all its lines', () => {
    act(() => screen.getByTestId('line-row-2').focus());
    fireEvent.keyDown(screen.getByTestId('line-row-2'), { key: ' ' });
    expect(useUIStore.getState().selection).toEqual({ type: 'line', lineId: 2 });
    fireEvent.keyDown(header(), { key: 'Enter' });
    expect(useUIStore.getState().requestZoomToLine).toEqual([1, 2]);
  });

  it('Shift-click on a header adds its lines to the selection; a plain click only focuses it', () => {
    fireEvent.click(header());
    expect(useUIStore.getState().multiSelection).toEqual([]);
    useUIStore.getState().setSelection({ type: 'line', lineId: 3 });
    fireEvent.click(header(), { shiftKey: true });
    expect(useUIStore.getState().multiSelection.map((s) => s.lineId)).toEqual([3, 1, 2]);
  });

  it('F2 renames inline; Enter commits through update_cave_section', () => {
    act(() => header().focus());
    fireEvent.keyDown(header(), { key: 'F2' });
    const input = screen.getByTestId('section-rename-input-7');
    fireEvent.change(input, { target: { value: '  Lower level ' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(useProjectStore.getState().updateCaveSection).toHaveBeenCalledWith(7, { name: 'Lower level' });
    expect(screen.queryByTestId('section-rename-input-7')).toBeNull();
  });

  it('the eye hides the Section: header and lines read as hidden', () => {
    fireEvent.click(screen.getByTestId('section-toggle-visibility-7'));
    expect(useUIStore.getState().hiddenSections).toEqual([7]);
    expect(header()).toHaveAttribute('data-hidden-on-map', 'true');
    expect(screen.getByTestId('line-row-1')).toHaveAttribute('data-hidden-on-map', 'true');
    expect(screen.getByTestId('line-row-3')).not.toHaveAttribute('data-hidden-on-map');
  });

  it('a selection on a line of a collapsed group opens the group', () => {
    act(() => useUIStore.getState().setSectionCollapsed('p1', 7, true));
    expect(screen.queryByTestId('line-row-2')).toBeNull();
    act(() => useUIStore.getState().setSelection({ type: 'shot', lineId: 2, shotId: 5 }));
    expect(header()).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByTestId('line-row-2')).toHaveAttribute('aria-selected', 'true');
  });
});
