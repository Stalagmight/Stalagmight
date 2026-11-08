/**
 * [#579 x #576] While an update restart installs, nothing changes the project.
 *
 * The install runs off the main thread, so the window stays live for the seconds
 * it takes, after the user has already passed the unsaved-changes prompt: an edit
 * made then would be dropped by the restart without a word. The screen hold
 * (`UpdateInstallOverlay`) keeps the user out; these pin the frontend guard behind
 * it - the canonical editing gate composes the install flag, and the mutation
 * queue refuses every edit and undo/redo while it is set, so a programmatic edit
 * never reaches the backend.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';

import i18n from '../../i18n';
import { useProjectStore } from '../projectStore';
import { useSpeleoStore } from '../speleoStore';
import { useUIStore } from '../uiStore';
import { selectUpdateInstalling, useUpdateStore } from '../updateStore';
import {
  editBlockedReasonNow,
  isEditingBlockedNow,
  useEditBlockedReason,
  useEditingBlocked,
} from '../../hooks/useEditingBlocked';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

/** The commands an edit or an undo/redo would send. */
const sent = (cmd: string) => invokeIpcMock.mock.calls.filter((c) => c[0] === cmd).length;

let showToast: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.clearAllMocks();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue({ warnings: [], remote_link: null, reversed_steps: [] });
  useUpdateStore.getState().reset();
  showToast = vi.fn();
  // The one fully-editable state: survey, measured geometry, a confirmed-local
  // project, written by this build.
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured', activeDialog: null, showToast } as never);
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  useProjectStore.setState({
    project: { file_path: '/CaveA.stalx', created_at: 'p1', sections: [] } as never,
    newerSchemaVersion: null,
    refreshProjectState: vi.fn().mockResolvedValue(undefined),
    refreshUndoState: vi.fn().mockResolvedValue(undefined),
    checkProjectIntegrity: vi.fn().mockResolvedValue([]),
    isDirty: false,
  } as never);
});

describe('the editing gate while an update installs', () => {
  it('blocks editing from the moment the install starts until it fails, and names why', () => {
    const blocked = renderHook(() => useEditingBlocked());
    const reason = renderHook(() => useEditBlockedReason());
    expect(blocked.result.current).toBe(false);
    expect(isEditingBlockedNow()).toBe(false);
    expect(editBlockedReasonNow()).toBeNull();

    act(() => useUpdateStore.getState().setInstalling(true));
    expect(selectUpdateInstalling(useUpdateStore.getState())).toBe(true);
    expect(blocked.result.current).toBe(true);
    expect(isEditingBlockedNow()).toBe(true);
    expect(reason.result.current).toBe('update');
    expect(editBlockedReasonNow()).toBe('update');

    // A failed install leaves the app up, and the project editable again.
    act(() => useUpdateStore.getState().setInstalling(false));
    expect(blocked.result.current).toBe(false);
    expect(isEditingBlockedNow()).toBe(false);
    expect(reason.result.current).toBeNull();
  });

  it('is not lifted by a check reply that lands mid-install', () => {
    const store = useUpdateStore.getState();
    const seq = store.beginCheck();
    store.setAvailable(seq, { version: '9.9.9', notesUrl: 'https://example.com/notes' });
    store.setInstalling(true);

    const next = useUpdateStore.getState().beginCheck();
    useUpdateStore.getState().setUpToDate(next);

    expect(useUpdateStore.getState().status).toBe('idle');
    expect(isEditingBlockedNow()).toBe(true);
  });
});

describe('the mutation queue while an update installs', () => {
  it('refuses an edit before it is sent, silently (the install overlay says why)', async () => {
    useUpdateStore.getState().setInstalling(true);

    const result = await useProjectStore.getState().updateShot(1, 1, { length: 55 });

    expect(result.success).toBe(false);
    expect(result.error).toBe(i18n.t('updateInstall.editRefused'));
    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(useProjectStore.getState().isDirty).toBe(false);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('refuses undo and redo before they are sent', async () => {
    useUpdateStore.getState().setInstalling(true);

    const undone = await useProjectStore.getState().undo();
    const redone = await useProjectStore.getState().redo();

    expect(undone.success).toBe(false);
    expect(redone.success).toBe(false);
    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(useProjectStore.getState().isDirty).toBe(false);
  });

  it('lets edits through again once the install has failed', async () => {
    useUpdateStore.getState().setInstalling(true);
    useUpdateStore.getState().setInstalling(false);

    const result = await useProjectStore.getState().updateShot(1, 1, { length: 55 });
    await useProjectStore.getState().undo();

    expect(result.success).toBe(true);
    expect(sent('update_shot')).toBe(1);
    expect(sent('undo')).toBe(1);
  });
});
