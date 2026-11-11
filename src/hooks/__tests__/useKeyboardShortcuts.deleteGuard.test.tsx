/**
 * Delete/Backspace read-only guard (#275 canonical-gating sweep).
 *
 * The guard must block deletion in every read-only state, not just the
 * closure-adjusted view: Plan/Export (isMapReadOnly's appMode term) and a
 * locked remote SpeleoDB project (selectSpeleoReadOnly) must ALSO block it.
 * Regression coverage for the "isMapReadOnly composed without speleoReadOnly"
 * bug class documented in the #275 audit.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useKeyboardShortcuts } from '../useKeyboardShortcuts';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';

// useKeyboardShortcuts pulls in telemetry/save helpers that ultimately call the
// Tauri IPC bridge; stub it so importing the real hook here stays offline.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue({}),
}));

const READ_ONLY_LINK: SpeleoLinkState = {
  permission: 'READ_AND_WRITE',
  editState: 'read-only',
  lockHolder: 'someone-else',
  lockedByOther: true,
  hasUnpushedEdits: false,
  unwound: false,

  undoMode: 'full',
  remoteState: 'up_to_date',
  noAccess: false,
  serverError: null,
};

const STATION_SELECTION = { type: 'station' as const, stationRef: { section_id: 0, station_index: 1 } };

function pressDelete(): void {
  window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', bubbles: true }));
}

beforeEach(() => {
  useUIStore.setState({
    selection: STATION_SELECTION,
    activeDialog: null,
    dialogData: {},
    placementMode: null,
    dataTableFullScreen: false,
    activeTool: 'select',
    appMode: 'survey',
    geometryMode: 'measured',
  });
  // Confirmed-local project - the editable baseline (#270 fail-closed gate).
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

describe('useKeyboardShortcuts Delete guard (#275)', () => {
  it('opens confirm-delete for a selected station in the editable state', () => {
    renderHook(() => useKeyboardShortcuts());
    pressDelete();
    expect(useUIStore.getState().activeDialog).toBe('confirm-delete');
  });

  it('blocks delete in the closure-adjusted survey view', () => {
    useUIStore.setState({ geometryMode: 'closureAdjusted' });
    renderHook(() => useKeyboardShortcuts());
    pressDelete();
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('blocks delete in Plan mode even when geometryMode fell back to measured (the appMode trap)', () => {
    useUIStore.setState({ appMode: 'plan', geometryMode: 'measured' });
    renderHook(() => useKeyboardShortcuts());
    pressDelete();
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('blocks delete on a locked remote SpeleoDB project (survey + measured otherwise editable)', () => {
    useSpeleoStore.setState({ link: READ_ONLY_LINK });
    renderHook(() => useKeyboardShortcuts());
    pressDelete();
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('allows delete once the SpeleoDB write lock is held', () => {
    useSpeleoStore.setState({ link: { ...READ_ONLY_LINK, editState: 'editing', lockedByOther: false } });
    renderHook(() => useKeyboardShortcuts());
    pressDelete();
    expect(useUIStore.getState().activeDialog).toBe('confirm-delete');
  });
});
