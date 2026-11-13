/**
 * The Toolbar per workspace mode, with Data as the new case.
 *
 * Data is editable, so it gets Import and Undo/Redo like Survey, and the FULL tool
 * set (not the read-only one) - but only while its map pane is open: with the pane
 * closed there is no map to aim a tool or a Display toggle at. Closing the pane
 * under a mutating tool drops back to Select. Plan stays read-only: no Import, no
 * Undo/Redo, no mutating tools.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue({}) }));

import '../../../i18n';
import Toolbar from '../Toolbar';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useDataWorkspaceStore } from '../../DataWorkspace/store';

function renderToolbar() {
  return render(
    <Toolbar
      menuItems={[]}
      onImport={vi.fn()}
      onUndo={vi.fn()}
      onRedo={vi.fn()}
      canUndo
      canRedo
      placementModeActive={false}
      dataTableFullScreen={false}
    />,
  );
}

const toolsGroup = () => screen.queryByTestId('toolbar-tools-group');
const displayButton = () => screen.queryByTestId('display-menu-button');

/** jsdom has no ResizeObserver; the condense pass only needs it to exist. */
class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured', activeTool: 'select', showGrid: false });
  useProjectStore.setState({ project: null });
  // Confirmed-local project: the editable baseline (#270 fail-closed gate).
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  useDataWorkspaceStore.setState({ mapPaneOpen: false });
});

describe('Toolbar in Data', () => {
  it('shows Import and Undo/Redo, but no tools or Display menu while the map pane is closed', () => {
    useUIStore.setState({ appMode: 'data' });
    renderToolbar();
    expect(screen.getByTestId('toolbar-import-button')).toBeInTheDocument();
    expect(screen.getByTestId('toolbar-undo-button')).toBeInTheDocument();
    expect(screen.getByTestId('toolbar-redo-button')).toBeInTheDocument();
    expect(toolsGroup()).toBeNull();
    expect(displayButton()).toBeNull();
  });

  it('shows the FULL tool set and the Display menu once the map pane opens', () => {
    useUIStore.setState({ appMode: 'data' });
    renderToolbar();
    act(() => useDataWorkspaceStore.setState({ mapPaneOpen: true }));
    expect(toolsGroup()).not.toBeNull();
    expect(displayButton()).not.toBeNull();
    // Data is editable: the mutating tools are offered, exactly as in Survey.
    expect(screen.getByTestId('tool-merge')).toBeInTheDocument();
  });

  it('drops a mutating tool back to Select when the pane closes', () => {
    useUIStore.setState({ appMode: 'data', activeTool: 'merge' });
    useDataWorkspaceStore.setState({ mapPaneOpen: true });
    renderToolbar();
    expect(useUIStore.getState().activeTool).toBe('merge');
    act(() => useDataWorkspaceStore.setState({ mapPaneOpen: false }));
    expect(useUIStore.getState().activeTool).toBe('select');
  });

  it('keeps a non-mutating tool when the pane closes', () => {
    useUIStore.setState({ appMode: 'data', activeTool: 'measure' });
    useDataWorkspaceStore.setState({ mapPaneOpen: true });
    renderToolbar();
    act(() => useDataWorkspaceStore.setState({ mapPaneOpen: false }));
    expect(useUIStore.getState().activeTool).toBe('measure');
  });

  it('entering Data with the pane closed drops a mutating tool too', () => {
    useUIStore.setState({ activeTool: 'move_station' });
    renderToolbar();
    act(() => useUIStore.getState().setAppMode('data'));
    expect(useUIStore.getState().activeTool).toBe('select');
  });
});

describe('Toolbar in the other modes', () => {
  it('Survey shows everything regardless of the Data pane flag', () => {
    renderToolbar();
    expect(screen.getByTestId('toolbar-undo-button')).toBeInTheDocument();
    expect(toolsGroup()).not.toBeNull();
    expect(displayButton()).not.toBeNull();
    expect(screen.getByTestId('tool-merge')).toBeInTheDocument();
  });

  it('Plan hides Import, Undo/Redo and the mutating tools', () => {
    useUIStore.setState({ appMode: 'plan' });
    renderToolbar();
    expect(screen.queryByTestId('toolbar-import-button')).toBeNull();
    expect(screen.queryByTestId('toolbar-undo-button')).toBeNull();
    expect(screen.queryByTestId('toolbar-redo-button')).toBeNull();
    expect(toolsGroup()).not.toBeNull();
    expect(screen.queryByTestId('tool-merge')).toBeNull();
  });
});
