/**
 * The save prompt for a file another program changed (`.stalx` v2, plan 6.5):
 * keep my version (overwrite), save as a new file, or use the file's version.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

const saveProjectAs = vi.fn();
vi.mock('../../../utils/projectActions', () => ({ saveProjectAs: () => saveProjectAs() }));
const surfaceSaveFailure = vi.fn();
vi.mock('../../../utils/saveFeedback', () => ({
  surfaceSaveFailure: (...args: unknown[]) => surfaceSaveFailure(...args),
}));

import '../../../i18n';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import SaveConflictDialog from '../SaveConflictDialog';

const saveProjectOverwriting = vi.fn();
const loadProject = vi.fn();
const closeDialog = vi.fn();
const openDialog = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  saveProjectOverwriting.mockResolvedValue({ success: true });
  loadProject.mockResolvedValue({ success: true });
  useProjectStore.setState({
    project: { file_path: '/caves/Main Sump.stalx', lines: [] },
    saveProjectOverwriting,
    loadProject,
  } as never);
  useUIStore.setState({ closeDialog, openDialog } as never);
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
