import { beforeEach, describe, expect, it, vi } from 'vitest';

const saveDialogMock = vi.hoisted(() => vi.fn());
const surfaceSaveFailureMock = vi.hoisted(() => vi.fn());

vi.mock('../ipc');
vi.mock('../fileDialogs', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../fileDialogs')>()),
  saveDialog: saveDialogMock,
}));
vi.mock('../saveFeedback', () => ({
  surfaceSaveFailure: surfaceSaveFailureMock,
  // A caught rejection's line, as the real one reads it (code, message or fallback).
  saveFailureOf: (_e: unknown, fallback?: string) => ({ code: null, error: fallback ?? 'error' }),
}));

import { saveProjectAs } from '../projectActions';
import { IpcCommandError } from '../commandError';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { invokeIpcMock } from '../../test/factories/ipc';

/**
 * [#576 HFI-2] The existence probe is the backend's `save_target_exists` (the
 * webview's fs plugin has no scope for user paths: its `exists()` threw for every
 * path in the real app, so every Save As asked "could not check"). Scripts the
 * backend's reply to that one command.
 */
function backendSays(answer: boolean | Error): void {
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd !== 'save_target_exists') throw new Error(`unexpected command ${cmd}`);
    if (answer instanceof Error) throw answer;
    return answer;
  });
}

/**
 * [#576 E10 / NF-SAVEAS] The Save As overwrite guard is the only thing between
 * the user and replacing an existing survey file, and it had no test at any tier
 * (the Playwright mock answers every existence probe with false). The pins below fix
 * all three outcomes of the existence probe - including the one that used to
 * fail OPEN: a probe that throws (permissions, a sandbox scope, a network drive)
 * must ask exactly like a probe that says "it exists" (E10-D1, "Ask (fail
 * closed)"), never save over the path unasked.
 */
describe('saveProjectAs overwrite guard', () => {
  let openDialog: ReturnType<typeof vi.fn>;
  let saveProject: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockReset();
    saveDialogMock.mockResolvedValue('/home/diver/CaveA.stalx');
    openDialog = vi.fn();
    saveProject = vi.fn(async () => ({ success: true, saved_path: '/home/diver/CaveA.stalx' }));
    useUIStore.setState({ openDialog } as never);
    useProjectStore.setState({ saveProject } as never);
  });

  it('asks before replacing a file that exists, and does not save', async () => {
    backendSays(true);

    expect(await saveProjectAs()).toBeNull();

    expect(saveProject).not.toHaveBeenCalled();
    expect(openDialog).toHaveBeenCalledWith('confirm-overwrite', {
      path: '/home/diver/CaveA.stalx',
      fileName: 'CaveA.stalx',
      closeAfterSave: false,
    });
  });

  it('saves straight away when the file does not exist', async () => {
    backendSays(false);

    const result = await saveProjectAs();

    expect(result?.success).toBe(true);
    expect(invokeIpcMock).toHaveBeenCalledWith(
      'save_target_exists',
      { path: '/home/diver/CaveA.stalx' },
      expect.anything(),
    );
    expect(saveProject).toHaveBeenCalledWith('/home/diver/CaveA.stalx');
    expect(openDialog).not.toHaveBeenCalled();
  });

  it('asks (fail closed) when the existence check itself fails, and does not save', async () => {
    backendSays(new Error('permission denied'));

    expect(await saveProjectAs()).toBeNull();

    expect(saveProject).not.toHaveBeenCalled();
    expect(openDialog).toHaveBeenCalledWith('confirm-overwrite', {
      path: '/home/diver/CaveA.stalx',
      fileName: 'CaveA.stalx',
      closeAfterSave: false,
      existenceUnknown: true,
    });
  });

  // [#576 HFI-2-D2] A refused path (an app or system folder): the save refuses it
  // too, with the reason, so Save As goes straight to it - one dialog, the
  // refusal - instead of first asking "could not check whether it exists".
  it('goes straight to the save when the backend refuses the path, and shows only its refusal', async () => {
    saveDialogMock.mockResolvedValue('/etc/CaveA.stalx');
    const refused = { success: false, error: 'Refused to change CaveA.stalx: it is inside a system folder' };
    saveProject.mockResolvedValue(refused);
    backendSays(
      new IpcCommandError({
        code: 'WRITE_PATH_REFUSED',
        message: 'Refused to change CaveA.stalx: it is inside a system folder',
      }),
    );

    expect(await saveProjectAs()).toBe(refused);

    expect(openDialog).not.toHaveBeenCalledWith('confirm-overwrite', expect.anything());
    expect(saveProject).toHaveBeenCalledWith('/etc/CaveA.stalx');
    expect(surfaceSaveFailureMock).toHaveBeenCalledTimes(1);
    expect(surfaceSaveFailureMock).toHaveBeenCalledWith(refused, openDialog, expect.anything());
  });

  it('still asks (fail closed) for any other coded failure of the check', async () => {
    backendSays(
      new IpcCommandError({
        code: 'FS_PERMISSION_DENIED',
        message: 'Could not check whether CaveA.stalx already exists: permission denied',
      }),
    );

    expect(await saveProjectAs()).toBeNull();

    expect(saveProject).not.toHaveBeenCalled();
    expect(openDialog).toHaveBeenCalledWith(
      'confirm-overwrite',
      expect.objectContaining({ path: '/home/diver/CaveA.stalx', existenceUnknown: true }),
    );
  });

  it('carries Save and Close through the fail-closed prompt', async () => {
    backendSays(new Error('network drive unreachable'));

    await saveProjectAs({ closeAfterSave: true });

    expect(saveProject).not.toHaveBeenCalled();
    expect(openDialog).toHaveBeenCalledWith(
      'confirm-overwrite',
      expect.objectContaining({ closeAfterSave: true, existenceUnknown: true }),
    );
  });

  it('checks the name it will write, not the name the dialog returned', async () => {
    saveDialogMock.mockResolvedValue('/home/diver/CaveA');
    backendSays(true);

    await saveProjectAs();

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'save_target_exists',
      { path: '/home/diver/CaveA.stalx' },
      expect.anything(),
    );
    expect(openDialog).toHaveBeenCalledWith(
      'confirm-overwrite',
      expect.objectContaining({ path: '/home/diver/CaveA.stalx' }),
    );
  });
});
