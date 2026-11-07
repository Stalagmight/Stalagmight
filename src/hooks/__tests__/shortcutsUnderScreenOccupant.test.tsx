/**
 * #576 D39(c): global shortcuts stay quiet while a top-layer prompt that is not
 * the `activeDialog` is on screen.
 *
 * The held-lock prompt, the telemetry opt-in, the API-key storage choice and the
 * dump picker render outside the single `activeDialog` slot and register in the
 * `screenOccupants` counter instead. Both shortcut hooks only checked
 * `activeDialog`, so with one of those prompts up, Ctrl+Z undid a survey edit
 * behind it, Ctrl+O / Ctrl+N started a project switch under it, Escape cleared the
 * selection beneath it, and on the Global View Enter opened the selected project.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { renderHook } from '@testing-library/react';

const runFileShortcutAction = vi.hoisted(() => vi.fn());
vi.mock('../../utils/fileShortcutActions', () => ({ runFileShortcutAction }));

import { useKeyboardShortcuts } from '../useKeyboardShortcuts';
import { useGlobalViewShortcuts } from '../useGlobalViewShortcuts';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import type { ProjectMetadata } from '../../types/project';

const STATION_SELECTION = { type: 'station' as const, stationRef: { section_id: 0, station_index: 1 } };

function press(key: string, init: KeyboardEventInit = {}, target: Window | Document = window): void {
  target.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...init }));
}

let undo: ReturnType<typeof vi.fn>;

beforeEach(() => {
  runFileShortcutAction.mockReset();
  undo = vi.fn();
  useProjectStore.setState({ undo });
  useUIStore.setState({
    activeDialog: null,
    dialogData: {},
    placementMode: null,
    dataTableFullScreen: false,
    appMode: 'survey',
    geometryMode: 'measured',
    screenOccupants: 0,
  });
  useUIStore.getState().setSelection(STATION_SELECTION);
});

afterEach(() => {
  useUIStore.setState({ screenOccupants: 0 });
});

describe('survey view shortcuts under a screen occupant (D39(c))', () => {
  it('Ctrl+Z, Ctrl+O, Ctrl+N and Escape do nothing while a prompt occupies the screen', () => {
    renderHook(() => useKeyboardShortcuts());
    useUIStore.getState().addScreenOccupant();

    press('z', { ctrlKey: true });
    press('o', { ctrlKey: true });
    press('n', { ctrlKey: true });
    press('Escape');

    expect(undo, 'Ctrl+Z undid a survey edit behind the prompt').not.toHaveBeenCalled();
    expect(runFileShortcutAction, 'Ctrl+O / Ctrl+N started a file action under the prompt').not.toHaveBeenCalled();
    expect(useUIStore.getState().selection, 'Escape cleared the selection beneath the prompt').toEqual(
      STATION_SELECTION
    );
  });

  it('works again once the prompt is gone', () => {
    renderHook(() => useKeyboardShortcuts());
    useUIStore.getState().addScreenOccupant();
    useUIStore.getState().removeScreenOccupant();

    press('z', { ctrlKey: true });
    press('o', { ctrlKey: true });

    expect(undo).toHaveBeenCalledTimes(1);
    expect(runFileShortcutAction).toHaveBeenCalledWith('open_file');
  });
});

describe('Global View shortcuts under a screen occupant (D39(c))', () => {
  const project = { file_path: '/caves/CaveA.stalx', name: 'CaveA', gps_center: null } as unknown as ProjectMetadata;

  function renderGlobalView() {
    const handleProjectOpen = vi.fn();
    renderHook(() =>
      useGlobalViewShortcuts({
        selectedProjectId: project.file_path,
        multiSelectedIds: new Set(),
        activeTab: 'list',
        recentProjects: [project],
        loading: { isLoading: false },
        contextMenu: { visible: false, x: 0, y: 0, projectPath: null },
        mapRef: { current: null },
        setSelectedProjectId: vi.fn(),
        setMultiSelectedIds: vi.fn(),
        setContextMenu: vi.fn(),
        handleProjectOpen,
        handleRemoveFromRecent: vi.fn(),
      })
    );
    return handleProjectOpen;
  }

  it('Enter and Ctrl+N do nothing while a prompt occupies the screen', () => {
    const handleProjectOpen = renderGlobalView();
    useUIStore.getState().addScreenOccupant();

    press('Enter', {}, document);
    press('n', { ctrlKey: true }, document);

    expect(handleProjectOpen, 'Enter opened the selected project under the prompt').not.toHaveBeenCalled();
    expect(runFileShortcutAction, 'Ctrl+N started a new project under the prompt').not.toHaveBeenCalled();
  });

  it('Enter opens the selected project again once the prompt is gone', () => {
    const handleProjectOpen = renderGlobalView();

    press('Enter', {}, document);

    expect(handleProjectOpen).toHaveBeenCalledWith(project.file_path);
  });
});
