/**
 * The Combine Sections row of the station context menu: reserved from the first
 * render on every section END station, so the backend's async answer never
 * shifts the rows under the pointer; absent on a mid-section station.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, cleanup, render, screen } from '@testing-library/react';

type Resolve = (candidate: unknown) => void;
let resolveCandidate: Resolve = () => {};
const queryCombineCandidate = vi.fn(
  () =>
    new Promise((resolve) => {
      resolveCandidate = resolve;
    })
);
vi.mock('../../../utils/combineSections', () => ({
  queryCombineCandidate: () => queryCombineCandidate(),
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
const labels = () => rows().map((r) => r.textContent);
const combineRow = () => screen.getByRole('button', { name: 'Combine Sections...' });

function openOn(stationIndex: number) {
  dialogData = {
    target: { type: 'station', ref: { section_id: 1, station_index: stationIndex } },
    position: { x: 10, y: 10 },
    survivesScroll: true,
  };
  render(<ContextMenu />);
}

const candidate = (blocked: string | null) => ({
  section_a: 1,
  section_b: 2,
  end_a: 'end',
  end_b: 'start',
  station_a: { section_id: 1, station_index: 3 },
  station_b: { section_id: 2, station_index: 0 },
  merged: true,
  reverses: false,
  blocked_reason: blocked,
  shot_count_a: 3,
  shot_count_b: 1,
  comment_a: null,
  comment_b: null,
  station_name_a: null,
  station_name_b: null,
});

describe('ContextMenu Combine Sections row', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('is reserved, disabled, while the answer is pending, and the rows never move', async () => {
    openOn(3);
    const before = labels();
    expect(combineRow()).toBeDisabled();
    expect(combineRow()).toHaveAttribute('title', expect.stringContaining('Checking'));

    await act(async () => resolveCandidate(candidate(null)));

    expect(labels()).toEqual(before);
    expect(combineRow()).toBeEnabled();
  });

  it('stays in place with the reason when the answer is a refusal or nothing to combine', async () => {
    for (const [answer, reason] of [
      [candidate('This is a branch'), 'This is a branch'],
      [null, 'No other section ends at this station'],
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

  it('is absent on a mid-section station, where nothing is asked', () => {
    openOn(1);
    expect(screen.queryByRole('button', { name: 'Combine Sections...' })).toBeNull();
    expect(queryCombineCandidate).not.toHaveBeenCalled();
  });
});
