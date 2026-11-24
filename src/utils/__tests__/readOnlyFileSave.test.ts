/**
 * Demo issue N-1: in a file made with a newer Stalagmight (open read-only),
 * Save and Ctrl+S (both `dispatchProjectSave`) raised the "Save Failed ...
 * report it as a bug" dialog, while a typed key or Ctrl+Z got the calm worded
 * notice. This version saves such a file nowhere: Save says why with that same
 * notice, once, and sends no save; Save As says so before any save dialog
 * opens (the backend refuses every destination, since its encoder would drop
 * what the newer version stored). An editable file saves as before.
 *
 * Injection: dropping the `notifyIfFileReadOnly` gate from dispatchProjectSave
 * (or from saveProjectAs) fails the first (or the second) case.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));
const saveDialog = vi.hoisted(() => vi.fn(async () => '/synthetic/elsewhere.stalx'));
vi.mock('../fileDialogs', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../fileDialogs')>()),
  saveDialog,
}));
vi.mock('@tauri-apps/plugin-fs', () => ({ exists: vi.fn(async () => false) }));

import { dispatchProjectSave } from '../dispatchProjectSave';
import { saveProjectAs } from '../projectActions';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import i18n from '../../i18n';

const saveProject = vi.fn(async () => ({ success: true }));
const openDialog = vi.fn();

const notices = () => useUIStore.getState().toasts.filter((toast) => toast.key === 'edit-disabled');

beforeEach(() => {
  vi.clearAllMocks();
  useUIStore.getState().clearToast();
  useUIStore.setState({ openDialog } as never);
  useSpeleoStore.setState({ link: null, session: null } as never);
  useProjectStore.setState({
    project: { file_path: '/synthetic/future.stalx' } as never,
    isDirty: false,
    fileReadOnlyReason: 'requires zz-future',
    saveProject: saveProject as never,
  } as never);
});

describe('saving a file made with a newer Stalagmight', () => {
  it('Save (and Ctrl+S) says why, once, and sends no save', async () => {
    expect(await dispatchProjectSave()).toBeNull();
    expect(await dispatchProjectSave()).toBeNull();

    expect(saveProject).not.toHaveBeenCalled();
    expect(openDialog).not.toHaveBeenCalled();
    expect(notices()).toHaveLength(1);
    expect(notices()[0]?.kind).toBe('warning');
    expect(notices()[0]?.message).toBe(i18n.t('editGate.readOnlyFileNotice'));
  });

  it('Save As says why before any save dialog opens', async () => {
    expect(await saveProjectAs()).toBeNull();

    expect(saveDialog).not.toHaveBeenCalled();
    expect(saveProject).not.toHaveBeenCalled();
    expect(openDialog).not.toHaveBeenCalled();
    expect(notices()).toHaveLength(1);
  });

  it('an editable file saves, and saves as, as before', async () => {
    useProjectStore.setState({ fileReadOnlyReason: null } as never);

    await dispatchProjectSave();
    await saveProjectAs();

    expect(saveProject).toHaveBeenCalledTimes(2);
    expect(saveProject).toHaveBeenLastCalledWith('/synthetic/elsewhere.stalx');
    expect(saveDialog).toHaveBeenCalledOnce();
    expect(notices()).toHaveLength(0);
  });
});
