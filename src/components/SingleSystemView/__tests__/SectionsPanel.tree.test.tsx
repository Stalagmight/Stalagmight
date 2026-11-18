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

function setup(panelLines: Line[] = lines) {
  useProjectStore.setState({
    project: {
      created_at: 'p1',
      lines: panelLines,
      cave_sections: [{ id: 7, name: 'Upper level', color: '#ff0000' }],
    } as never,
    surveyData: null,
    updateCaveSection: vi.fn().mockResolvedValue(ok),
    createCaveSection: vi.fn().mockResolvedValue({ ...ok, cave_section_id: 8 }),
  });
  useUIStore.setState({
    appMode: 'survey',
    geometryMode: 'measured',
    selection: null,
    multiSelection: [],
    hiddenSections: [],
    collapsedSections: {},
    pendingSectionRename: null,
    pendingNewSection: null,
    activeDialog: null,
    requestZoomToLine: null,
    selectionAnnouncement: '',
    toasts: [],
  });
  // A confirmed-local project: editing is open.
  useSpeleoStore.setState({ projectLinkKind: 'local' } as never);
  render(<SectionsPanel lines={panelLines} />);
}

const header = () => screen.getByTestId('section-header-7');
const ungrouped = () => screen.getByTestId('sections-ungrouped-header');

