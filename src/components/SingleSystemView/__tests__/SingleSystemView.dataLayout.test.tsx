/**
 * The single-project view's layout in the Data workspace.
 *
 * Data shows its workspace in the map area, to the LEFT of the map, and neither
 * dock: no Details on the right, no docked data table at the bottom. With the
 * map pane closed the workspace covers the map; with it open the map takes a
 * right-hand pane behind a splitter. Through every switch MapCanvas must stay
 * mounted - a remount throws away the WebGL context, sources and camera - so the
 * stubbed map counts its mounts.
 *
 * Every heavy child is stubbed: this pins the layout, not the panels.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { useEffect } from 'react';

const mounts = vi.hoisted(() => ({ map: 0, workspace: 0 }));

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue({}) }));
vi.mock('../../Map/MapCanvas', () => ({
  default: function MapCanvasStub() {
    useEffect(() => {
      mounts.map += 1;
    }, []);
    return <div data-testid="map-canvas-stub" />;
  },
}));
vi.mock('../../DataWorkspace/DataWorkspace', () => ({
  DataWorkspace: function DataWorkspaceStub() {
    useEffect(() => {
      mounts.workspace += 1;
    }, []);
    return <div data-testid="data-workspace" />;
  },
}));
vi.mock('../../DataWorkspace/DataMapPaneHeader', () => ({
  DataMapPaneHeader: () => <div data-testid="data-map-pane-header-stub" />,
}));
vi.mock('../../DataWorkspace/BackToDataChip', () => ({
  BackToDataChip: () => <div data-testid="back-to-data-chip-stub" />,
}));
vi.mock('../Toolbar', () => ({ default: () => <div data-testid="toolbar" /> }));
vi.mock('../SpeleoBanner', () => ({ SpeleoBanner: () => null }));
vi.mock('../LeftWorkspace', () => ({ LeftWorkspace: () => <div data-testid="left-workspace-stub" /> }));
vi.mock('../SectionsPanel', () => ({ default: () => <div data-testid="sections-panel-stub" /> }));
vi.mock('../DetailPanel', () => ({ default: () => <div data-testid="detail-panel-stub" /> }));
vi.mock('../DataTable', () => ({ default: () => <div data-testid="docked-data-table-stub" /> }));
vi.mock('../GasPlanPanel', () => ({ GasPlanPanel: () => <div data-testid="gas-plan-panel-stub" /> }));
vi.mock('../PlanTablesPanel', () => ({ PlanTablesPanel: () => <div data-testid="plan-tables-stub" /> }));
vi.mock('../ExportPanel', () => ({ ExportPanel: () => <div data-testid="export-panel-stub" /> }));
vi.mock('../../Dialogs/ExportPreview', () => ({ default: () => <div data-testid="export-preview-stub" /> }));
vi.mock('../../common/OfflineIndicator', () => ({ OfflineIndicator: () => null }));
vi.mock('../../Map/MapAttribution', () => ({ MapAttribution: () => null }));
vi.mock('../useProjectMenuItems', () => ({ useProjectMenuItems: () => [] }));
vi.mock('../../../hooks/useKeyboardShortcuts', () => ({ useKeyboardShortcuts: () => {} }));
vi.mock('../../../hooks/useOnboardingStep', () => ({
  useOnboardingStep: () => ({ flowId: null, stepId: null }),
}));
vi.mock('../../../hooks/useAutoSave', () => ({
  useAutoSave: () => ({ status: 'idle', countdown: 0 }),
}));
vi.mock('../../../store/speleoCollaboratorsStore', () => ({ useCollaboratorsSync: () => {} }));
vi.mock('../../../hooks/useProject', () => ({
  useProject: () => ({
    project: { name: 'Synthetic cave', file_path: '/tmp/synthetic.stalx', sections: [] },
    stats: { section_count: 0, shot_count: 0, total_length: 0 },
    handleSaveProject: vi.fn(),
    canUndo: false,
    canRedo: false,
    undo: vi.fn(),
    redo: vi.fn(),
  }),
}));

import '../../../i18n';
import SingleSystemView from '../SingleSystemView';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useDataWorkspaceStore, MAP_PANE_DEFAULT_WIDTH, MAP_PANE_MIN_WIDTH } from '../../DataWorkspace/store';
import { DATA_WORKSPACE_MIN_WIDTH, mapPaneMaxWidth } from '../DataModeLayout';
import { useProjectStore } from '../../../store/projectStore';
import { defaultConfig, type Config } from '../../../types/config';
import { resetDataWorkspaceViewPersistenceForTests } from '../../DataWorkspace/useDataWorkspaceViewPersistence';

class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

const docks = () => ({
  detail: screen.queryByTestId('detail-panel-bar'),
  data: screen.queryByTestId('data-panel-bar'),
});
const splitter = () => screen.queryByTestId('data-map-pane-splitter');

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  mounts.map = 0;
  mounts.workspace = 0;
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  useUIStore.setState({
    appMode: 'survey',
    geometryMode: 'measured',
    view: 'single',
    detailPanelOpen: true,
    detailPanelMode: 'pin',
    dataTableOpen: true,
    dataTableMode: 'pin',
    focusedSectionId: null,
    dataTableFullScreen: false,
    placementMode: null,
  });
  useDataWorkspaceStore.setState({ mapPaneOpen: false, mapPaneWidth: MAP_PANE_DEFAULT_WIDTH });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('SingleSystemView and the persisted Data view', () => {
  it('loads it while the project view is open, before Data is ever entered', () => {
    resetDataWorkspaceViewPersistenceForTests();
    useDataWorkspaceStore.setState({ viewLoaded: false, mapPaneOpen: false });
    const config = structuredClone(defaultConfig) as Config;
    useProjectStore.setState({
      config: { ...config, panels: { ...config.panels, data_workspace_map_pane_open: true, data_workspace_map_pane_width: 640 } },
    });
    try {
      render(<SingleSystemView />);
      expect(useUIStore.getState().appMode).toBe('survey');
      expect(useDataWorkspaceStore.getState()).toMatchObject({ viewLoaded: true, mapPaneOpen: true, mapPaneWidth: 640 });
    } finally {
      useProjectStore.setState({ config: null });
      resetDataWorkspaceViewPersistenceForTests();
    }
  });
});

describe('SingleSystemView in Data', () => {
  it('Survey has both docks and no Data workspace', () => {
    render(<SingleSystemView />);
    expect(docks().detail).not.toBeNull();
    expect(docks().data).not.toBeNull();
    expect(screen.queryByTestId('data-workspace')).toBeNull();
    expect(screen.getByTestId('back-to-data-chip-stub')).toBeInTheDocument();
  });

  it('Data renders the workspace over the map and neither dock', () => {
    useUIStore.setState({ appMode: 'data' });
    render(<SingleSystemView />);
    expect(screen.getByTestId('data-workspace')).toBeInTheDocument();
    expect(docks().detail).toBeNull();
    expect(docks().data).toBeNull();
    expect(screen.queryByTestId('detail-panel-stub')).toBeNull();
    expect(screen.queryByTestId('docked-data-table-stub')).toBeNull();
    // Pane closed: the workspace covers the map, which stays mounted but hidden.
    expect(screen.getByTestId('data-workspace-slot').className).toContain('absolute');
    expect(screen.getByTestId('map-pane')).toHaveAttribute('aria-hidden', 'true');
    expect(splitter()).toBeNull();
    expect(screen.queryByTestId('data-map-pane-header-stub')).toBeNull();
    expect(screen.queryByTestId('back-to-data-chip-stub')).toBeNull();
    // The footer status bar stays.
    expect(screen.getByTestId('status-bar')).toBeInTheDocument();
  });

  it('opening the map pane shows the splitter, the pane header and the map at the stored width', () => {
    useUIStore.setState({ appMode: 'data' });
    useDataWorkspaceStore.setState({ mapPaneOpen: true, mapPaneWidth: 520 });
    render(<SingleSystemView />);
    const sep = splitter();
    expect(sep).not.toBeNull();
    expect(sep).toHaveAttribute('role', 'separator');
    expect(sep).toHaveAttribute('aria-valuenow', '520');
    expect(sep).toHaveAttribute('aria-valuemin', String(MAP_PANE_MIN_WIDTH));
    expect(screen.getByTestId('data-map-pane-header-stub')).toBeInTheDocument();
    expect(screen.getByTestId('map-pane').style.width).toBe('520px');
    expect(screen.getByTestId('map-pane')).not.toHaveAttribute('aria-hidden');
  });

  it('never remounts the map (or the workspace) across mode and pane changes', () => {
    render(<SingleSystemView />);
    expect(mounts.map).toBe(1);
    act(() => useUIStore.getState().setAppMode('data'));
    act(() => useDataWorkspaceStore.setState({ mapPaneOpen: true }));
    act(() => useDataWorkspaceStore.setState({ mapPaneOpen: false }));
    act(() => useDataWorkspaceStore.setState({ mapPaneOpen: true }));
    act(() => useUIStore.getState().setAppMode('plan'));
    act(() => useUIStore.getState().setAppMode('data'));
    act(() => useUIStore.getState().setAppMode('survey'));
    expect(mounts.map).toBe(1);
    expect(screen.getAllByTestId('map-canvas-stub')).toHaveLength(1);
  });

  it('keeps the workspace mounted when the map pane opens and closes', () => {
    useUIStore.setState({ appMode: 'data' });
    render(<SingleSystemView />);
    act(() => useDataWorkspaceStore.setState({ mapPaneOpen: true }));
    act(() => useDataWorkspaceStore.setState({ mapPaneOpen: false }));
    expect(mounts.workspace).toBe(1);
  });

  it('the splitter resizes by keyboard and resets on double-click', () => {
    vi.useFakeTimers();
    // jsdom lays nothing out; give the map area a real width so the clamp has room.
    const clientWidth = vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(1600);
    try {
      useUIStore.setState({ appMode: 'data' });
      useDataWorkspaceStore.setState({ mapPaneOpen: true, mapPaneWidth: 500 });
      render(<SingleSystemView />);
      const sep = splitter()!;
      // The splitter is the pane's left edge: ArrowLeft widens the map.
      fireEvent.keyDown(sep, { key: 'ArrowLeft' });
      expect(sep).toHaveAttribute('aria-valuenow', '516');
      fireEvent.keyDown(sep, { key: 'ArrowRight', shiftKey: true });
      expect(sep).toHaveAttribute('aria-valuenow', '452');
      // The store is written once the burst settles, not per key.
      expect(useDataWorkspaceStore.getState().mapPaneWidth).toBe(500);
      act(() => {
        vi.advanceTimersByTime(400);
      });
      expect(useDataWorkspaceStore.getState().mapPaneWidth).toBe(452);
      fireEvent.keyDown(sep, { key: 'Home' });
      expect(sep).toHaveAttribute('aria-valuenow', String(MAP_PANE_MIN_WIDTH));
      fireEvent.doubleClick(sep);
      expect(useDataWorkspaceStore.getState().mapPaneWidth).toBe(MAP_PANE_DEFAULT_WIDTH);
      expect(sep).toHaveAttribute('aria-valuenow', String(MAP_PANE_DEFAULT_WIDTH));
      // End jumps to the widest the table allows.
      fireEvent.keyDown(sep, { key: 'End' });
      expect(sep).toHaveAttribute('aria-valuenow', String(1600 - DATA_WORKSPACE_MIN_WIDTH));
    } finally {
      clientWidth.mockRestore();
      vi.useRealTimers();
    }
  });
});

describe('the splitter drag', () => {
  it('moves the pane live but writes the store once, on release', () => {
    const clientWidth = vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(1600);
    const capture = vi.fn();
    Object.defineProperty(HTMLElement.prototype, 'setPointerCapture', { configurable: true, value: capture });
    try {
      useUIStore.setState({ appMode: 'data' });
      useDataWorkspaceStore.setState({ mapPaneOpen: true, mapPaneWidth: 500 });
      render(<SingleSystemView />);
      const pane = screen.getByTestId('map-pane');
      // The pane's right edge is the area's right edge (x = 1600).
      vi.spyOn(pane, 'getBoundingClientRect').mockReturnValue({ left: 1100, right: 1600, top: 0, bottom: 800, width: 500, height: 800, x: 1100, y: 0, toJSON: () => ({}) });
      const sep = splitter()!;
      fireEvent.pointerDown(sep, { pointerId: 1, clientX: 1100 });
      fireEvent.pointerMove(sep, { pointerId: 1, clientX: 1000 });
      fireEvent.pointerMove(sep, { pointerId: 1, clientX: 980 });
      expect(sep).toHaveAttribute('aria-valuenow', '620');
      expect(pane.style.width).toBe('620px');
      expect(useDataWorkspaceStore.getState().mapPaneWidth).toBe(500);
      fireEvent.pointerUp(sep, { pointerId: 1, clientX: 980 });
      expect(useDataWorkspaceStore.getState().mapPaneWidth).toBe(620);
    } finally {
      clientWidth.mockRestore();
      delete (HTMLElement.prototype as { setPointerCapture?: unknown }).setPointerCapture;
    }
  });
});

describe('mapPaneMaxWidth', () => {
  it('leaves the table its minimum width, never below the pane floor', () => {
    expect(mapPaneMaxWidth(1600, 480)).toBe(1600 - DATA_WORKSPACE_MIN_WIDTH);
    expect(mapPaneMaxWidth(700, 480)).toBe(MAP_PANE_MIN_WIDTH);
  });

  it('does not clamp before the area has been measured', () => {
    expect(mapPaneMaxWidth(0, 900)).toBe(900);
    expect(mapPaneMaxWidth(0, 100)).toBe(MAP_PANE_MIN_WIDTH);
  });
});
