import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useSpeleoStore } from '../speleoStore';

const invokeIpcMock = vi.hoisted(() => vi.fn());

// [#576 D22] The link bits after a resolve/unresolve are the backend's, on the
// command's answer, applied by the real `invokeIpc`: only Tauri's `invoke` is mocked.
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeIpcMock }));

describe('projectStore warning edits', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'get_project_warnings') return [];
      // A forward edit: the backend dirtied the copy and forgot its Unwind position.
      if (command === 'resolve_warning' || command === 'unresolve_warning') {
        return { warnings: [], remote_link: { has_unpushed_edits: true, unwound: false } };
      }
      return undefined;
    });
    useSpeleoStore.setState({
      linkPath: null,
      link: {
        permission: 'READ_AND_WRITE',
        editState: 'editing',
        lockHolder: null,
        lockedByOther: false,
        hasUnpushedEdits: true,
        unwound: true,
        forceEditing: false,
        undoMode: 'full',
        forceEditAllowed: false,
        remoteState: 'up_to_date',
        noAccess: false,
        serverError: null,
      },
    });
  });

  it('mirrors a resolved warning as a forward edit immediately', async () => {
    await expect(
      useProjectStore.getState().resolveWarning(1, 2, 'distance', 'checked'),
    ).resolves.toBe(true);

    expect(useSpeleoStore.getState().link?.unwound).toBe(false);
  });

  it('mirrors an unresolved warning as a forward edit immediately', async () => {
    await useProjectStore.getState().unresolveWarning(1, 2, 'distance');

    expect(useSpeleoStore.getState().link?.unwound).toBe(false);
  });
});
