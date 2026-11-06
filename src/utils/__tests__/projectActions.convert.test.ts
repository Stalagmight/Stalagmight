import { beforeEach, describe, expect, it, vi } from 'vitest';

const saveDialogMock = vi.hoisted(() => vi.fn());
const surfaceSaveFailureMock = vi.hoisted(() => vi.fn());
const surfaceSaveWarningsMock = vi.hoisted(() => vi.fn());
const documentDirMock = vi.hoisted(() => vi.fn());
vi.mock('../ipc');
stubGetErrorMessage(fallbackOr('error'));
vi.mock('../fileDialogs', () => ({
  saveDialog: saveDialogMock,
  ensureExtension: (p: string) => (p.endsWith('.stalx') ? p : `${p}.stalx`),
  projectSaveFilters: () => [],
  PROJECT_EXTENSION: 'stalx',
}));
vi.mock('../saveFeedback', () => ({
  surfaceSaveFailure: surfaceSaveFailureMock,
  surfaceSaveWarnings: surfaceSaveWarningsMock,
  // A caught rejection's line, as the real one reads it (code, message or fallback).
  saveFailureOf: (_e: unknown, fallback?: string) => ({ code: null, error: fallback ?? 'error' }),
}));
vi.mock('@tauri-apps/api/path', () => ({ documentDir: documentDirMock }));

import { convertToUnmanagedStalx } from '../projectActions';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { useOverwritePromptStore } from '../overwriteGuard';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../test/factories/ipc';

const CHOSEN = '/home/diver/Documents/stalagmight/Cenote_Azul.stalx';

/**
 * [#576 FZ-SDB-5-D1/D2] Unlink MOVES the copy out of the SpeleoDB cache:
 *
 *   1. a Save As in the default projects folder with the project name filled in
 *      (Cancel, or declining the overwrite ask, calls nothing at all),
 *   2. `speleodb_unlink` with that destination - the backend writes the copy, closes
 *      its session on the cache file and deletes it; the frontend saves nothing,
 *   3. then the link is re-seeded and the moved copy loaded, because the backend
 *      session is closed.
 */
describe('convertToUnmanagedStalx moves the copy out of the cache', () => {
  let order: string[];
  let saveProjectMock: ReturnType<typeof vi.fn>;
  let loadProjectMock: ReturnType<typeof vi.fn>;
  let refreshMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockReset();
    order = [];
    documentDirMock.mockResolvedValue('/home/diver/Documents');
    saveDialogMock.mockResolvedValue(CHOSEN);
    saveProjectMock = vi.fn(async () => {
      order.push('save');
      return { success: true, unlinked_copy_path: CHOSEN };
    });
    loadProjectMock = vi.fn(async () => {
      order.push('load');
      return { success: true };
    });
    refreshMock = vi.fn(async () => {
      order.push('refresh');
    });
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      order.push(cmd);
      return cmd === 'speleodb_unlink' ? { saved_path: CHOSEN, unlinked_copy_path: null, warnings: [] } : undefined;
    });
    useProjectStore.setState({
      project: { name: 'Cenote Azul', file_path: '/cache/p1/project.stalx' },
      saveProject: saveProjectMock,
      loadProject: loadProjectMock,
    } as never);
    useSpeleoStore.setState({ refreshLinkStatus: refreshMock } as never);
    useUIStore.setState({ openDialog: vi.fn() } as never);
  });

  it('asks with a Save As in the default projects folder, the project name filled in', async () => {
    await convertToUnmanagedStalx();

    expect(saveDialogMock).toHaveBeenCalledTimes(1);
    expect(saveDialogMock.mock.calls[0]![0]).toMatchObject({
      defaultPath: '/home/diver/Documents/stalagmight/Cenote_Azul.stalx',
    });
  });

  it('without a documents dir it offers the bare name, which saveDialog resolves', async () => {
    documentDirMock.mockRejectedValue(new Error('no documents dir'));

    await convertToUnmanagedStalx();

    expect(saveDialogMock.mock.calls[0]![0]).toMatchObject({ defaultPath: 'Cenote_Azul.stalx' });
  });

  it('unlinks to the destination, then re-seeds, then loads the moved copy - and saves nothing', async () => {
    const converted = await convertToUnmanagedStalx();

    expect(converted).toBe(true);
    expect(order).toEqual(['speleodb_unlink', 'refresh', 'load']);
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_unlink', { destination: CHOSEN });
    expect(saveProjectMock).not.toHaveBeenCalled();
    expect(loadProjectMock).toHaveBeenCalledWith(CHOSEN);
  });

  it('loads the path the backend reports, and tells what the copy went without', async () => {
    const warnings = [{ code: 'UNDO_HISTORY_DROPPED', message: 'dropped', severity: 'warning' }];
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      order.push(cmd);
      return cmd === 'speleodb_unlink'
        ? { saved_path: '/home/diver/Documents/stalagmight/landed.stalx', unlinked_copy_path: null, warnings }
        : undefined;
    });

    expect(await convertToUnmanagedStalx()).toBe(true);

    expect(loadProjectMock).toHaveBeenCalledWith('/home/diver/Documents/stalagmight/landed.stalx');
    expect(surfaceSaveWarningsMock).toHaveBeenCalledWith(warnings);
  });

  it('a cancelled Save As calls nothing at all', async () => {
    saveDialogMock.mockResolvedValue(null);

    expect(await convertToUnmanagedStalx()).toBe(false);
    expect(order).toEqual([]);
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });

  it('a failed unlink reports, re-seeds nothing and loads nothing', async () => {
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      order.push(cmd);
      throw new Error('lock poisoned');
    });
    const openDialog = vi.fn();
    useUIStore.setState({ openDialog } as never);

    const converted = await convertToUnmanagedStalx();

    expect(converted).toBe(false);
    expect(order).toEqual(['speleodb_unlink']);
    expect(loadProjectMock).not.toHaveBeenCalled();
    // Routed through the save-failure router with its own Detach Failed title and
    // skipIntegrityRouting - the unlink reject ran no integrity re-check.
    expect(surfaceSaveFailureMock).toHaveBeenCalledWith(
      expect.objectContaining({ error: expect.stringContaining('Detaching from SpeleoDB failed') }),
      openDialog,
      expect.anything(), // the integrity state, unused with skipIntegrityRouting
      expect.objectContaining({ skipIntegrityRouting: true, title: expect.any(String) }),
    );
  });
});

