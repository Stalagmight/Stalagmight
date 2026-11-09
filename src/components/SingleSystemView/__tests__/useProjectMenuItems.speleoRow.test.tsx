/**
 * The seam left behind after the SpeleoDB submenu moved to `useSpeleoMenuItems`:
 * the hamburger keeps only the wrapper row, and it must still carry the extracted
 * rows. The rows' own gating is covered by `useSpeleoMenuItems.test.tsx`.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';

vi.mock('../../../utils/ipc', () => ({
  invokeIpc: vi.fn(),
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));

import { useProjectMenuItems } from '../useProjectMenuItems';
import { useProject } from '../../../hooks/useProject';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../../store/speleoStore';

const LINK: SpeleoLinkState = {
  permission: 'READ_AND_WRITE',
  editState: 'read-only',
  lockHolder: null,
  lockedByOther: false,
  hasUnpushedEdits: false,
  unwound: false,
  forceEditing: false,
  forceEditAllowed: false,
  undoMode: 'full',
  remoteState: 'up_to_date',
  noAccess: false,
  serverError: null,
};

const speleoRow = () => {
  const { result } = renderHook(() =>
    useProjectMenuItems({ onSave: async () => ({ success: true }) }),
  );
  return result.current.find((it) => it.id === 'speleodb-menu');
};

beforeEach(() => {
  useSpeleoStore.setState({
    link: null,
    remoteProjectId: 'p1',
    session: { loggedIn: true, userEmail: 'me@x', endpoint: 'https://x/', },
  });
});

describe('useProjectMenuItems SpeleoDB row', () => {
  it('carries a populated submenu for a remote project', () => {
    useSpeleoStore.setState({ link: LINK });
    const row = speleoRow();
    expect(row).toBeDefined();
    expect(row?.submenu?.length).toBeGreaterThan(0);
  });

  it('is absent for a local project', () => {
    expect(speleoRow()).toBeUndefined();
  });
});


describe('SpeleoDB manual save intent', () => {
  it.each([
    [false, 'speleodb-done'],
    [true, 'speleodb-close'],
  ] as const)('preserves closeAfterSave=%s while collecting the commit message', async (closeAfterSave, dialog) => {
    useSpeleoStore.setState({ link: { ...LINK, editState: 'editing', hasUnpushedEdits: true } });
    const openDialog = vi.spyOn(useUIStore.getState(), 'openDialog').mockImplementation(() => {});
    const { result } = renderHook(() => useProject());
    try {
      await act(async () => {
        await result.current.handleSaveProject({ closeAfterSave });
      });
      await waitFor(() => expect(openDialog).toHaveBeenCalledWith(
        dialog,
        ...(closeAfterSave ? [] : [{ mode: 'push' }]),
      ));
    } finally {
      openDialog.mockRestore();
    }
  });
});
