/**
 * Ctrl+E and Escape for the Export workspace. Export is not a dialog any more:
 * Ctrl+E enters it and closes it again (back to the mode it was entered from),
 * and stays inert behind a dialog, an independent overlay, placement mode and
 * the fullscreen data table. Escape closes it as the last layer, after any open
 * dialog or transient map state.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useKeyboardShortcuts } from '../useKeyboardShortcuts';
import { useUIStore, type AppMode } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';

// useKeyboardShortcuts pulls in telemetry/save helpers that ultimately call the
// Tauri IPC bridge; stub it so importing the real hook here stays offline.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue({}),
}));

function pressCtrlE(): void {
  window.dispatchEvent(new KeyboardEvent('keydown', { key: 'e', ctrlKey: true, bubbles: true }));
}

function pressEscape(): void {
  window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
}

const STATION_SELECTION = { type: 'station' as const, stationRef: { section_id: 0, station_index: 1 } };

beforeEach(() => {
  useUIStore.setState({
    selection: null,
    multiSelection: [],
    activeDialog: null,
    dialogData: {},
    placementMode: null,
    screenOccupants: 0,
    dataTableFullScreen: false,
    activeTool: 'select',
    appMode: 'survey',
    geometryMode: 'measured',
    measureStart: null,
    measureEnd: null,
  });
  useUIStore.setState({ exportReturnMode: null });
  useProjectStore.setState({ project: { file_path: '/synthetic.stalx', sections: [] } as never });
});

describe('Ctrl+E toggles the Export workspace', () => {
  it.each<Exclude<AppMode, 'export'>>(['survey', 'data', 'plan'])('enters from %s and closes back to it', (from) => {
    useUIStore.getState().setAppMode(from);
    renderHook(() => useKeyboardShortcuts());
    pressCtrlE();
    expect(useUIStore.getState().appMode).toBe('export');
    expect(useUIStore.getState().activeDialog).toBeNull();
    pressCtrlE();
    expect(useUIStore.getState().appMode).toBe(from);
  });

  it('does nothing with no project open', () => {
    useProjectStore.setState({ project: null });
    renderHook(() => useKeyboardShortcuts());
    pressCtrlE();
    expect(useUIStore.getState().appMode).toBe('survey');
  });

  it('is inert behind an open dialog, in Survey and in Export', () => {
    renderHook(() => useKeyboardShortcuts());
    useUIStore.setState({ activeDialog: 'settings' });
    pressCtrlE();
    expect(useUIStore.getState().appMode).toBe('survey');

    useUIStore.setState({ activeDialog: null });
    useUIStore.getState().enterExport();
    useUIStore.setState({ activeDialog: 'settings' });
    pressCtrlE();
    expect(useUIStore.getState().appMode).toBe('export');
  });

  it('is inert behind an independent overlay', () => {
    useUIStore.setState({ screenOccupants: 1 });
    renderHook(() => useKeyboardShortcuts());
    pressCtrlE();
    expect(useUIStore.getState().appMode).toBe('survey');
  });

  it('is inert during placement mode', () => {
    useUIStore.setState({ placementMode: { kind: 'synthetic' } as never });
    renderHook(() => useKeyboardShortcuts());
    pressCtrlE();
    expect(useUIStore.getState().appMode).toBe('survey');
  });

  it('is inert while the data table is fullscreen', () => {
    useUIStore.setState({ dataTableFullScreen: true });
    renderHook(() => useKeyboardShortcuts());
    pressCtrlE();
    expect(useUIStore.getState().appMode).toBe('survey');
    expect(useUIStore.getState().dataTableFullScreen).toBe(true);
  });
});

describe('Escape in the Export workspace', () => {
  it('closes Export back to the recorded mode when nothing else is open', () => {
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().enterExport();
    renderHook(() => useKeyboardShortcuts());
    pressEscape();
    expect(useUIStore.getState().appMode).toBe('plan');
  });

  it('closes an open dialog first and stays in Export', () => {
    useUIStore.getState().enterExport();
    useUIStore.setState({ activeDialog: 'settings' });
    renderHook(() => useKeyboardShortcuts());
    pressEscape();
    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(useUIStore.getState().appMode).toBe('export');
    pressEscape();
    expect(useUIStore.getState().appMode).toBe('survey');
  });

  it('clears a map selection first and stays in Export', () => {
    useUIStore.getState().enterExport();
    useUIStore.setState({ selection: STATION_SELECTION, multiSelection: [STATION_SELECTION] });
    renderHook(() => useKeyboardShortcuts());
    pressEscape();
    expect(useUIStore.getState().selection).toBeNull();
    expect(useUIStore.getState().appMode).toBe('export');
    pressEscape();
    expect(useUIStore.getState().appMode).toBe('survey');
  });

  it('leaves Export alone while a dropdown owns Escape', () => {
    useUIStore.getState().enterExport();
    const dropdown = document.createElement('div');
    dropdown.setAttribute('data-dropdown-open', '');
    document.body.appendChild(dropdown);
    try {
      renderHook(() => useKeyboardShortcuts());
      pressEscape();
      expect(useUIStore.getState().appMode).toBe('export');
    } finally {
      dropdown.remove();
    }
  });

  it('does not change mode outside Export', () => {
    useUIStore.getState().setAppMode('plan');
    renderHook(() => useKeyboardShortcuts());
    pressEscape();
    expect(useUIStore.getState().appMode).toBe('plan');
  });
});
