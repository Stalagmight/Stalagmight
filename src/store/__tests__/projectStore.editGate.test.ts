import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { READ_ONLY_EDIT_NOTICE } from '../../utils/editErrors';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

/**
 * #191 read-only edit gate - executeCommand failure handling. When the backend
 * refuses a mutation on a remote project opened read-only it rejects with the
 * CommandError `{code: 'EDIT_DISABLED', message}` (#576 C4), which executeCommand
 * settles into the refused outcome (not a throw). executeCommand must then:
 *  (a) ALWAYS re-sync the view to backend truth, discarding any optimistic edit -
 *      the load-bearing guard against the "shots vanish on drag" regression (a
 *      committed drag preview was left stranded because the success-only refresh
 *      never fired); this also holds on a genuine IPC throw (re-sync, then re-throw);
 *  (b) surface ONE friendly read-only toast; other failures fall to the
 *      catch-all error toast (docs/notifications.md: success:false never dies
 *      silently) - which yields while a dialog is open, because dialogs render
 *      their own inline error and must not be double-messaged.
 */
describe('projectStore executeCommand failure handling (#191 read-only gate)', () => {
  let showToast: ReturnType<typeof vi.fn>;
  let refreshProjectState: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    refreshProjectState = vi.fn().mockResolvedValue(undefined);
    useProjectStore.setState({
      refreshProjectState,
      checkProjectIntegrity: vi.fn().mockResolvedValue(undefined),
    } as Partial<ReturnType<typeof useProjectStore.getState>> as never);
    showToast = vi.fn();
    // activeDialog reset explicitly: the catch-all cases below flip it, and a
    // leaked 'edit-shot' would silently put later tests on the yield path.
    useUIStore.setState({
      showToast,
      activeDialog: null,
    } as Partial<ReturnType<typeof useUIStore.getState>> as never);
  });

  it('re-syncs the view AND shows the read-only notice on an EDIT_DISABLED rejection', async () => {
    invokeIpcMock.mockRejectedValue({
      code: 'EDIT_DISABLED',
      message: 'Move section is disabled: this SpeleoDB project is open read-only.',
    });

    const result = await useProjectStore.getState().moveSection(1, { x: 1, y: 2, z: 0 });

    expect(result.success).toBe(false);
    // (a) anti-"vanishing shots": a rejected edit re-syncs to backend truth.
    expect(refreshProjectState).toHaveBeenCalled();
    // (b) one friendly read-only toast.
    expect(showToast).toHaveBeenCalledWith(READ_ONLY_EDIT_NOTICE(), 'edit-disabled', 'warning');
  });

  it('surfaces an unclaimed failure via the catch-all error toast when no dialog is open', async () => {
    useUIStore.setState({ activeDialog: null } as never);
    invokeIpcMock.mockRejectedValue({ code: 'FAILED', message: 'Invalid offset' });

    await useProjectStore.getState().moveSection(1, { x: 1, y: 2, z: 0 });

    expect(refreshProjectState).toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith('Invalid offset', 'command-failed', 'error');
  });

  it('still toasts an unclaimed failure from the context menu (it renders no errors)', async () => {
    useUIStore.setState({ activeDialog: 'context-menu' } as never);
    invokeIpcMock.mockRejectedValue({ code: 'FAILED', message: 'Invalid offset' });

    await useProjectStore.getState().moveSection(1, { x: 1, y: 2, z: 0 });

    expect(showToast).toHaveBeenCalledWith('Invalid offset', 'command-failed', 'error');
  });

  it('re-syncs but does NOT toast a failure while a dialog is open (it owns the inline error)', async () => {
    useUIStore.setState({ activeDialog: 'edit-shot' } as never);
    invokeIpcMock.mockRejectedValue({ code: 'FAILED', message: 'Invalid offset' });

    await useProjectStore.getState().moveSection(1, { x: 1, y: 2, z: 0 });

    expect(refreshProjectState).toHaveBeenCalled();
    expect(showToast).not.toHaveBeenCalled();
  });

  it('re-syncs then re-throws on a genuine IPC rejection (no stranded optimistic edit)', async () => {
    invokeIpcMock.mockRejectedValue(new Error('transport down'));

    await expect(
      useProjectStore.getState().moveSection(1, { x: 1, y: 2, z: 0 })
    ).rejects.toThrow('transport down');

    // The throw must still re-sync (so a committed drag preview can't be stranded),
    // then propagate so callers' existing error handling is unchanged.
    expect(refreshProjectState).toHaveBeenCalled();
  });

  it('undo shows the friendly read-only notice (not the raw EDIT_DISABLED string)', async () => {
    invokeIpcMock.mockRejectedValue({
      code: 'EDIT_DISABLED',
      message: 'Undo is disabled: this SpeleoDB project is open read-only.',
    });

    await useProjectStore.getState().undo();

    // executeUndoRedo must match executeCommand: friendly notice under the shared
    // 'edit-disabled' dedup key, never the raw refusal under the undo toastId.
    expect(showToast).toHaveBeenCalledWith(READ_ONLY_EDIT_NOTICE(), 'edit-disabled', 'warning');
    expect(showToast).not.toHaveBeenCalledWith(
      expect.stringContaining('EDIT_DISABLED'),
      expect.anything()
    );
    expect(showToast).not.toHaveBeenCalledWith(
      'Undo is disabled: this SpeleoDB project is open read-only.',
      expect.anything(),
      expect.anything()
    );
  });
});
