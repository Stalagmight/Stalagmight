/**
 * The junction row of the station context menu (plan 5.2): a station two lines
 * share offers "Detach Line from Station...", which opens the detach dialog for
 * the clicked slot, and shows the unlink glyph at the Icons level. The label
 * used to be "Unmerge Stations", and the icon lookup kept matching the old
 * label, so the detach row fell back to the generic arrow.
 */
import type { ReactElement } from 'react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { ArrowUpRightIcon } from '../icons';
import { EntityUnlinkSymbol } from '../toolIcons';

vi.mock('../../../utils/combineLines', () => ({
  queryCombineCandidates: () => new Promise(() => {}),
  queryCombineCandidateForLines: () => new Promise(() => {}),
  combinePreview: () => ({ lines: [], stations: [] }),
}));

let dialogData: Record<string, unknown> = {};
const openDialog = vi.fn();
const uiState = () => ({
  closeDialog: vi.fn(),
  dialogData,
  openDialog,
  setActiveTool: vi.fn(),
  setMergeStationA: vi.fn(),
  triggerZoomToLine: vi.fn(),
  multiSelection: [],
  geometryMode: 'measured',
  appMode: 'survey',
  syntacticLevel: 'icons',
});
vi.mock('../../../store/uiStore', () => ({
  useUIStore: Object.assign((selector: (s: ReturnType<typeof uiState>) => unknown) => selector(uiState()), {
    getState: () => uiState(),
  }),
}));

const shot = (id: number) => ({ id, length: 5, azimuth_in: 0, azimuth_out: 0, depth_in: 0, depth_out: 0 });
const slot = (line_id: number, station_index: number, merged: Array<{ line_id: number; station_index: number }>) => ({
  station_ref: { line_id, station_index },
  coordinates: { x: 0, y: 0, z: 0 },
  is_zero_datum: false,
  gps_coordinates: null,
  merged_with: merged,
  closures_with: [],
  id: 7,
  name: '2',
});
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: () => ({
    project: {
      lines: [
        { id: 1, name: 'Main', shots: [shot(1), shot(2), shot(3)] },
        { id: 2, name: 'Side', shots: [shot(4)] },
      ],
      station_merges: [{ station_a: { line_id: 1, station_index: 2 }, station_b: { line_id: 2, station_index: 0 } }],
    },
    // Main's station 2 is Side's start: one station, two slots.
    stations: [slot(1, 2, [{ line_id: 2, station_index: 0 }]), slot(2, 0, [{ line_id: 1, station_index: 2 }])],
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
vi.mock('react-i18next', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react-i18next')>()),
  useTranslation: () => ({ t: (key: string) => key }),
}));

import ContextMenu from '../ContextMenu';

function openOn(lineId: number, stationIndex: number) {
  dialogData = {
    target: { type: 'station', ref: { line_id: lineId, station_index: stationIndex } },
    position: { x: 10, y: 10 },
    survivesScroll: true,
  };
  render(<ContextMenu />);
}

const detachRow = () => screen.getByRole('button', { name: 'Detach Line from Station...' });

describe('ContextMenu junction detach row', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('is offered on a junction, with no "Unmerge" wording', () => {
    openOn(1, 2);
    expect(detachRow()).toBeEnabled();
    expect(screen.queryByRole('button', { name: /unmerge/i })).toBeNull();
  });

  it('shows the unlink glyph, not the generic fallback arrow', () => {
    // Each glyph rendered the way the menu renders it (DOM-serialised alike).
    const markup = (node: ReactElement) => render(node).container.innerHTML;
    const unlink = markup(<EntityUnlinkSymbol entity="station" size={18} />);
    const fallback = markup(<ArrowUpRightIcon size={16} />);
    openOn(1, 2);
    const html = detachRow().innerHTML;
    expect(html).toContain(unlink);
    expect(html).not.toContain(fallback);
  });

  it("opens the detach dialog for the clicked slot with the station's other slots", () => {
    openOn(2, 0);
    fireEvent.click(detachRow());
    expect(openDialog).toHaveBeenCalledWith('unmerge-stations', {
      stationRef: { line_id: 2, station_index: 0 },
      mergedStations: [{ line_id: 1, station_index: 2 }],
    });
  });
});
