/**
 * Keyboard shortcuts in the Data workspace.
 *
 * Data is editable, so undo/redo work there (and stay blocked in Plan). But its
 * map is a pane the user opens on demand, and the table owns the keys while it
 * has focus: with the pane closed, or focus inside the workspace, the map-aimed
 * shortcuts (Delete of the map selection, the tool keys, Ctrl+A) must stand
 * down - the same shape as the fullscreen data table's allow-list.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useKeyboardShortcuts } from '../useKeyboardShortcuts';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useDataWorkspaceStore } from '../../components/DataWorkspace/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue({}),
}));

const STATION_SELECTION = { type: 'station' as const, stationRef: { section_id: 0, station_index: 1 } };
const undo = vi.fn();
const redo = vi.fn();

function press(init: KeyboardEventInit, target: EventTarget = window): void {
  target.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...init }));
}

/** A stand-in for the Data workspace root with a focusable control inside. */
function mountWorkspace(): HTMLButtonElement {
  const root = document.createElement('div');
  root.setAttribute('data-testid', 'data-workspace');
  const cell = document.createElement('button');
  root.appendChild(cell);
  document.body.appendChild(root);
  return cell;
}

beforeEach(() => {
  undo.mockReset();
  redo.mockReset();
  useUIStore.setState({
    selection: STATION_SELECTION,
    activeDialog: null,
    dialogData: {},
    placementMode: null,
    dataTableFullScreen: false,
    activeTool: 'select',
    appMode: 'data',
    geometryMode: 'measured',
  });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  useProjectStore.setState({ undo, redo } as never);
  useDataWorkspaceStore.setState({ mapPaneOpen: false });
});

afterEach(() => {
  document.body.innerHTML = '';
});

describe('undo/redo by keyboard', () => {
  it('Ctrl+Z / Ctrl+Y work in Data, with the map pane closed or open', () => {
    renderHook(() => useKeyboardShortcuts());
    press({ key: 'z', ctrlKey: true });
    expect(undo).toHaveBeenCalledTimes(1);
    press({ key: 'y', ctrlKey: true });
    expect(redo).toHaveBeenCalledTimes(1);
    useDataWorkspaceStore.setState({ mapPaneOpen: true });
    press({ key: 'z', ctrlKey: true }, mountWorkspace());
    expect(undo).toHaveBeenCalledTimes(2);
  });

  it('are blocked in Plan (read-only)', () => {
    useUIStore.setState({ appMode: 'plan' });
    renderHook(() => useKeyboardShortcuts());
    press({ key: 'z', ctrlKey: true });
    press({ key: 'y', ctrlKey: true });
    expect(undo).not.toHaveBeenCalled();
    expect(redo).not.toHaveBeenCalled();
  });
});

describe('map-selection keys in Data', () => {
  it('Delete never offers to delete the map selection while the pane is closed', () => {
    renderHook(() => useKeyboardShortcuts());
    press({ key: 'Delete' });
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('Delete works on the map selection with the pane open and focus outside the table', () => {
    useDataWorkspaceStore.setState({ mapPaneOpen: true });
    renderHook(() => useKeyboardShortcuts());
    press({ key: 'Delete' });
    expect(useUIStore.getState().activeDialog).toBe('confirm-delete');
  });

  it('Delete with focus inside the Data workspace belongs to the table, not the map', () => {
    useDataWorkspaceStore.setState({ mapPaneOpen: true });
    renderHook(() => useKeyboardShortcuts());
    press({ key: 'Delete' }, mountWorkspace());
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('a tool key does nothing with the pane closed or focus in the table, and works on the open pane', () => {
    renderHook(() => useKeyboardShortcuts());
    press({ key: 'm' });
    expect(useUIStore.getState().activeTool).toBe('select');
    useDataWorkspaceStore.setState({ mapPaneOpen: true });
    press({ key: 'm' }, mountWorkspace());
    expect(useUIStore.getState().activeTool).toBe('select');
    press({ key: 'm' });
    expect(useUIStore.getState().activeTool).toBe('merge');
  });

  it('Ctrl+A does not select every map station while the pane is closed', () => {
    useProjectStore.setState({ stations: [{ station_ref: { section_id: 0, station_index: 0 } }] } as never);
    useUIStore.setState({ showStations: true, multiSelection: [] });
    renderHook(() => useKeyboardShortcuts());
    press({ key: 'a', ctrlKey: true });
    expect(useUIStore.getState().multiSelection).toHaveLength(0);
    // The same key on the open pane is the map's select-all again.
    useDataWorkspaceStore.setState({ mapPaneOpen: true });
    press({ key: 'a', ctrlKey: true });
    expect(useUIStore.getState().multiSelection).toHaveLength(1);
  });
});

describe('outside Data nothing changes', () => {
  it('Survey keeps Delete and the tool keys regardless of the Data pane flag', () => {
    useUIStore.setState({ appMode: 'survey' });
    renderHook(() => useKeyboardShortcuts());
    press({ key: 'm' });
    expect(useUIStore.getState().activeTool).toBe('merge');
    useUIStore.setState({ activeTool: 'select' });
    press({ key: 'Delete' });
    expect(useUIStore.getState().activeDialog).toBe('confirm-delete');
  });
});
