import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useSpeleoStore } from '../speleoStore';

const invokeIpcMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
}));

describe('projectStore warning edits', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'get_project_warnings') return [];
      return undefined;
    });
    useSpeleoStore.setState({
      link: {
        permission: 'READ_AND_WRITE',
        editState: 'editing',
        lockHolder: null,
        lockedByOther: false,
        hasUnpushedEdits: true,
        unwound: true,

        undoMode: 'full',

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
