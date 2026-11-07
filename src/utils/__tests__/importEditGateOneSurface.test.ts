/**
 * [#576 NF-W7E-2] An import the edit gate refuses is reported once.
 *
 * Importing into a project open read-only (a SpeleoDB project without the lock,
 * the Plan or Export workspace, a newer file) is refused with an edit-gate code.
 * The store's edit path shows the read-only notice for every such refusal - the
 * surface docs/notifications.md's command-failure contract gives the edit-gate
 * codes - and the import dialogs then handed the same refusal to the import
 * router, which opened the "Import failed" error dialog on top: one event, two
 * surfaces, and the second one dressed a blocked edit as a failure with a
 * report-a-bug hint. The router now leaves an edit-gate refusal to the notice and
 * just closes the import dialog.
 *
 * Store tier (the real `executeCompassImport` and router); the IPC is mocked.
 * All data is synthetic.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

import { invokeIpcMock, stubGetErrorMessage, messageOr } from '../../test/factories/ipc';

vi.mock('../ipc');
stubGetErrorMessage(messageOr('error'));

import { surfaceImportFailure, type ImportDialogUi } from '../saveFeedback';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import type { CommandErrorCode } from '../../types/ipc/CommandErrorCode';
import type { Project } from '../../types/project';

const mockUi = (): ImportDialogUi & { [K in keyof ImportDialogUi]: ReturnType<typeof vi.fn> } => ({
  closeDialog: vi.fn(),
  setView: vi.fn(),
  triggerFitToBounds: vi.fn(),
  openDialog: vi.fn(),
  showToast: vi.fn(),
});

beforeEach(() => {
  invokeIpcMock.mockReset();
  useUIStore.setState({ toasts: [], activeDialog: 'compass-import', dialogData: {}, importContext: 'existing-project' });
  useProjectStore.setState({
    project: { name: 'CaveA', created_at: '2026-01-01T10:00:00Z', file_path: '/caves/CaveA.stalx', sections: [] } as unknown as Project,
  });
});

describe('an import refused by the edit gate (#576 NF-W7E-2)', () => {
  it.each<CommandErrorCode>(['EDIT_DISABLED', 'WORKSPACE_READ_ONLY', 'NEWER_FILE_EDIT_DISABLED', 'ADOPTION_IN_PROGRESS'])(
    '%s reaches one surface: the read-only notice, not also the import-error dialog',
    async (code) => {
      invokeIpcMock.mockImplementation((command: string) => {
        if (command === 'execute_compass_import') {
          return Promise.reject({ code, message: 'execute_compass_import is disabled: read-only.' });
        }
        if (command === 'get_project_state_binary') return Promise.resolve(new ArrayBuffer(0));
        return Promise.resolve(undefined);
      });
      const ui = mockUi();

      const result = await useProjectStore.getState().executeCompassImport('/files/CaveA.mak');
      expect(result.success).toBe(false);
      if (result.success) return;
      surfaceImportFailure('compass-import', ui, { phase: 'command', error: result.error, code: result.code });

      const toasts = useUIStore.getState().toasts.map((toast) => toast.key);
      const dialogs = ui.openDialog.mock.calls.map(([name]) => name);
      console.log(`[NF-W7E-2] ${code}: toasts=${JSON.stringify(toasts)} dialogs=${JSON.stringify(dialogs)}`);
      expect(toasts).toEqual(['edit-disabled']);
      expect(dialogs).toEqual([]);
      // The import dialog's work is over either way.
      expect(ui.closeDialog).toHaveBeenCalled();
    },
  );

  it('any other refused import still opens the import-error dialog', () => {
    const ui = mockUi();
    surfaceImportFailure('compass-import', ui, { phase: 'command', error: 'Failed to parse', code: 'FAILED' });
    expect(ui.openDialog).toHaveBeenCalledWith('error', expect.objectContaining({ message: 'Failed to parse' }));
  });
});
