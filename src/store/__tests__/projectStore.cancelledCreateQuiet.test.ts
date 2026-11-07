/**
 * [#576 Q-W44-2-D1, W60-NF1] A create the user cancelled (the loading modal's Cancel)
 * ends without an error modal, whatever its answer, as a cancelled load does. Before,
 * a create's failure answer that arrived after the Cancel found the create's
 * `clientOpId` still current (Cancel resets the loading state but keeps it) and put
 * the loading state back into its error state ("Operation cancelled").
 *
 * Store tier: the real store; `create_project_full` is held and answered by hand,
 * the rest of the IPC is mocked. All data is synthetic.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { deferred, type Deferred } from '../../test/factories/async';
import { invokeIpcMock } from '../../test/factories/ipc';
import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';

vi.mock('../../utils/ipc');

const NAME = 'Cave C';
const CANCELLED = { code: 'CANCELLED', message: 'Operation cancelled' };
const IO = { code: 'IO', message: 'Could not create the project file.' };

/** The create's answer, delivered by the test; the cancel's answer, held when asked. */
let createAnswer: Deferred<unknown>;
let holdCancel: Deferred<undefined> | null;

beforeEach(() => {
  vi.clearAllMocks();
  createAnswer = deferred<unknown>();
  holdCancel = null;
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'create_project_full') return createAnswer.promise;
    if (command === 'cancel_loading' && holdCancel && useProjectStore.getState().loading.isLoading) {
      return holdCancel.promise;
    }
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

/** The create's IPC has been sent and its loading state is showing. */
async function createSent(): Promise<void> {
  await vi.waitFor(() =>
    expect(invokeIpcMock.mock.calls.some(([command]) => command === 'create_project_full')).toBe(true),
  );
  expect(useProjectStore.getState().loading.isLoading).toBe(true);
}

describe('a create the user cancelled shows no error (#576 Q-W44-2-D1, W60-NF1)', () => {
  it('a create nobody cancelled still shows its failure', async () => {
    const outcome = useProjectStore.getState().createProject(NAME);
    await createSent();

    createAnswer.reject(IO);

    const result = await outcome;
    expect(result.success).toBe(false);
    expect(result.error).toBe(IO.message);
    expect(useProjectStore.getState().loading.isLoading).toBe(true);
    expect(useProjectStore.getState().loading.error?.message).toBe(IO.message);
  });

  it('answering CANCELLED after the Cancel: no error, the loading state ends', async () => {
    const outcome = useProjectStore.getState().createProject(NAME);
    await createSent();

    await useProjectStore.getState().cancelLoading({ byUser: true });
    createAnswer.reject(CANCELLED);

    expect((await outcome).success).toBe(false);
    expect(useProjectStore.getState().loading.error).toBeUndefined();
    expect(useProjectStore.getState().loading.isLoading).toBe(false);
    expect(useProjectStore.getState().project).toBeNull();
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('answering CANCELLED before the cancel IPC answers: no error, and none once it does', async () => {
    const outcome = useProjectStore.getState().createProject(NAME);
    await createSent();

    holdCancel = deferred<undefined>();
    const cancelled = useProjectStore.getState().cancelLoading({ byUser: true });
    createAnswer.reject(CANCELLED);
    expect((await outcome).success).toBe(false);
    expect(useProjectStore.getState().loading.error).toBeUndefined();
    expect(useProjectStore.getState().loading.isLoading).toBe(false);

    holdCancel.resolve(undefined);
    await cancelled;
    expect(useProjectStore.getState().loading.error).toBeUndefined();
    expect(useProjectStore.getState().loading.isLoading).toBe(false);
  });
});
