/**
 * The save prompt for a file another program changed (`.stalx` v2, plan 6.5):
 * keep my version (overwrite), save as a new file, or use the file's version;
 * and for one it moved or deleted (`FILE_MISSING`), only the first two.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

const saveProjectAs = vi.fn();
vi.mock('../../../utils/projectActions', () => ({ saveProjectAs: () => saveProjectAs() }));
const surfaceSaveFailure = vi.fn();
vi.mock('../../../utils/saveFeedback', () => ({
  surfaceSaveFailure: (...args: unknown[]) => surfaceSaveFailure(...args),
}));
const invokeIpc = vi.fn();
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: (...args: unknown[]) => invokeIpc(...args),
}));

import '../../../i18n';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import SaveConflictDialog from '../SaveConflictDialog';

const saveProjectOverwriting = vi.fn();
const loadProject = vi.fn();
const closeProject = vi.fn();
const closeDialog = vi.fn();
const openDialog = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  saveProjectOverwriting.mockResolvedValue({ success: true });
  loadProject.mockResolvedValue({ success: true });
  closeProject.mockResolvedValue({ success: true });
  // The file opens as it is now: nothing refuses the reopen.
  invokeIpc.mockImplementation(async (command: string) => {
    if (command === 'project_file_reload_refusal') return null;
    throw new Error(`unexpected IPC ${command}`);
  });
  useProjectStore.setState({
    project: { file_path: '/caves/Main Sump.stalx', lines: [] },
    saveProjectOverwriting,
    loadProject,
    closeProject,
  } as never);
  useUIStore.setState({
    closeDialog,
    openDialog,
    dialogData: { error: 'EXTERNALLY_MODIFIED: /caves/Main Sump.stalx was changed by another program since it was opened' },
  } as never);
  useSpeleoStore.setState({ link: null } as never);
});

describe('SaveConflictDialog', () => {
  it('names the file and offers the three answers', () => {
    render(<SaveConflictDialog />);
    expect(screen.getByTestId('save-conflict-lead')).toHaveTextContent(
      'Another program changed Main Sump.stalx after Stalagmight opened it.'
    );
    expect(screen.getByTestId('save-conflict-overwrite')).toHaveTextContent('Keep my version');
    expect(screen.getByTestId('save-conflict-save-as')).toHaveTextContent('Save as a new file...');
    expect(screen.getByTestId('save-conflict-reload')).toHaveTextContent("Use the file's version");
  });

  it('overwrites the file with "Keep my version"', async () => {
    render(<SaveConflictDialog />);
    fireEvent.click(screen.getByTestId('save-conflict-overwrite'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(saveProjectOverwriting).toHaveBeenCalledOnce();
    expect(surfaceSaveFailure).not.toHaveBeenCalled();
  });

  it('routes another failure of the overwrite to the usual save failure surface', async () => {
    saveProjectOverwriting.mockResolvedValue({ success: false, error: 'FS_PATH_NOT_FOUND: gone' });
    render(<SaveConflictDialog />);
    fireEvent.click(screen.getByTestId('save-conflict-overwrite'));
    await waitFor(() => expect(surfaceSaveFailure).toHaveBeenCalledWith('FS_PATH_NOT_FOUND: gone', openDialog));
  });

  it('steps aside for Save As, and reopens the file for "Use the file\'s version"', async () => {
    const { unmount } = render(<SaveConflictDialog />);
    fireEvent.click(screen.getByTestId('save-conflict-save-as'));
    await waitFor(() => expect(saveProjectAs).toHaveBeenCalledOnce());
    expect(closeDialog).toHaveBeenCalled();
    unmount();

    render(<SaveConflictDialog />);
    fireEvent.click(screen.getByTestId('save-conflict-reload'));
    await waitFor(() => expect(loadProject).toHaveBeenCalledWith('/caves/Main Sump.stalx'));
    // The open path would be refused as already open: the session closes first,
    // dropping the unsaved changes, and only then is the file read again. And
    // before the close, the backend confirmed the file opens as it is now.
    expect(closeProject).toHaveBeenCalledOnce();
    expect(closeProject.mock.invocationCallOrder[0] ?? Infinity).toBeLessThan(loadProject.mock.invocationCallOrder[0] ?? -Infinity);
    expect(invokeIpc).toHaveBeenCalledWith('project_file_reload_refusal', undefined, expect.anything());
    expect(invokeIpc.mock.invocationCallOrder[0] ?? Infinity).toBeLessThan(closeProject.mock.invocationCallOrder[0] ?? -Infinity);
  });

  /**
   * Pre-PR review finding 1: "Use the file's version" closed the session (the
   * unsaved changes went with it) and THEN found the file would not open,
   * leaving neither version. It closes only for a file the backend says opens.
   * Injection: dropping the `project_file_reload_refusal` check calls
   * closeProject here.
   */
  it('keeps the session and the changes when the file would not open again', async () => {
    invokeIpc.mockResolvedValue('File not found: /caves/Main Sump.stalx');
    render(<SaveConflictDialog />);
    fireEvent.click(screen.getByTestId('save-conflict-reload'));
    await waitFor(() =>
      expect(screen.getByTestId('save-conflict-error')).toHaveTextContent(
        "The file can't be opened as it is now, so your unsaved changes were kept: File not found: /caves/Main Sump.stalx"
      )
    );
    expect(closeProject).not.toHaveBeenCalled();
    expect(loadProject).not.toHaveBeenCalled();
    expect(closeDialog).not.toHaveBeenCalled();
  });

  /**
   * Pre-PR review finding 1: a save refused because the file was moved or
   * deleted (`FILE_MISSING` after the guard's code) said "Another program
   * changed ..." and offered "Use the file's version", whose close dropped the
   * edits before the reopen failed. It says what happened and offers only the
   * two answers that keep the edits. Injection: deriving `missing` as false
   * shows the reload button (and the changed-file wording) here.
   */
  it('says a moved or deleted file is gone, offers no reopen, and never closes the session', () => {
    useUIStore.setState({
      dialogData: {
        error: 'EXTERNALLY_MODIFIED: FILE_MISSING: /caves/Main Sump.stalx was moved or deleted by another program since it was opened',
      },
    } as never);
    render(<SaveConflictDialog />);
    expect(screen.getByTestId('save-conflict-lead')).toHaveTextContent(
      'Main Sump.stalx was moved or deleted by another program after Stalagmight opened it.'
    );
    expect(screen.getByTestId('save-conflict-lead')).not.toHaveTextContent('Another program changed');
    expect(screen.getByText('The project file was moved or deleted')).toBeInTheDocument();
    expect(screen.queryByTestId('save-conflict-reload')).toBeNull();
    expect(screen.getByTestId('save-conflict-overwrite')).toHaveTextContent('Keep my version');
    expect(screen.getByTestId('save-conflict-save-as')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('save-conflict-cancel'));
    expect(closeProject).not.toHaveBeenCalled();
    expect(loadProject).not.toHaveBeenCalled();
  });

  it('keeps the prompt and reads nothing when the close fails', async () => {
    closeProject.mockResolvedValue({ success: false, error: 'Already closing' });
    render(<SaveConflictDialog />);
    fireEvent.click(screen.getByTestId('save-conflict-reload'));
    await waitFor(() => expect(screen.getByTestId('save-conflict-error')).toHaveTextContent('Already closing'));
    expect(loadProject).not.toHaveBeenCalled();
    expect(closeDialog).not.toHaveBeenCalled();
  });

  it('offers only the overwrite for a SpeleoDB copy, whose reload is the sync\'s', () => {
    useSpeleoStore.setState({ link: { projectId: 'p' } } as never);
    render(<SaveConflictDialog />);
    expect(screen.getByTestId('save-conflict-overwrite')).toBeInTheDocument();
    expect(screen.queryByTestId('save-conflict-save-as')).toBeNull();
    expect(screen.queryByTestId('save-conflict-reload')).toBeNull();
  });

  it('decides later without saving anything', () => {
    render(<SaveConflictDialog />);
    fireEvent.click(screen.getByTestId('save-conflict-cancel'));
    expect(closeDialog).toHaveBeenCalled();
    expect(saveProjectOverwriting).not.toHaveBeenCalled();
  });
});
