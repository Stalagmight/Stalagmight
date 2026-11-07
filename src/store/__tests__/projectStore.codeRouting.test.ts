/**
 * [#576 C4, A6] Every store route decides on the refusal's CODE, never on its text.
 *
 * Each case here is one of the string markers the frontend used to match with
 * `startsWith` / `includes` / a regex (DESIGN section 2), now the backend's
 * `CommandErrorCode`. The chain runs end to end: the real `invokeIpc` (only Tauri's
 * `invoke` is mocked) wraps the rejection, the store settles it into the refused
 * outcome, and the route reads its code. A message that still LOOKS coded proves
 * the text is not read.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import i18n from '../../i18n';
import { useProjectStore } from '../projectStore';
import { useSpeleoStore } from '../speleoStore';
import { useUIStore } from '../uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import type { CommandError } from '../../types/ipc/CommandError';
import type { CommandErrorCode } from '../../types/ipc/CommandErrorCode';

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', async () => (await import('../../test/factories/tauri')).tauriCore(invokeMock));

/** `command` rejects with `failure`; every other command answers nothing. */
function refuse(command: string, failure: CommandError) {
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === command) throw failure;
    return null;
  });
}

let showToast: ReturnType<typeof vi.fn>;
let openDialog: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeMock.mockReset();
  showToast = vi.fn();
  openDialog = vi.fn();
  useUIStore.setState({ showToast, openDialog, activeDialog: null } as never);
  useProjectStore.setState({
    project: { file_path: '/caves/a.stalx', created_at: 'p1', sections: [], zero_datums: [] } as never,
    isDirty: false,
    autosaveFailure: null,
    autosaveFailureKind: null,
    integrityIssues: [],
    integritySaveBlocked: false,
    refreshProjectState: vi.fn().mockResolvedValue(undefined),
    refreshUndoState: vi.fn().mockResolvedValue(undefined),
    checkProjectIntegrity: vi.fn().mockResolvedValue([]),
  } as never);
  useSpeleoStore.setState({ divergence: null });
});

describe('an edit refused by the gate gets its own notice, by code', () => {
  it.each([
    ['EDIT_DISABLED', 'editGate.readOnlyNotice'],
    ['WORKSPACE_READ_ONLY', 'editGate.workspaceReadOnlyNotice'],
    ['NEWER_FILE_EDIT_DISABLED', 'editGate.newerFileNotice'],
  ] as const)('%s -> %s, once, and nothing else', async (code, notice) => {
    refuse('move_section', { code, message: 'Move section is disabled.' });

    const result = await useProjectStore.getState().moveSection(1, { x: 1, y: 0, z: 0 });

    expect(result).toMatchObject({ success: false, code, error: 'Move section is disabled.' });
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(i18n.t(notice), TOAST_KEYS.EDIT_DISABLED, 'warning');
    expect(useProjectStore.getState().isDirty).toBe(false);
  });

  it('routes EDIT_REVERIFY_DIVERGED to the divergence prompt, not a notice', async () => {
    refuse('move_section', { code: 'EDIT_REVERIFY_DIVERGED', message: 'The lock changed.' });

    await useProjectStore.getState().moveSection(1, { x: 1, y: 0, z: 0 });

    expect(useSpeleoStore.getState().divergence).toBe('reverify');
    expect(showToast).not.toHaveBeenCalled();
  });

  it('never reads the message: a coded-looking FAILED is the catch-all, verbatim', async () => {
    refuse('move_section', { code: 'FAILED', message: 'EDIT_DISABLED: looks coded, is not' });

    await useProjectStore.getState().moveSection(1, { x: 1, y: 0, z: 0 });

    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith('EDIT_DISABLED: looks coded, is not', TOAST_KEYS.COMMAND_FAILED, 'error');
    expect(useSpeleoStore.getState().divergence).toBeNull();
  });
});

