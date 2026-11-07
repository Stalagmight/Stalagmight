import { beforeEach, describe, expect, it, vi } from 'vitest';

import i18n from '../../i18n';
import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

/**
 * #576 C5: what the user sees when the backend refuses an edit in Plan or Export.
 *
 * While the workspace is Plan or Export the backend refuses every survey edit with
 * `EDIT_DISABLED: WORKSPACE_READ_ONLY: ...`. That is the read-only WORKSPACE, not a
 * SpeleoDB lock, so the notice must point at Survey - not at "Start editing", which
 * would not help.
 */
/** The backend's read-only-workspace refusal (#576 C4): its code and its sentence. */
const WORKSPACE_REFUSAL = {
  code: 'WORKSPACE_READ_ONLY',
  message:
  'Update shot 1:1 is disabled: the Plan workspace is read-only. ' +
  'Switch to Survey to edit.',
};

describe('a read-only-workspace refusal reaches the user as its own notice', () => {
  let showToast: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    useProjectStore.setState({
      project: { file_path: '/CaveA.stalx', created_at: 'p1', sections: [] } as never,
      refreshProjectState: vi.fn().mockResolvedValue(undefined),
      refreshUndoState: vi.fn().mockResolvedValue(undefined),
      checkProjectIntegrity: vi.fn().mockResolvedValue([]),
      isDirty: false,
    } as Partial<ReturnType<typeof useProjectStore.getState>> as never);
    showToast = vi.fn();
    useUIStore.setState({ showToast, activeDialog: null } as Partial<ReturnType<typeof useUIStore.getState>> as never);
  });

  it('a survey edit refused by the read-only workspace shows the switch-to-Survey notice', async () => {
    invokeIpcMock.mockRejectedValue(WORKSPACE_REFUSAL);

    const result = await useProjectStore.getState().updateShot(1, 1, { length: 55 });

    expect(result.success).toBe(false);
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(i18n.t('editGate.workspaceReadOnlyNotice'), 'edit-disabled', 'warning');
    // Not the SpeleoDB lock notice ("click Start editing"), and never the raw code.
    expect(showToast).not.toHaveBeenCalledWith(i18n.t('editGate.readOnlyNotice'), expect.anything(), expect.anything());
    expect(useProjectStore.getState().isDirty).toBe(false);
  });

  it('undo refused by the read-only workspace shows the same notice', async () => {
    invokeIpcMock.mockRejectedValue({
      code: 'WORKSPACE_READ_ONLY',
      message: 'Undo is disabled: the Export workspace is read-only.',
    });

    await useProjectStore.getState().undo();

    expect(showToast).toHaveBeenCalledWith(i18n.t('editGate.workspaceReadOnlyNotice'), 'edit-disabled', 'warning');
  });

  it('a Repair refused by the read-only workspace names the workspace', async () => {
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'repair_project_integrity') {
        throw { code: 'WORKSPACE_READ_ONLY', message: 'Repair integrity is disabled: the Plan workspace is read-only.' };
      }
      return undefined;
    });

    await expect(useProjectStore.getState().repairProjectIntegrity()).rejects.toMatchObject({ code: 'WORKSPACE_READ_ONLY' });

    expect(showToast).toHaveBeenCalledWith(i18n.t('editGate.workspaceReadOnlyNotice'), 'edit-disabled', 'warning');
  });
});
