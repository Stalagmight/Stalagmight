/**
 * The map's survey (editing) context menu in the Data workspace.
 *
 * Data is editable and its map pane is the same map, so a right-click there gets
 * Survey's full menu (Combine Sections and all). With the pane closed there is
 * no map, so nothing may open the map's mutation menu: it renders nothing.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { useDataWorkspaceStore } from '../../DataWorkspace/store';

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
  combinePreviewAll: () => ({ sections: [], stations: [] }),
}));

let dialogData: Record<string, unknown> = {};
let appMode: 'survey' | 'data' = 'survey';
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
  appMode,
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

const combineRow = () => screen.queryByRole('button', { name: 'Combine Sections...' });

function openOnEnd() {
  dialogData = {
    target: { type: 'station', ref: { section_id: 1, station_index: 3 } },
    position: { x: 10, y: 10 },
    survivesScroll: true,
  };
  return render(<ContextMenu />);
}

describe('ContextMenu in Data', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    appMode = 'data';
    useDataWorkspaceStore.setState({ mapPaneOpen: true });
  });

  it('gives the open map pane the full survey menu, Combine Sections included', () => {
    openOnEnd();
    expect(screen.getByTestId('context-menu')).toBeInTheDocument();
    expect(combineRow()).not.toBeNull();
    expect(queryCombineCandidate).toHaveBeenCalled();
  });

  it('renders nothing while the map pane is closed', () => {
    useDataWorkspaceStore.setState({ mapPaneOpen: false });
    openOnEnd();
    expect(screen.queryByTestId('context-menu')).toBeNull();
    expect(queryCombineCandidate).not.toHaveBeenCalled();
  });

  it('Survey is unaffected by the Data pane flag', () => {
    appMode = 'survey';
    useDataWorkspaceStore.setState({ mapPaneOpen: false });
    openOnEnd();
    expect(combineRow()).not.toBeNull();
  });
});