describe('SectionsPanel tree', () => {
  beforeEach(() => setup());

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

  it('Ctrl-click on a header toggles its lines: adds them, then takes them out again', () => {
    useUIStore.getState().setSelection({ type: 'line', lineId: 3 });
    fireEvent.click(header(), { ctrlKey: true });
    expect(useUIStore.getState().multiSelection.map((s) => s.lineId)).toEqual([3, 1, 2]);
    fireEvent.click(header(), { ctrlKey: true });
    expect(useUIStore.getState().multiSelection.map((s) => s.lineId)).toEqual([3]);
    // Partly selected: Ctrl adds the rest (Cmd is the same on macOS).
    act(() => useUIStore.getState().selectAll([{ type: 'line', lineId: 2 }]));
    fireEvent.click(header(), { metaKey: true });
    expect(useUIStore.getState().multiSelection.map((s) => s.lineId)).toEqual([2, 1]);
    // Shift only ever adds.
    fireEvent.click(header(), { shiftKey: true });
    expect(useUIStore.getState().multiSelection.map((s) => s.lineId)).toEqual([2, 1]);
  });

  it('H on a focused header hides its Section, again shows it, and says so', () => {
    act(() => header().focus());
    fireEvent.keyDown(header(), { key: 'h' });
    expect(useUIStore.getState().hiddenSections).toEqual([7]);
    expect(header()).toHaveAttribute('data-hidden-on-map', 'true');
    expect(useUIStore.getState().selectionAnnouncement).toBe('Upper level is hidden on the map');
    fireEvent.keyDown(header(), { key: 'H' });
    expect(useUIStore.getState().hiddenSections).toEqual([]);
    expect(useUIStore.getState().selectionAnnouncement).toBe('Upper level is shown on the map');
    // Not on the Ungrouped header, and not with a modifier.
    fireEvent.keyDown(ungrouped(), { key: 'h' });
    fireEvent.keyDown(header(), { key: 'h', ctrlKey: true });
    expect(useUIStore.getState().hiddenSections).toEqual([]);
  });

  it('the eye announces the change too', () => {
    fireEvent.click(screen.getByTestId('section-toggle-visibility-7'));
    expect(useUIStore.getState().selectionAnnouncement).toBe('Upper level is hidden on the map');
  });

  it('a hidden Section frames nothing: double-click only focuses, Enter says why', () => {
    act(() => useUIStore.getState().setSectionHidden(7, true));
    fireEvent.doubleClick(header());
    expect(useUIStore.getState().requestZoomToLine).toBeNull();
    act(() => header().focus());
    fireEvent.keyDown(header(), { key: 'Enter' });
    expect(useUIStore.getState().requestZoomToLine).toBeNull();
    expect(useUIStore.getState().selectionAnnouncement).toBe('Show the section on the map first');
    expect(header()).toHaveAttribute('title', expect.stringContaining('Hidden on the map'));
    // Shown again, double-click frames its lines.
    act(() => useUIStore.getState().setSectionHidden(7, false));
    fireEvent.doubleClick(header());
    expect(useUIStore.getState().requestZoomToLine).toEqual([1, 2]);
  });

  it('leaving the rename field with an unusable name keeps the old one and says why', () => {
    act(() => header().focus());
    fireEvent.keyDown(header(), { key: 'F2' });
    const input = screen.getByTestId('section-rename-input-7');
    fireEvent.change(input, { target: { value: '   ' } });
    fireEvent.blur(input);
    expect(screen.queryByTestId('section-rename-input-7')).toBeNull();
    expect(useProjectStore.getState().updateCaveSection).not.toHaveBeenCalled();
    expect(screen.getByTestId('section-name-7')).toHaveTextContent('Upper level');
    expect(useUIStore.getState().toasts.map((toast) => [toast.message, toast.kind])).toEqual([
      ['The section is still named "Upper level". A section needs a name.', 'warning'],
    ]);
  });

  it('Enter on a name with a control or bidi character keeps the field open with the reason', () => {
    act(() => header().focus());
    fireEvent.keyDown(header(), { key: 'F2' });
    const input = screen.getByTestId('section-rename-input-7');
    fireEvent.change(input, { target: { value: 'Upper\u202Elevel' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(screen.getByTestId('section-rename-error-7')).toHaveTextContent(
      'A name cannot contain control characters or invisible formatting characters.',
    );
    expect(useProjectStore.getState().updateCaveSection).not.toHaveBeenCalled();
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

describe('SectionsPanel New Section', () => {
  beforeEach(() => setup());
  const create = () => useProjectStore.getState().createCaveSection as ReturnType<typeof vi.fn>;
  const open = () => {
    fireEvent.click(screen.getByTestId('sections-new-section-button'));
    return screen.getByTestId('sections-new-section-input') as HTMLInputElement;
  };

  it('asks for the name first: the next free name, selected, and nothing created yet', () => {
    const input = open();
    expect(input).toHaveFocus();
    expect(input.value).toBe('Section 1');
    expect([input.selectionStart, input.selectionEnd]).toEqual([0, 'Section 1'.length]);
    expect(create()).not.toHaveBeenCalled();
    expect(screen.getByTestId('sections-new-section-hint')).toHaveTextContent('Enter creates the section');
  });

  it('Enter creates the Section with the typed name in ONE create_cave_section', async () => {
    const input = open();
    fireEvent.change(input, { target: { value: '  Sump ' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(create()).toHaveBeenCalledTimes(1);
    expect(create()).toHaveBeenCalledWith('Sump', expect.stringMatching(/^#/), []);
    expect(screen.queryByTestId('sections-new-section-row')).toBeNull();
    await act(async () => {});
    expect(useProjectStore.getState().updateCaveSection).not.toHaveBeenCalled();
  });

  it('Escape cancels: nothing is created and the row goes', () => {
    const input = open();
    fireEvent.change(input, { target: { value: 'Sump' } });
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(screen.queryByTestId('sections-new-section-row')).toBeNull();
    expect(useUIStore.getState().pendingNewSection).toBeNull();
    expect(create()).not.toHaveBeenCalled();
  });

  it('a taken name is refused at the field; leaving it with one creates nothing and says why', () => {
    const input = open();
    fireEvent.change(input, { target: { value: 'Upper level' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(screen.getByTestId('sections-new-section-error')).toHaveTextContent(
      'There is already a section named "Upper level".',
    );
    expect(input).toHaveAttribute('aria-invalid', 'true');
    fireEvent.blur(input);
    expect(screen.queryByTestId('sections-new-section-row')).toBeNull();
    expect(create()).not.toHaveBeenCalled();
    expect(useUIStore.getState().toasts.map((toast) => toast.message)).toEqual([
      'No section was created. There is already a section named "Upper level".',
    ]);
  });

  it('leaving the field with a usable name creates the Section', () => {
    const input = open();
    fireEvent.blur(input);
    expect(create()).toHaveBeenCalledWith('Section 1', expect.stringMatching(/^#/), []);
  });

  it('"New Section..." from a line holds the line, and says so', () => {
    act(() => useUIStore.getState().startNewSection([3]));
    expect(screen.getByTestId('sections-new-section-hint')).toHaveTextContent('with its line');
    const input = screen.getByTestId('sections-new-section-input');
    fireEvent.change(input, { target: { value: 'Sump' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(create()).toHaveBeenCalledWith('Sump', expect.stringMatching(/^#/), [3]);
  });

  it('closes when editing becomes blocked', () => {
    open();
    act(() => useUIStore.setState({ geometryMode: 'closureAdjusted' }));
    expect(screen.queryByTestId('sections-new-section-row')).toBeNull();
    expect(create()).not.toHaveBeenCalled();
  });
});

describe('SectionsPanel Ungrouped header', () => {
  it('is listed while some line is ungrouped', () => {
    setup();
    expect(ungrouped()).toBeInTheDocument();
  });

  it('is left out when every line is in a Section, and comes back as the drop target of a drag', () => {
    const grouped = lines.map((l) => ({ ...l, cave_section_id: 7 })) as Line[];
    setup(grouped);
    expect(screen.queryByTestId('sections-ungrouped-header')).toBeNull();
    // jsdom lays nothing out: no row is under the pointer.
    document.elementFromPoint = () => null;
    fireEvent.pointerDown(screen.getByTestId('line-row-2'), { button: 0, pointerType: 'mouse', clientX: 10, clientY: 10 });
    fireEvent.pointerMove(document, { clientX: 10, clientY: 40 });
    expect(ungrouped()).toBeInTheDocument();
    expect(screen.getByTestId('sections-ungrouped-stats')).toHaveTextContent('0 lines');
    fireEvent.pointerUp(document, { clientX: 10, clientY: 40 });
    expect(screen.queryByTestId('sections-ungrouped-header')).toBeNull();
  });
});
