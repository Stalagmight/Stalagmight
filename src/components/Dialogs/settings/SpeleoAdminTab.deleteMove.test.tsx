/**
 * [#576 SDB-DEL-D1] Delete project on SpeleoDB treats the local copy the way Unlink
 * does: once the server delete has succeeded, the same Save As (the default projects
 * folder, the project name filled in) asks where to keep the copy, and the copy is
 * moved out of the SpeleoDB cache through the Unlink move (`speleodb_unlink`), then
 * loaded there, so the session and Recent projects follow it. Cancel leaves the copy
 * unlinked in the cache; a failed delete asks nothing.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const saveDialogMock = vi.hoisted(() => vi.fn());
const documentDirMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc');
stubGetErrorMessage(errorMessageOrString);
vi.mock('../../../utils/fileDialogs', () => ({
  saveDialog: saveDialogMock,
  ensureExtension: (p: string) => (p.endsWith('.stalx') ? p : `${p}.stalx`),
  projectSaveFilters: () => [],
  PROJECT_EXTENSION: 'stalx',
}));
vi.mock('@tauri-apps/api/path', () => ({ documentDir: documentDirMock }));

import { SpeleoAdminTab } from './SpeleoAdminTab';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { invokeIpcMock, stubGetErrorMessage, errorMessageOrString } from '../../../test/factories/ipc';

const CACHE_COPY = '/home/diver/.cache/stalagmight/speleodb/p1/project.stalx';
const CHOSEN = '/home/diver/Documents/stalagmight/Cenote_Azul.stalx';

let order: string[];
let loadProject: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.clearAllMocks();
  invokeIpcMock.mockReset();
  order = [];
  documentDirMock.mockResolvedValue('/home/diver/Documents');
  saveDialogMock.mockImplementation(async () => {
    order.push('save-as');
    return CHOSEN;
  });
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    order.push(cmd);
    return cmd === 'speleodb_unlink' ? { saved_path: CHOSEN, unlinked_copy_path: null, warnings: [] } : null;
  });
  loadProject = vi.fn(async (path: string) => {
    order.push(`load ${path}`);
    return { success: true };
  });
  useUIStore.setState({ showToast: vi.fn(), closeDialog: vi.fn(), openDialog: vi.fn() } as never);
  useProjectStore.setState({
    project: { name: 'Cenote Azul', file_path: CACHE_COPY },
    loadProject,
  } as never);
  useSpeleoStore.setState({
    link: {
      permission: 'ADMIN',
      editState: 'read-only',
      lockHolder: null,
      lockedByOther: false,
      hasUnpushedEdits: false,
      unwound: false,
      remoteState: 'up_to_date',
      noAccess: false,
      serverError: null,
    } as never,
    refreshLinkStatus: vi.fn(async () => {
      order.push('refresh');
    }),
    syncProjectLink: vi.fn(),
  } as never);
});

async function deleteProject(): Promise<void> {
  render(<SpeleoAdminTab />);
  await userEvent.type(screen.getByTestId('speleodb-delete-confirm-input'), 'Cenote Azul');
  await userEvent.click(screen.getByTestId('speleodb-delete-button'));
}

describe('SpeleoAdminTab: Delete project moves the local copy out of the cache', () => {
  it('asks the Unlink Save As after the delete, then moves the copy there and loads it', async () => {
    await deleteProject();
    await waitFor(() => expect(loadProject).toHaveBeenCalled());
    expect(order).toEqual(['speleodb_delete_project', 'save-as', 'speleodb_unlink', 'refresh', `load ${CHOSEN}`]);
    // The same Save As as Unlink: the default projects folder, the project name filled in.
    expect(saveDialogMock).toHaveBeenCalledWith(
      expect.objectContaining({ defaultPath: '/home/diver/Documents/stalagmight/Cenote_Azul.stalx' }),
    );
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_unlink', { destination: CHOSEN, expectedProject: null });
  });

  it('Cancel leaves the copy where it is: nothing is moved or loaded', async () => {
    saveDialogMock.mockImplementation(async () => {
      order.push('save-as');
      return null;
    });
    await deleteProject();
    await waitFor(() => expect(saveDialogMock).toHaveBeenCalled());
    // Let the cancelled ask settle before asserting nothing followed it.
    await waitFor(() => expect(order).toEqual(['speleodb_delete_project', 'save-as']));
    expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_unlink', expect.anything());
    expect(loadProject).not.toHaveBeenCalled();
  });

  it('a failed delete asks nothing', async () => {
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      order.push(cmd);
      if (cmd === 'speleodb_delete_project') throw new Error('server 500');
      return null;
    });
    await deleteProject();
    await waitFor(() => expect(screen.getByTestId('speleodb-delete-button')).toBeEnabled());
    expect(saveDialogMock).not.toHaveBeenCalled();
    expect(order).toEqual(['speleodb_delete_project']);
  });
});
