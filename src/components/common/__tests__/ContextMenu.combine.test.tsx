/**
 * The combine row of the station context menu: reserved from the first render on
 * every section END station, so the backend's async answer never shifts the
 * rows under the pointer; absent on a mid-section station. With one other
 * section at the point it is that pair, directly; with two others, a submenu
 * of every pair; with more, a submenu per section of whom it merges with.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

type Resolve = (candidate: unknown) => void;
let resolveCandidate: Resolve = () => {};
const queryCombineCandidate = vi.fn(
  () =>
    new Promise((resolve) => {
      resolveCandidate = resolve;
    })
);
vi.mock('../../../utils/combineSections', () => ({
  queryCombineCandidates: () => queryCombineCandidate(),
  queryCombineCandidateForSections: () => queryCombineCandidate(),
  combinePreview: () => ({ sections: [], stations: [] }),
}));

let dialogData: Record<string, unknown> = {};
const openDialog = vi.fn();
const uiState = () => ({
  closeDialog: vi.fn(),
  dialogData,
  openDialog,
  setActiveTool: vi.fn(),
  setMergeStationA: vi.fn(),
  triggerZoomToSection: vi.fn(),
  multiSelection: [],
  geometryMode: 'measured',
  appMode: 'survey',
  syntacticLevel: 'none',
});
vi.mock('../../../store/uiStore', () => ({
  useUIStore: Object.assign((selector: (s: ReturnType<typeof uiState>) => unknown) => selector(uiState()), {
    getState: () => uiState(),
  }),
}));

const shot = (id: number) => ({ id, length: 5, azimuth_in: 0, azimuth_out: 0, depth_in: 0, depth_out: 0 });
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: () => ({
    project: { sections: [{ id: 1, name: 'Main', shots: [shot(1), shot(2), shot(3)] }], station_merges: [] },
    stations: [],
    setShotKind: vi.fn(),
    unmergeStations: vi.fn(),
    unmergeStationPair: vi.fn(),
    removeLoopClosure: vi.fn(),
  }),
}));
vi.mock('../../../store/gasPlanStore', () => ({
  useGasPlanStore: { getState: () => ({ reachField: null, lastReachField: null }) },
}));
vi.mock('../../../store/speleoStore', () => ({
  useSpeleoStore: () => false,
  selectSpeleoReadOnly: () => false,
}));
vi.mock('../../../hooks/useDismissableLayer', () => ({ useDismissableLayer: () => {} }));
vi.mock('../../../utils/telemetry', () => ({
  setSource: vi.fn(),
  trackContextMenuAction: vi.fn(),
  trackDialogComplete: vi.fn(),
}));
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }));

import ContextMenu from '../ContextMenu';

const rows = () => screen.getAllByRole('button').filter((b) => b.getAttribute('data-testid')?.startsWith('context-menu-item-'));
/** The combine row: the 8th row of an end station's menu, whatever its label. */
const combineRow = () => screen.getByRole('button', { name: /^Combine Section/ });
/** A flyout's rows, as [label, enabled, hint]. */
const submenuRows = (testId = 'context-menu-submenu') =>
  [...screen.getByTestId(testId).querySelectorAll('button')].map((b) => [
    b.getAttribute('aria-label') ?? b.textContent,
    !b.disabled,
    b.getAttribute('title'),
  ]);

function openOn(stationIndex: number) {
  dialogData = {
    target: { type: 'station', ref: { section_id: 1, station_index: stationIndex } },
    position: { x: 10, y: 10 },
    survivesScroll: true,
  };
  render(<ContextMenu />);
}

