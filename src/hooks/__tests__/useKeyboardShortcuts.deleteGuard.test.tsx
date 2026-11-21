/**
 * Delete/Backspace read-only guard (#275 canonical-gating sweep).
 *
 * The guard must block deletion in every read-only state, not just the
 * closure-adjusted view: Plan/Export (isMapReadOnly's appMode term) and a
 * locked remote SpeleoDB project (selectSpeleoReadOnly) must ALSO block it.
 * Regression coverage for the "isMapReadOnly composed without speleoReadOnly"
 * bug class documented in the #275 audit.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useKeyboardShortcuts } from '../useKeyboardShortcuts';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { READ_ONLY_FILE_NOTICE } from '../../utils/editErrors';
import { notifyIfFileReadOnly } from '../useEditingBlocked';

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

const STATION_SELECTION = { type: 'station' as const, stationRef: { line_id: 0, station_index: 1 } };

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

/**
 * Demo finding I-10: in a file a newer Stalagmight made (open read-only) the
 * Delete key on a selection said nothing. It now shows the worded notice
 * Ctrl+Z gets there, once while it is on screen; the other read-only states
 * keep their own badge or banner and stay silent. Injection: dropping the
 * `notifyIfFileReadOnly()` call from the Delete branch fails the first case.
 */
describe('Delete in a read-only file (demo finding I-10)', () => {
  const notices = () => useUIStore.getState().toasts.filter((t) => t.key === TOAST_KEYS.EDIT_DISABLED);

  beforeEach(() => {
    useUIStore.getState().clearToast();
  });
  afterEach(() => {
    useProjectStore.setState({ fileReadOnlyReason: null } as never);
    useUIStore.getState().clearToast();
  });

  it('says why, once, instead of swallowing the key', () => {
    useProjectStore.setState({ fileReadOnlyReason: 'requires zz-future' } as never);
    renderHook(() => useKeyboardShortcuts());
    pressDelete();
    pressDelete();
    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(notices()).toHaveLength(1);
    expect(notices()[0]!.message).toBe(READ_ONLY_FILE_NOTICE());
    expect(notices()[0]!.kind).toBe('warning');
  });

  it('stays silent for the closure-adjusted view', () => {
    useUIStore.setState({ geometryMode: 'closureAdjusted' });
    renderHook(() => useKeyboardShortcuts());
    pressDelete();
    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(notices()).toHaveLength(0);
  });

  it('does not stack a second notice while the first is still showing', () => {
    useProjectStore.setState({ fileReadOnlyReason: 'requires zz-future' } as never);
    expect(notifyIfFileReadOnly()).toBe(true);
    // Past the toast store's one-second burst window, a plain showToast would
    // stack a second toast under the same key; the notice is still up.
    useUIStore.setState({ toasts: useUIStore.getState().toasts.map((t) => ({ ...t, refreshedAt: -1e9 })) });
    expect(notifyIfFileReadOnly()).toBe(true);
    expect(notices()).toHaveLength(1);
    // Gone (dismissed), it shows again on the next attempt.
    useUIStore.getState().clearToast();
    notifyIfFileReadOnly();
    expect(notices()).toHaveLength(1);
    // An editable file: nothing.
    useProjectStore.setState({ fileReadOnlyReason: null } as never);
    useUIStore.getState().clearToast();
    expect(notifyIfFileReadOnly()).toBe(false);
    expect(notices()).toHaveLength(0);
  });
});
