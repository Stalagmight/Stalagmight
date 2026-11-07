import { beforeEach, describe, expect, it, vi } from 'vitest';

const saveDialogMock = vi.hoisted(() => vi.fn());
const surfaceSaveFailureMock = vi.hoisted(() => vi.fn());
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
  // A caught rejection's line, as the real one reads it (code, message or fallback).
  saveFailureOf: (_e: unknown, fallback?: string) => ({ code: null, error: fallback ?? 'error' }),
}));

import { convertToUnmanagedStalx } from '../projectActions';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { useOverwritePromptStore } from '../overwriteGuard';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../test/factories/ipc';

/**
 * The detach flow's data-safety ORDER, under Save-a-Copy semantics (an explicit
 * destination on a linked project writes a detached unlinked copy and the
 * session stays put):
 *
 *   1. the copy is written first (durable before anything is given up),
 *   2. `speleodb_unlink` runs only after the copy succeeded,
 *   3. `loadProject(finalPath)` runs LAST, so the session lands on the file
 *      the user chose (the save no longer relocates it there).
 *
 * The old sequence relied on the save relocating the session and would now
 * leave the user editing the CACHE file while told their copy was saved.
 */
describe('convertToUnmanagedStalx ordering', () => {
  let order: string[];
  let saveProjectMock: ReturnType<typeof vi.fn>;
  let loadProjectMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    order = [];
    saveDialogMock.mockResolvedValue('/home/diver/kept.stalx');
    saveProjectMock = vi.fn(async () => {
      order.push('save');
      return { success: true, unlinked_copy_path: '/home/diver/kept.stalx' };
    });
    loadProjectMock = vi.fn(async () => {
      order.push('load');
      return { success: true };
    });
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      order.push(cmd);
      return undefined;
    });
    useProjectStore.setState({
      saveProject: saveProjectMock,
      loadProject: loadProjectMock,
    } as never);
    useSpeleoStore.setState({
      refreshLinkStatus: vi.fn(async () => {
        order.push('refresh');
      }),
    } as never);
    useUIStore.setState({ openDialog: vi.fn() } as never);
  });

  it('writes the copy, then unlinks, then loads the copy - in that order', async () => {
    const converted = await convertToUnmanagedStalx();

    expect(converted).toBe(true);
    expect(order).toEqual(['save', 'speleodb_unlink', 'refresh', 'load']);
    expect(loadProjectMock).toHaveBeenCalledWith('/home/diver/kept.stalx');
  });

  it('a failed save detaches nothing and loads nothing', async () => {
    saveProjectMock.mockImplementation(async () => {
      order.push('save');
      return { success: false, error: 'disk full', code: 'FAILED', failure: { code: 'FAILED', message: 'disk full' } };
    });

    const converted = await convertToUnmanagedStalx();

    expect(converted).toBe(false);
    expect(order).toEqual(['save']);
    expect(surfaceSaveFailureMock).toHaveBeenCalled();
  });

  it('a cancelled save dialog does nothing at all', async () => {
    saveDialogMock.mockResolvedValue(null);

    expect(await convertToUnmanagedStalx()).toBe(false);
    expect(order).toEqual([]);
  });

  it('a project that turned out unlinked keeps the old relocate contract', async () => {
    // No unlinked_copy_path: the save was an ordinary Save As that already
    // relocated the session. Nothing to detach, nothing to re-open.
    saveProjectMock.mockImplementation(async () => {
      order.push('save');
      return { success: true, saved_path: '/home/diver/kept.stalx' };
    });

    const converted = await convertToUnmanagedStalx();

    expect(converted).toBe(true);
    expect(order).toEqual(['save', 'refresh']);
    expect(loadProjectMock).not.toHaveBeenCalled();
  });

  it('a failed unlink reports and does not load the copy', async () => {
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      order.push(cmd);
      throw new Error('lock poisoned');
    });
    const openDialog = vi.fn();
    useUIStore.setState({ openDialog } as never);

    const converted = await convertToUnmanagedStalx();

    expect(converted).toBe(false);
    expect(order).toEqual(['save', 'speleodb_unlink']);
    expect(loadProjectMock).not.toHaveBeenCalled();
    // Routed through the save-failure router (with its own Detach Failed title
    // and skipIntegrityRouting - the unlink reject ran no integrity re-check).
    expect(surfaceSaveFailureMock).toHaveBeenCalledWith(
      expect.objectContaining({ error: expect.stringContaining('detaching from SpeleoDB failed') }),
      openDialog,
      expect.anything(), // the integrity state, unused with skipIntegrityRouting
      expect.objectContaining({ skipIntegrityRouting: true }),
    );
  });
});

/**
 * [#576 D35] The copy is written to the NORMALISED name. When that differs from
 * the dialog's answer (`OtherCave` -> `OtherCave.stalx`) the OS overwrite prompt
 * never saw it, and the backend's copy-only branch renames over whatever is
 * there - another project's file. Ask first; declining changes nothing.
 */
describe('convertToUnmanagedStalx onto a rewritten existing name', () => {
  let saveProjectMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    useOverwritePromptStore.setState({ request: null });
    saveDialogMock.mockResolvedValue('/home/diver/OtherCave');
    // clearAllMocks keeps implementations; drop the failing-unlink one above. The
    // backend's existence probe (`save_target_exists`, #576 HFI-2) finds a file.
    invokeIpcMock.mockReset();
    invokeIpcMock.mockImplementation(async (cmd: string) => (cmd === 'save_target_exists' ? true : undefined));
    saveProjectMock = vi.fn(async () => ({ success: true, unlinked_copy_path: '/home/diver/OtherCave.stalx' }));
    useProjectStore.setState({ saveProject: saveProjectMock, loadProject: vi.fn() } as never);
    useSpeleoStore.setState({ refreshLinkStatus: vi.fn(async () => {}) } as never);
    useUIStore.setState({ openDialog: vi.fn() } as never);
  });

  it('asks before replacing it, and declining saves and detaches nothing', async () => {
    const pending = convertToUnmanagedStalx();

    await vi.waitFor(() => expect(useOverwritePromptStore.getState().request).not.toBeNull());
    expect(useOverwritePromptStore.getState().request?.path).toBe('/home/diver/OtherCave.stalx');
    expect(saveProjectMock).not.toHaveBeenCalled();
    useOverwritePromptStore.getState().request!.resolve(false);

    expect(await pending).toBe(false);
    expect(saveProjectMock).not.toHaveBeenCalled();
    // Only the existence probe ran: no unlink, nothing else.
    expect(invokeIpcMock.mock.calls.map(([cmd]) => cmd)).toEqual(['save_target_exists']);
  });

  it('writes the copy there once the user agrees', async () => {
    const pending = convertToUnmanagedStalx();
    await vi.waitFor(() => expect(useOverwritePromptStore.getState().request).not.toBeNull());
    useOverwritePromptStore.getState().request!.resolve(true);

    expect(await pending).toBe(true);
    expect(saveProjectMock).toHaveBeenCalledWith('/home/diver/OtherCave.stalx');
  });
});