const candidate = (blocked: string | null, partner = 2, first = 1) => ({
  section_a: first,
  section_b: partner,
  end_a: 'end',
  end_b: 'start',
  station_a: { section_id: first, station_index: 3 },
  station_b: { section_id: partner, station_index: 0 },
  merged: true,
  reverses: false,
  blocked_reason: blocked,
  blocked_keeping_a: null,
  blocked_keeping_b: null,
  shot_count_a: 3,
  shot_count_b: 1,
  comment_a: null,
  comment_b: null,
  station_name_a: null,
  station_name_b: null,
  depth_difference_m: null,
  branches: [],
  loose_ends: [],
  join_kind: 'merged',
  passing_through: [],
  repeated_shot_ids: 0,
  plan_keeping_a: null,
  plan_keeping_b: null,
});

describe('ContextMenu combine row', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  /** The visible hint line of the combine row. */
  const hintLine = () => screen.getByTestId(`${combineRow().getAttribute('data-testid')}-hint`);
  /** Row positions: the rows never move when the answer arrives. */
  const rowIds = () => rows().map((r) => r.getAttribute('data-testid'));

  it('is reserved, disabled, while the answer is pending, then names the one other section directly', async () => {
    openOn(3);
    const before = rowIds();
    expect(combineRow()).toHaveAccessibleName('Combine Sections...');
    expect(combineRow()).toBeDisabled();
    expect(combineRow()).toHaveAttribute('title', expect.stringContaining('Checking'));
    expect(hintLine()).toHaveTextContent('Checking which sections meet here...');

    await act(async () => resolveCandidate([candidate(null)]));

    expect(rowIds()).toEqual(before);
    expect(combineRow()).toHaveAccessibleName('Combine Section With Section 2...');
    expect(combineRow()).toBeEnabled();
    expect(hintLine()).toHaveTextContent('Merged here');
    expect(combineRow()).not.toHaveAttribute('aria-haspopup');
    fireEvent.click(combineRow());
    expect(screen.queryByTestId('context-menu-submenu')).toBeNull();
    expect(openDialog).toHaveBeenCalledWith(
      'combine-sections',
      expect.objectContaining({ candidate: expect.objectContaining({ section_b: 2 }), keep: 1 })
    );
  });

  it('says when the one other section is across a loop closure', async () => {
    openOn(3);
    await act(async () => resolveCandidate([{ ...candidate(null), merged: false, join_kind: 'closure' }]));
    expect(combineRow()).toBeEnabled();
    expect(hintLine()).toHaveTextContent('Across a loop closure');
  });

  it('gives a hint one line in the menu, and lets a flyout row wrap', async () => {
    openOn(3);
    expect(hintLine().className).toContain('h-4');
    expect(hintLine().className).toContain('truncate');
    await act(async () => resolveCandidate([candidate(null, 2), candidate('A long reason', 4), candidate(null, 4, 2)]));
    fireEvent.mouseEnter(combineRow());
    const flyoutHint = screen.getByTestId('context-menu-subitem-1-hint');
    expect(flyoutHint.className).toContain('line-clamp-3');
    expect(flyoutHint.className).not.toContain('h-4');
  });

  it('shows the reason as a visible line under the label, not only as a tooltip', async () => {
    const reason = 'Section 1 and Section 2 each carry a zero datum: a section can hold only one';
    openOn(3);
    await act(async () => resolveCandidate([candidate(reason)]));
    expect(hintLine()).toHaveTextContent(reason);
    expect(hintLine()).toBeVisible();
    expect(combineRow()).toBeDisabled();
    expect(combineRow()).toHaveAttribute('title', reason);
  });

  it('positions the menu for the taller hinted row', () => {
    dialogData = {
      target: { type: 'station', ref: { section_id: 1, station_index: 3 } },
      position: { x: 10, y: window.innerHeight - 5 },
      survivesScroll: true,
    };
    render(<ContextMenu />);
    const menu = screen.getByTestId('context-menu');
    // One child per item (row or separator), 36 px each, plus the hint's one
    // 16 px line (it used to be two, which left a blank line under a short hint).
    const height = menu.children.length * 36 + 16;
    expect(menu.style.top).toBe(`${window.innerHeight - height - 20}px`);
  });

  it('stays in place with the reason when nothing else ends there', async () => {
    openOn(0);
    const before = rowIds();
    await act(async () => resolveCandidate([]));
    expect(rowIds()).toEqual(before);
    expect(combineRow()).toBeDisabled();
    expect(combineRow()).toHaveAttribute('title', 'No other section ends at this station');
  });

  it('with three sections at the point, lists every pair in one submenu', async () => {
    openOn(3);
    const pairs = [candidate(null, 2), candidate('Different quality', 4), candidate(null, 4, 2)];
    await act(async () => resolveCandidate(pairs));
    expect(combineRow()).toHaveAccessibleName('Combine Sections...');
    expect(combineRow()).toBeEnabled();
    expect(hintLine()).toHaveTextContent('3 sections end here');
    fireEvent.mouseEnter(combineRow());
    expect(submenuRows()).toEqual([
      ['Main with Section 2', true, 'Merged here'],
      ['Main with Section 4', false, 'Different quality'],
      ['Section 2 with Section 4', true, 'Merged here'],
    ]);
    // A disabled pair opens nothing; a pair of the others keeps the first-named.
    fireEvent.click(screen.getByTestId('context-menu-subitem-1'));
    expect(openDialog).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId('context-menu-subitem-2'));
    expect(openDialog).toHaveBeenCalledWith('combine-sections', expect.objectContaining({ candidate: pairs[2], keep: 2 }));
    // Moving to another row closes the submenu.
    fireEvent.mouseEnter(rows()[0]!);
    expect(screen.queryByTestId('context-menu-submenu')).toBeNull();
  });

  it('with four or more, nests "A with..." over whom A merges with', async () => {
    openOn(3);
    const pairs = [
      candidate(null, 2),
      candidate(null, 4),
      candidate('No', 5),
      candidate(null, 4, 2),
      candidate(null, 5, 2),
      candidate(null, 5, 4),
    ];
    await act(async () => resolveCandidate(pairs));
    expect(hintLine()).toHaveTextContent('4 sections end here');
    fireEvent.click(combineRow());
    expect(submenuRows().map(([label]) => label)).toEqual([
      'Main with...',
      'Section 2 with...',
      'Section 4 with...',
      'Section 5 with...',
    ]);
    fireEvent.mouseEnter(screen.getByTestId('context-menu-subitem-0'));
    expect(submenuRows('context-menu-submenu-2')).toEqual([
      ['Section 2 (1 shot)', true, 'Merged here'],
      ['Section 4 (1 shot)', true, 'Merged here'],
      ['Section 5 (1 shot)', false, 'No'],
    ]);
    // Another section's list: it is the one kept.
    fireEvent.mouseEnter(screen.getByTestId('context-menu-subitem-3'));
    // (The fixture gives every pair's `section_a` 3 shots.)
    expect(submenuRows('context-menu-submenu-2').map(([label]) => label)).toEqual([
      'Main (3 shots)',
      'Section 2 (3 shots)',
      'Section 4 (3 shots)',
    ]);
    fireEvent.click(screen.getByTestId('context-menu-subitem-2-1'));
    expect(openDialog).toHaveBeenCalledWith('combine-sections', expect.objectContaining({ candidate: pairs[4], keep: 5 }));
    // Hovering a plain row of the first flyout would close the nested one;
    // hovering the top menu closes both.
    fireEvent.mouseEnter(rows()[0]!);
    expect(screen.queryByTestId('context-menu-submenu')).toBeNull();
    expect(screen.queryByTestId('context-menu-submenu-2')).toBeNull();
  });

  it('is absent on a mid-section station, where nothing is asked', () => {
    openOn(1);
    expect(screen.queryByRole('button', { name: /^Combine Section/ })).toBeNull();
    expect(queryCombineCandidate).not.toHaveBeenCalled();
  });
});