/**
 * [#576 D35] The copy is written to the NORMALISED name. When that differs from
 * the dialog's answer (`OtherCave` -> `OtherCave.stalx`) the OS overwrite prompt
 * never saw it, and the backend writes over whatever is there - another project's
 * file. Ask first; declining changes nothing.
 */
describe('convertToUnmanagedStalx onto a rewritten existing name', () => {
  let saveProjectMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    useOverwritePromptStore.setState({ request: null });
    documentDirMock.mockResolvedValue('/home/diver/Documents');
    saveDialogMock.mockResolvedValue('/home/diver/OtherCave');
    // The backend's existence probe (`save_target_exists`, #576 HFI-2) finds a file.
    invokeIpcMock.mockReset();
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'save_target_exists') return true;
      if (cmd === 'speleodb_unlink')
        return { saved_path: '/home/diver/OtherCave.stalx', unlinked_copy_path: null, warnings: [] };
      return undefined;
    });
    saveProjectMock = vi.fn(async () => ({ success: true, unlinked_copy_path: '/home/diver/OtherCave.stalx' }));
    useProjectStore.setState({
      project: { name: 'Cenote Azul', file_path: '/cache/p1/project.stalx' },
      saveProject: saveProjectMock,
      loadProject: vi.fn(async () => ({ success: true })),
    } as never);
    useSpeleoStore.setState({ refreshLinkStatus: vi.fn(async () => {}) } as never);
    useUIStore.setState({ openDialog: vi.fn() } as never);
  });

  it('asks before replacing it, and declining calls nothing past the probe', async () => {
    const pending = convertToUnmanagedStalx();

    await vi.waitFor(() => expect(useOverwritePromptStore.getState().request).not.toBeNull());
    expect(useOverwritePromptStore.getState().request?.path).toBe('/home/diver/OtherCave.stalx');
    useOverwritePromptStore.getState().request!.resolve(false);

    expect(await pending).toBe(false);
    expect(saveProjectMock).not.toHaveBeenCalled();
    // Only the existence probe ran: no unlink, nothing else.
    expect(invokeIpcMock.mock.calls.map(([cmd]) => cmd)).toEqual(['save_target_exists']);
  });

  it('moves the copy to the normalised name once the user agrees', async () => {
    const pending = convertToUnmanagedStalx();
    await vi.waitFor(() => expect(useOverwritePromptStore.getState().request).not.toBeNull());
    useOverwritePromptStore.getState().request!.resolve(true);

    expect(await pending).toBe(true);
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_unlink', { destination: '/home/diver/OtherCave.stalx' });
    expect(saveProjectMock).not.toHaveBeenCalled();
  });
});