describe('a refused save is classified by its code', () => {
  it.each([
    ['FS_PERMISSION_DENIED', 'permission'],
    ['FS_PATH_NOT_FOUND', 'path-not-found'],
    ['FS_PATH_TOO_LONG', 'path-too-long'],
    ['IGNORES_UNREADABLE', 'ignores-unreadable'],
    ['SPELEODB_LINK_UNREADABLE', 'side-table-unreadable'],
    ['UNDO_MARKERS_UNREADABLE', 'side-table-unreadable'],
    ['NEWER_FILE_READ_ONLY', 'newer-file'],
    ['PROJECT_SETTINGS_UNREADABLE', 'settings-unreadable'],
    ['NEWER_FILE_EDIT_DISABLED', 'other'],
    ['FAILED', 'other'],
  ] as const)('an autosave refused with %s records the %s kind', async (code: CommandErrorCode, kind) => {
    useProjectStore.setState({ isDirty: true } as never);
    refuse('save_project', { code, message: "Can't save: a sentence of its own." });

    const result = await useProjectStore.getState().saveProject(undefined, true);

    expect(result).toMatchObject({ success: false, code });
    expect(useProjectStore.getState().autosaveFailureKind).toBe(kind);
    expect(useProjectStore.getState().autosaveFailure).toBeTruthy();
    // The background save never opens a dialog, whatever the code.
    expect(openDialog).not.toHaveBeenCalled();
  });

  it('opens the folder-access guidance for FS_PERMISSION_DENIED on a manual save, and only for it', async () => {
    refuse('save_project', { code: 'FS_PERMISSION_DENIED', message: 'denied' });
    await useProjectStore.getState().saveProject();
    expect(openDialog).toHaveBeenCalledWith('save-permission', expect.objectContaining({ context: 'save' }));

    openDialog.mockClear();
    refuse('save_project', { code: 'FAILED', message: 'FS_PERMISSION_DENIED: looks coded, is not' });
    await useProjectStore.getState().saveProject();
    expect(openDialog).not.toHaveBeenCalled();
  });
});

describe('a failed undo/redo: a step ran, or none did (by code)', () => {
  it('HISTORY_STEP_FAILED of a jump changed the project; its message is the toast', async () => {
    refuse('undo_to', {
      code: 'HISTORY_STEP_FAILED',
      message: 'Undo failed at step 2 of 3: boom',
      history_step: { failed_step: 2, total_steps: 3 },
    });

    const result = await useProjectStore.getState().undoTo(2);

    expect(result).toMatchObject({ success: false, code: 'HISTORY_STEP_FAILED' });
    expect(useProjectStore.getState().isDirty).toBe(true);
    expect(showToast).toHaveBeenCalledWith('Undo failed at step 2 of 3: boom', TOAST_KEYS.UNDO_FAILED, 'error');
  });

  it('HISTORY_STEP_FAILED of a single step (1 of 1) changed the project too', async () => {
    refuse('redo', {
      code: 'HISTORY_STEP_FAILED',
      message: 'Failed to parse redo data',
      history_step: { failed_step: 1, total_steps: 1 },
    });

    await useProjectStore.getState().redo();

    expect(useProjectStore.getState().isDirty).toBe(true);
    expect(showToast).toHaveBeenCalledWith('Failed to parse redo data', TOAST_KEYS.REDO_FAILED, 'error');
  });

  it.each([
    ['NOTHING_TO_UNDO', 'Nothing to undo'],
    ['NOTHING_TO_REDO', 'Nothing to redo'],
  ] as const)('%s ran no step: nothing marked, its message shown', async (code, message) => {
    refuse(code === 'NOTHING_TO_UNDO' ? 'undo' : 'redo', { code, message });

    await (code === 'NOTHING_TO_UNDO' ? useProjectStore.getState().undo() : useProjectStore.getState().redo());

    expect(useProjectStore.getState().isDirty).toBe(false);
    expect(showToast).toHaveBeenCalledWith(
      message,
      code === 'NOTHING_TO_UNDO' ? TOAST_KEYS.UNDO_FAILED : TOAST_KEYS.REDO_FAILED,
      'error',
    );
  });

  it('a message that merely reads like a jump failure is not one', async () => {
    refuse('undo_to', { code: 'FAILED', message: 'Undo failed at step 2 of 3: but uncoded' });

    await useProjectStore.getState().undoTo(2);

    expect(useProjectStore.getState().isDirty).toBe(false);
  });
});
