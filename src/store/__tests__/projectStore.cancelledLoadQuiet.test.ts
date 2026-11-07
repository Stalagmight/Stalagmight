/**
 * [#576 Q-W44-2-D1] A load the user cancelled (the loading modal's Cancel) ends
 * without an error modal, whatever its answer: CANCELLED, or a
 * FILE_REPLACED_DURING_LOAD that crossed the Cancel. Before, either answer put the
 * loading state back into its error state ("Operation cancelled", "The file was
 * replaced while it was being opened.") after Cancel had closed it.
 *
 * Store tier: the real store; the load IPC and the rest of the IPC are mocked. All
 * data is synthetic.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { invokeIpcMock } from '../../test/factories/ipc';
import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';

const loadBinaryMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc');
vi.mock('../../utils/loadProjectBinary', () => ({ loadProjectFullBinary: loadBinaryMock }));

const PATH = '/caves/CaveA.stalx';
const CANCELLED = { code: 'CANCELLED', message: 'Operation cancelled' };
const REPLACED = { code: 'FILE_REPLACED_DURING_LOAD', message: 'The file was replaced while it was being opened.' };
const IO = { code: 'IO', message: 'Could not read the project file.' };

/** A load answer the test delivers by hand. */
function holdLoad(): { reject: (e: unknown) => void } {
  const held: { reject: (e: unknown) => void } = { reject: () => {} };
  loadBinaryMock.mockImplementation(
    () =>
      new Promise((_resolve, reject) => {
        held.reject = reject;
      }),
  );
  return held;
}

async function loadStarted(): Promise<void> {
  await vi.waitFor(() => expect(loadBinaryMock).toHaveBeenCalled());
  expect(useProjectStore.getState().loading.isLoading).toBe(true);
}

beforeEach(() => {
  vi.clearAllMocks();
  loadBinaryMock.mockReset();
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'list_recent_projects' || command === 'take_unrecognised_shot_kinds') return [];
    return undefined;
  });
  useUIStore.setState({ activeDialog: null, toasts: [] });
  useProjectStore.setState({
    project: null,
    loading: { loadingId: 0, clientOpId: 0, isLoading: false, overallProgress: 0 },
    refreshUndoState: vi.fn(async () => {}),
  } as never);
});

describe('a load the user cancelled shows no error (#576 Q-W44-2-D1)', () => {
  it('a load nobody cancelled still shows its failure', async () => {
    const held = holdLoad();
    const outcome = useProjectStore.getState().loadProject(PATH);
    await loadStarted();

    held.reject(IO);

    expect(await outcome).toEqual({ success: false });
    expect(useProjectStore.getState().loading.isLoading).toBe(true);
    expect(useProjectStore.getState().loading.error?.message).toBe(IO.message);
  });

  it.each([
    ['CANCELLED', CANCELLED],
    ['FILE_REPLACED_DURING_LOAD', REPLACED],
  ])('answering %s after the Cancel: no error, the loading state ends', async (_code, answer) => {
    const held = holdLoad();
    const outcome = useProjectStore.getState().loadProject(PATH);
    await loadStarted();

    await useProjectStore.getState().cancelLoading({ byUser: true });
    held.reject(answer);

    expect(await outcome).toEqual({ success: false });
    expect(loadBinaryMock).toHaveBeenCalledTimes(1);
    expect(useProjectStore.getState().loading.error).toBeUndefined();
    expect(useProjectStore.getState().loading.isLoading).toBe(false);
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it.each([
    ['CANCELLED', CANCELLED],
    ['FILE_REPLACED_DURING_LOAD', REPLACED],
  ])('answering %s before the cancel IPC answers: no error, and no repeat', async (_code, answer) => {
    const held = holdLoad();
    let answerCancel: () => void = () => {};
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'list_recent_projects' || command === 'take_unrecognised_shot_kinds') return [];
      if (command === 'cancel_loading' && useProjectStore.getState().loading.isLoading) {
        return new Promise<void>((resolve) => {
          answerCancel = resolve;
        });
      }
      return undefined;
    });
    const outcome = useProjectStore.getState().loadProject(PATH);
    await loadStarted();

    const cancelled = useProjectStore.getState().cancelLoading({ byUser: true });
    held.reject(answer);
    expect(await outcome).toEqual({ success: false });
    expect(loadBinaryMock).toHaveBeenCalledTimes(1);
    expect(useProjectStore.getState().loading.error).toBeUndefined();
    expect(useProjectStore.getState().loading.isLoading).toBe(false);

    answerCancel();
    await cancelled;
    expect(useProjectStore.getState().loading.error).toBeUndefined();
    expect(useProjectStore.getState().loading.isLoading).toBe(false);
  });
});
