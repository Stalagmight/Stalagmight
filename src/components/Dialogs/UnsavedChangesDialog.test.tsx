import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const performQuitMock = vi.hoisted(() => vi.fn());
const handleSaveProjectMock = vi.hoisted(() => vi.fn());
// The quit continuation: a SpeleoDB project hands its lock back, then quits.
vi.mock('../../utils/quit', () => ({ quitAfterHandingBackLock: performQuitMock, abandonUpdateRestart: vi.fn() }));
vi.mock('../../hooks/useProject', () => ({ useProject: () => ({ handleSaveProject: handleSaveProjectMock }) }));
vi.mock('../../utils/ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));

import UnsavedChangesDialog from './UnsavedChangesDialog';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';

const readOnlyLink: SpeleoLinkState = {
  permission: 'READ_AND_WRITE', editState: 'read-only', lockHolder: null, lockedByOther: false,
  hasUnpushedEdits: true, unwound: false, undoMode: 'full', remoteState: 'up_to_date', noAccess: false, serverError: null,
};

let saveProject: ReturnType<typeof vi.fn>;
let closeProject: ReturnType<typeof vi.fn>;
let loadProject: ReturnType<typeof vi.fn>;

beforeEach(() => {
  performQuitMock.mockReset();
  handleSaveProjectMock.mockReset().mockResolvedValue(null);
  saveProject = vi.fn(async () => {
    useProjectStore.setState({ isDirty: false });
    return { success: true };
  });
  closeProject = vi.fn(async () => ({ success: true }));
  loadProject = vi.fn(async () => ({ success: true }));
  useProjectStore.setState({
    project: { file_path: '/synthetic/cache/project.stalx' } as never,
    isDirty: true, saveProject, closeProject, loadProject,
  } as never);
  useSpeleoStore.setState({ link: readOnlyLink });
  useUIStore.setState({ activeDialog: 'unsaved-changes', dialogData: {}, setView: vi.fn() as never });
});

// frontend#6: a SpeleoDB project here is one we cannot upload (no lock held), so
// "save" must be the local save - never the upload dialog, never discard-only.
describe('UnsavedChangesDialog for a SpeleoDB project that cannot be uploaded', () => {
  it('Save & Quit saves on this computer and quits, without routing to the upload dialog', async () => {
    useUIStore.setState({ dialogData: { pendingQuit: true } });
    render(<UnsavedChangesDialog />);
    expect(screen.getByTestId('unsaved-changes-save-quit')).toHaveTextContent('Save on this computer');
    await userEvent.click(screen.getByTestId('unsaved-changes-save-quit'));
    await waitFor(() => expect(performQuitMock).toHaveBeenCalledOnce());
    expect(saveProject).toHaveBeenCalledOnce();
    expect(handleSaveProjectMock).not.toHaveBeenCalled();
  });

  it('a failed local save does not quit', async () => {
    saveProject.mockResolvedValue({ success: false, error: 'Synthetic disk failure' });
    useUIStore.setState({ dialogData: { pendingQuit: true }, openDialog: vi.fn() as never });
    render(<UnsavedChangesDialog />);
    await userEvent.click(screen.getByTestId('unsaved-changes-save-quit'));
    await waitFor(() => expect(saveProject).toHaveBeenCalled());
    expect(performQuitMock).not.toHaveBeenCalled();
  });

  it.each([
    ['close', {}, () => expect(closeProject).toHaveBeenCalledOnce()],
    ['open another project', { pendingOpenPath: '/synthetic/other.stalx' }, () => expect(loadProject).toHaveBeenCalledWith('/synthetic/other.stalx')],
  ] as const)('%s offers Save on this computer, which saves and then carries on', async (_name, data, carriedOn) => {
    useUIStore.setState({ dialogData: data });
    render(<UnsavedChangesDialog />);
    await userEvent.click(screen.getByTestId('unsaved-changes-save-local'));
    await waitFor(carriedOn);
    expect(saveProject).toHaveBeenCalledOnce();
  });

  it('a project without SpeleoDB keeps the plain choices', () => {
    useSpeleoStore.setState({ link: null });
    render(<UnsavedChangesDialog />);
    expect(screen.queryByTestId('unsaved-changes-save-local')).toBeNull();
    expect(screen.getByTestId('unsaved-changes-discard')).toHaveAttribute('data-primary-action', '');
  });
});
