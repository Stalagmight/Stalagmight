/**
 * The "Combine Section With..." row of the station context menu: reserved from
 * the first render on every section END station, so the backend's async answer
 * never shifts the rows under the pointer; absent on a mid-section station. It
 * opens a submenu with every section ending there, each opening the dialog for
 * that pair or disabled with its own reason.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';

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
const labels = () => rows().map((r) => r.getAttribute('aria-label') ?? r.textContent);
const combineRow = () => screen.getByRole('button', { name: 'Combine Section With...' });
/** The submenu's rows, as [label, enabled, hint]. */
const submenuRows = () =>
  [...screen.getByTestId('context-menu-submenu').querySelectorAll('button')].map((b) => [
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

const candidate = (blocked: string | null, partner = 2) => ({
  section_a: 1,
  section_b: partner,
  end_a: 'end',
  end_b: 'start',
  station_a: { section_id: 1, station_index: 3 },
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

describe('ContextMenu Combine Sections row', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  /** The visible hint line of the Combine row. */
  const hintLine = () => screen.getByTestId(`${combineRow().getAttribute('data-testid')}-hint`);

  it('is reserved, disabled, while the answer is pending, and the rows never move', async () => {
    openOn(3);
    const before = labels();
    expect(combineRow()).toBeDisabled();
    expect(combineRow()).toHaveAttribute('title', expect.stringContaining('Checking'));
    expect(hintLine()).toHaveTextContent('Checking whether the sections can be combined...');

    await act(async () => resolveCandidate([candidate(null)]));

    expect(labels()).toEqual(before);
    expect(combineRow()).toBeEnabled();
    expect(hintLine()).toHaveTextContent('With Section 2');
    expect(combineRow()).toHaveAttribute('aria-haspopup', 'menu');
    expect(screen.queryByTestId('context-menu-submenu')).toBeNull();
    fireEvent.click(combineRow());
    expect(combineRow()).toHaveAttribute('aria-expanded', 'true');
    expect(submenuRows()).toEqual([['Section 2 (1 shot)', true, 'Merged here']]);
    fireEvent.click(screen.getByTestId('context-menu-subitem-0'));
    expect(openDialog).toHaveBeenCalledWith(
      'combine-sections',
      expect.objectContaining({ candidate: expect.objectContaining({ section_b: 2 }) })
    );
  });

  it('says when the two ends meet across a loop closure', async () => {
    openOn(3);
    await act(async () => resolveCandidate([{ ...candidate(null), merged: false, join_kind: 'closure' }]));
    expect(combineRow()).toBeEnabled();
    expect(hintLine()).toHaveTextContent('With Section 2, across a loop closure');
    fireEvent.mouseEnter(combineRow());
    expect(submenuRows()).toEqual([['Section 2 (1 shot)', true, 'Across a loop closure']]);
  });

  it('shows the reason as a visible line under the label, not only as a tooltip', async () => {
    openOn(3);
    await act(async () => resolveCandidate([candidate('Section 1 and Section 2 each carry a zero datum: a section can hold only one')]));
    expect(hintLine()).toHaveTextContent('Section 1 and Section 2 each carry a zero datum: a section can hold only one');
    expect(hintLine()).toBeVisible();
    expect(combineRow()).toHaveAttribute('title', 'Section 1 and Section 2 each carry a zero datum: a section can hold only one');
  });

  it('positions the menu for the taller hinted row', () => {
    dialogData = {
      target: { type: 'station', ref: { section_id: 1, station_index: 3 } },
      position: { x: 10, y: window.innerHeight - 5 },
      survivesScroll: true,
    };
    render(<ContextMenu />);
    const menu = screen.getByTestId('context-menu');
    // One child per item (row or separator), 36 px each, plus the hint's 32.
    const height = menu.children.length * 36 + 32;
    expect(menu.style.top).toBe(`${window.innerHeight - height - 20}px`);
  });

  it('stays in place with the reason when the answer is a refusal or nothing to combine', async () => {
    for (const [answer, reason] of [
      [[candidate('A section passes through here')], 'A section passes through here'],
      [[], 'No other section ends at this station'],
    ] as const) {
      openOn(0);
      const before = labels();
      await act(async () => resolveCandidate(answer));
      expect(labels()).toEqual(before);
      expect(combineRow()).toBeDisabled();
      expect(combineRow()).toHaveAttribute('title', reason);
      cleanup();
    }
  });

  it('at a junction, lists every partner in the submenu, each with its own reason', async () => {
    openOn(3);
    const before = labels();
    const both = [candidate(null, 2), candidate('Different quality', 4)];
    await act(async () => resolveCandidate(both));
    expect(labels()).toEqual(before);
    expect(combineRow()).toBeEnabled();
    expect(hintLine()).toHaveTextContent('2 sections end here');
    fireEvent.mouseEnter(combineRow());
    expect(submenuRows()).toEqual([
      ['Section 2 (1 shot)', true, 'Merged here'],
      ['Section 4 (1 shot)', false, 'Different quality'],
    ]);
    // A disabled partner opens nothing.
    fireEvent.click(screen.getByTestId('context-menu-subitem-1'));
    expect(openDialog).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId('context-menu-subitem-0'));
    expect(openDialog).toHaveBeenCalledWith('combine-sections', expect.objectContaining({ candidate: both[0] }));
    // Moving to another row closes the submenu.
    fireEvent.mouseEnter(rows()[0]!);
    expect(screen.queryByTestId('context-menu-submenu')).toBeNull();
    cleanup();

    // None combinable: the submenu still opens, so every reason stays readable.
    openOn(3);
    await act(async () => resolveCandidate([candidate('No', 2), candidate('Neither', 4)]));
    expect(hintLine()).toHaveTextContent('None of the 2 sections ending here can be combined');
    fireEvent.click(combineRow());
    expect(submenuRows().map(([, enabled, hint]) => [enabled, hint])).toEqual([
      [false, 'No'],
      [false, 'Neither'],
    ]);
  });

  it('is absent on a mid-section station, where nothing is asked', () => {
    openOn(1);
    expect(screen.queryByRole('button', { name: 'Combine Section With...' })).toBeNull();
    expect(queryCombineCandidate).not.toHaveBeenCalled();
  });
});
