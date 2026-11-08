// @vitest-environment jsdom
/**
 * [#576 W84-NF] Opening a project from SpeleoDB over an open project waits for the
 * edits queued before it, as every other exit does (W83/W84).
 *
 * The open's entry check (`handBackRefusal`) reads `isDirty`, which an edit sets only
 * once its IPC and the refresh after it have returned. A grid cell committed on blur on
 * the way to the SpeleoDB browser is still in flight then, so the open project read as
 * clean, the open went ahead, and its command waited for the edit (`afterQueuedEdits`):
 * only the backend's persisted dirty bit then refused the replace, in its own words, and
 * a frontend-only edit had nothing to stop it. Now each entry check (open, keep the local
 * copy, take the server copy, adopt) waits for the queued edits first; one that has not
 * settled in time counts as unsaved work.
 *
 * The REAL store (`updateShot` through the mutation queue) and the real hook; the IPC is
 * the shared mock, the edit's reply held by hand. All data is synthetic.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook } from '@testing-library/react';

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: vi.fn(async () => {}), destroy: vi.fn(async () => {}) }),
}));
vi.mock('../../utils/ipc');

import { useOpenRemoteProject, type OpenRemoteOutcome } from '../useOpenRemoteProject';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { mutationsSettled } from '../../store/mutationQueue';
import { deferred, type Deferred } from '../../test/factories/async';
import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';
import type { SpeleoProjectSummary } from '../../types/ipc.generated';

emptyBinaryRefresh();

const EDIT = 'update_shot';
const OPEN_COMMANDS = ['speleodb_open_project', 'speleodb_open_project_discarding_local', 'speleodb_adopt_ariane'];
const SUMMARY: SpeleoProjectSummary = {
  id: 'p-remote',
  name: 'Cave R',
  permission: 'READ_AND_WRITE',
  lock_holder: null,
  type_tag: 'stalx',
  latest_commit_sha: 'abc123',
};
const UNSAVED = 'Save or discard the changes in the open project first - opening Cave R would close it and lose them.';

let loadProject: ReturnType<typeof vi.fn>;

const sentOpens = () => invokeIpcMock.mock.calls.filter((c) => OPEN_COMMANDS.includes(c[0] as string));
const calls = (cmd: string) => invokeIpcMock.mock.calls.filter((c) => c[0] === cmd).length;

/** Await `p` while letting fake time run in short steps; fails rather than hangs. */
async function settle<T>(p: Promise<T>): Promise<T> {
  let done = false;
  const tracked = p.finally(() => {
    done = true;
  });
  for (let i = 0; i < 100 && !done; i += 1) await vi.advanceTimersByTimeAsync(10);
  expect(done).toBe(true);
  return tracked;
}

let heldEdit: Deferred<unknown> | null = null;

/** Hold the edit's IPC open; every open command answers as a successful pull would. */
function holdEdit() {
  const edit = deferred<unknown>();
  heldEdit = edit;
  invokeIpcMock.mockImplementation((cmd: string) => {
    if (cmd === EDIT) return edit.promise;
    if (cmd === 'speleodb_open_project' || cmd === 'speleodb_open_project_discarding_local')
      return Promise.resolve({ local_path: '/cache/p-remote.stalx', concurrent_session: false });
    if (cmd === 'speleodb_adopt_ariane')
      return Promise.resolve({ local_path: '/cache/p-remote.stalx', import_receipt: null });
    if (cmd === 'check_project_integrity') return Promise.resolve({ messages: [], save_blocked: false });
    return Promise.resolve(null);
  });
  return edit;
}

/** A grid cell committed on blur: queued, its IPC not answered. Boxed, so awaiting this does not await the edit. */
async function startEdit(): Promise<{ done: Promise<unknown> }> {
  const done = useProjectStore.getState().updateShot(1, 1, { length: 55 });
  await vi.advanceTimersByTimeAsync(0);
  expect(calls(EDIT)).toBe(1);
  expect(useProjectStore.getState().isDirty).toBe(false);
  return { done };
}

type Verb = (h: ReturnType<typeof useOpenRemoteProject>) => Promise<OpenRemoteOutcome>;
const VERBS: Array<[string, Verb]> = [
  ['Open', (h) => h.openRemote(SUMMARY)],
  ['Keep the local copy', (h) => h.keepLocalCopy(SUMMARY, '/cache/p-remote.stalx')],
  ['Take the server copy', (h) => h.takeRemoteDiscardingLocal(SUMMARY)],
  ['Adopt', (h) => h.adoptRemote(SUMMARY)],
];

beforeEach(() => {
  vi.useFakeTimers();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(null);
  loadProject = vi.fn(async () => ({ success: true }));
  useUIStore.setState({ activeDialog: null, dialogData: {}, toasts: [] });
  useSpeleoStore.setState({ link: null, remoteProjectId: null, projectLinkKind: 'local' });
  // A saved local project: nothing for the entry check to protect yet.
  useProjectStore.setState({
    project: { name: 'CaveA', file_path: '/cave/a.stalx' } as never,
    isDirty: false,
    isClosing: false,
    loadProject: loadProject as never,
  });
});

afterEach(async () => {
  heldEdit?.resolve({ remote_link: null });
  heldEdit = null;
  await settle(mutationsSettled());
  vi.useRealTimers();
});

describe('opening from SpeleoDB waits for the edits queued before it (#576 W84-NF)', () => {
  it.each(VERBS)('%s with an edit in flight on a clean project: refused once the edit lands, nothing pulled', async (_name, verb) => {
    const { result } = renderHook(() => useOpenRemoteProject({}));
    const edit = holdEdit();
    const { done: editDone } = await startEdit();

    const opening = verb(result.current);
    await vi.advanceTimersByTimeAsync(0);
    expect(sentOpens()).toHaveLength(0);

    edit.resolve({ remote_link: null });
    await settle(editDone);
    const outcome = await settle(opening);

    expect(useProjectStore.getState().isDirty).toBe(true);
    expect(outcome).toEqual({ status: 'error', message: UNSAVED, reLogin: false });
    expect(sentOpens()).toHaveLength(0);
    expect(loadProject).not.toHaveBeenCalled();
  });

  it('an edit that never settles counts as unsaved work once the wait runs out', async () => {
    const { result } = renderHook(() => useOpenRemoteProject({}));
    holdEdit();
    await startEdit();

    const opening = result.current.openRemote(SUMMARY);
    await vi.advanceTimersByTimeAsync(5_000);
    const outcome = await settle(opening);

    expect(outcome).toEqual({ status: 'error', message: UNSAVED, reLogin: false });
    expect(sentOpens()).toHaveLength(0);
    expect(loadProject).not.toHaveBeenCalled();
  });

  it('control: a clean project with nothing in flight opens at once', async () => {
    holdEdit();
    const { result } = renderHook(() => useOpenRemoteProject({}));

    const outcome = await settle(result.current.openRemote(SUMMARY));

    expect(outcome).toEqual({ status: 'opened' });
    expect(sentOpens()).toHaveLength(1);
    expect(loadProject).toHaveBeenCalledWith('/cache/p-remote.stalx');
  });
});
