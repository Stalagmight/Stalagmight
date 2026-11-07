import { beforeEach, describe, expect, it, vi } from 'vitest';

import i18n from '../../i18n';
import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

/**
 * #576 D39(a): a refused Repair is a coded rejection (code `EDIT_DISABLED`, #576 C4).
 * It used to resolve
 * `{ fixed: 0, remaining: 0, changed: false }`, which the UI reported as
 * "Repaired 0 issues. Save to keep the fix." for a repair that never ran. The
 * store now shows the read-only notice and re-throws, so the caller reports
 * neither a repair nor a failure.
 */
const LOCK_REFUSAL = {
  code: 'EDIT_DISABLED',
  message: 'Repair integrity is disabled: this SpeleoDB project is open read-only. Acquire the lock to edit.',
};

describe('a refused Repair', () => {
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

  it('shows the read-only notice and re-throws instead of reporting a repair', async () => {
    invokeIpcMock.mockImplementation(async (command: string) => {
      // Tauri rejects a Result<_, CommandError> command with the CommandError object.
      if (command === 'repair_project_integrity') throw LOCK_REFUSAL;
      return undefined;
    });
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});

    const outcome = await useProjectStore.getState().repairProjectIntegrity().then(
      (value) => ({ resolved: value }),
      (error: unknown) => ({ rejected: error }),
    );

    expect(outcome).toEqual({ rejected: LOCK_REFUSAL });
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(i18n.t('editGate.readOnlyNotice'), 'edit-disabled', 'warning');
    // An expected refusal, not an error: nothing marked dirty, nothing logged as a failure.
    expect(useProjectStore.getState().isDirty).toBe(false);
    expect(consoleError).not.toHaveBeenCalled();
    consoleError.mockRestore();
  });
});
