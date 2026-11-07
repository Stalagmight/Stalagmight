/**
 * #394 F14: this dialog is a STEP in an import flow, not the New Project
 * command. Handing `create_project_full` a save path with the initial write
 * still enabled put an EMPTY `.stalx` on disk (plus a Recent Projects entry
 * pointing at it) before the import that justifies the project had run - so a
 * wizard the user then cancelled stranded both.
 *
 * The store infers `deferred = !options.savePath`, so only the "decide the
 * location later" branch was ever safe; the branch where the user picks a path
 * up front - the one the dialog encourages - was the leak. The flag has to be
 * explicit.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).translationStub('keyAndOptions'));

const dialogData: {
  dmpFile?: { sections: Array<{ shots: Array<{ shot_type: string }> }> };
  filePath?: string;
  importType?: string;
  defaultName?: string;
} = {};

const closeDialog = vi.fn();
const openDialog = vi.fn();
const setImportContext = vi.fn();

vi.mock('../../../store/uiStore', () => ({
  useUIStore: (sel: (s: unknown) => unknown) => sel({ closeDialog, dialogData, openDialog, setImportContext }),
}));

const createProject = vi.fn();
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ createProject }),
}));

vi.mock('../DialogWrapper', async () =>
  (await import('../../../test/factories/components')).dialogWrapperStub({ title: 'testid' }),
);

vi.mock('../../common/Button', async () => (await import('../../../test/factories/components')).buttonStub());

import NewProjectForImportDialog from '../NewProjectForImportDialog';

describe('NewProjectForImportDialog defers the first write (#394 F14)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    createProject.mockResolvedValue({ success: true });
    dialogData.dmpFile = { sections: [{ shots: [{ shot_type: 'Standard' }] }] };
    dialogData.filePath = '/dives/cenote.dmp';
    dialogData.importType = 'dmp';
    dialogData.defaultName = 'Cenote';
  });

  /**
   * Injection: drop `deferInitialSave: true` and the store's
   * `deferred = options?.deferInitialSave ?? !options?.savePath` resolves false
   * for this call - which is exactly the write that stranded the empty file.
   */
  it('defers even when the user picked a save path up front', async () => {
    render(<NewProjectForImportDialog />);

    fireEvent.change(screen.getByTestId('project-save-path-input'), {
      target: { value: '/caves/cenote' },
    });
    fireEvent.click(screen.getByTestId('create-and-continue-button'));

    await waitFor(() => expect(createProject).toHaveBeenCalled());
    expect(createProject.mock.calls[0]![2]).toEqual({
      savePath: '/caves/cenote.stalx',
      deferInitialSave: true,
    });
  });

  /** The no-path branch keeps behaving as it always did - now stated, not inferred. */
  it('defers with no save path too, and reserves no path', async () => {
    render(<NewProjectForImportDialog />);

    fireEvent.click(screen.getByTestId('create-and-continue-button'));

    await waitFor(() => expect(createProject).toHaveBeenCalled());
    expect(createProject.mock.calls[0]![2]).toEqual({ deferInitialSave: true });
  });
});
