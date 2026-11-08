// @vitest-environment jsdom
/**
 * [#576 FUZZ-MERGE-NF] The sign-out prompt (SIGNOUT-PROMPT-D1) waits for the edits
 * queued before it.
 *
 * Which locks a sign-out would hand back is the backend's answer
 * (`speleodb_sign_out_locks`, or `speleodb_sign_in_hand_back_locks` for a sign-in as
 * another account), and a lock kept for un-pushed edits is told apart there by the
 * project's un-pushed mark - which an edit sets only once its IPC has returned. So an
 * edit still in flight left its project's lock listed as releasable, and "Release and
 * sign out" released it under that edit. Now the sign-out waits for the queued edits
 * before it asks, as every exit does; when they have not settled in time it is refused
 * with a short message, and nothing is asked, released or signed out.
 *
 * The REAL project store (`updateShot` through the mutation queue) and the real
 * speleoStore; the IPC is the shared mock, the edit's reply held by hand.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../utils/ipc');
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: vi.fn(async () => {}), destroy: vi.fn(async () => {}) }),
}));

import { useSpeleoStore } from '../speleoStore';
import { useProjectStore } from '../projectStore';
// Loaded up front, as the app loads it at start (App.tsx imports it). Left to the
// store's late import, made here while an edit is in flight under fake timers, the
// test runner handed the module back before its top level had run (a TDZ error on
// EXIT_EDITS_SETTLE_TIMEOUT_MS).
import '../../utils/quit';
import { useUIStore } from '../uiStore';
import { mutationsSettled } from '../mutationQueue';
import { deferred, type Deferred } from '../../test/factories/async';
import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';

emptyBinaryRefresh();

const EDIT = 'update_shot';
const PENDING = 'An edit is still being applied. Try again in a moment.';

const PATHS = [
  {
    kind: 'logout',
    ask: 'speleodb_sign_out_locks',
    act: 'speleodb_logout',
    stage: () => useSpeleoStore.getState().setPendingAuth({ kind: 'logout' }),
  },
  {
    kind: 'switch',
    ask: 'speleodb_sign_in_hand_back_locks',
    act: 'speleodb_login',
    stage: () =>
      useSpeleoStore
        .getState()
        .setPendingAuth({ kind: 'login', email: 'new@y', password: 'pw', token: '', useToken: false }),
  },
] as const;

const calls = (cmd: string) => invokeIpcMock.mock.calls.filter((c) => c[0] === cmd).length;

let heldEdit: Deferred<unknown> | null = null;

/** The backend: the edit's reply held; nothing held, so neither path asks. */
function backend() {
  heldEdit = deferred<unknown>();
  const edit = heldEdit;
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd === EDIT) return edit.promise;
    if (cmd === 'speleodb_sign_out_locks' || cmd === 'speleodb_sign_in_hand_back_locks')
      return { releasable: [], unpushed: [] };
    if (cmd === 'speleodb_logout') return { locks_left_held: [] };
    if (cmd === 'speleodb_login')
      return {
        user_email: 'new@y',
        endpoint: 'https://x/',
        keychain_backed: true,
        storage: 'keychain',
        keychain: 'available',
        needs_storage_choice: false,
        replaced_session: false,
        locks_left_held: [],
      };
    if (cmd === 'check_project_integrity') return { messages: [], save_blocked: false };
    return null;
  });
  return edit;
}

/** A grid cell committed on blur: queued, its IPC not answered. Boxed, so awaiting this does not await the edit. */
async function startEdit(): Promise<{ done: Promise<unknown> }> {
  const done = useProjectStore.getState().updateShot(1, 1, { length: 55 });
  await vi.advanceTimersByTimeAsync(0);
  expect(calls(EDIT)).toBe(1);
  return { done };
}

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

beforeEach(() => {
  vi.useFakeTimers();
  invokeIpcMock.mockReset();
  useUIStore.setState({ toasts: [], activeDialog: null, dialogData: {} });
  useSpeleoStore.setState({ signOutPrompt: null, link: null, projectLinkKind: 'local' });
  useSpeleoStore.getState().setSessionFromLogin({
    user_email: 'old@x',
    endpoint: 'https://x/',
    keychain_backed: true,
    storage: 'keychain',
    keychain: 'available',
    needs_storage_choice: false,
    replaced_session: false,
    locks_left_held: [],
  });
  useProjectStore.setState({
    project: { name: 'CaveA', file_path: '/cave/a.stalx' } as never,
    isDirty: false,
    isClosing: false,
  });
});

afterEach(async () => {
  heldEdit?.resolve({ remote_link: null });
  heldEdit = null;
  await settle(mutationsSettled());
  useSpeleoStore.setState({ session: null, pendingAuth: null, signOutPrompt: null });
  useProjectStore.setState({ project: null });
  vi.useRealTimers();
});

describe.each(PATHS)('sign-out waits for in-flight edits: $kind (#576 FUZZ-MERGE-NF)', (path) => {
  beforeEach(() => path.stage());

  it('asks the backend which locks to hand back only once the edit has landed', async () => {
    const edit = backend();
    const { done: editDone } = await startEdit();

    const applied = useSpeleoStore.getState().applyPendingAuth('https://x/');
    await vi.advanceTimersByTimeAsync(0);
    expect(calls(path.ask)).toBe(0);
    expect(calls(path.act)).toBe(0);

    edit.resolve({ remote_link: null });
    await settle(editDone);
    const result = await settle(applied);

    expect(result.ok).toBe(true);
    expect(calls(path.ask)).toBe(1);
    expect(calls(path.act)).toBe(1);
    // The ask came after the edit's reply, so it saw what the edit left.
    const order = invokeIpcMock.mock.calls.map((c) => c[0]);
    expect(order.indexOf(path.ask)).toBeGreaterThan(order.indexOf(EDIT));
  });

  it('an edit that never settles: refused with a short message, nothing asked or signed out', async () => {
    backend();
    await startEdit();

    const applied = useSpeleoStore.getState().applyPendingAuth('https://x/');
    await vi.advanceTimersByTimeAsync(5_000);
    const result = await settle(applied);

    expect(result.ok).toBe(false);
    expect((result.error as Error).message).toBe(PENDING);
    expect(calls(path.ask)).toBe(0);
    expect(calls(path.act)).toBe(0);
    expect(useSpeleoStore.getState().session?.userEmail).toBe('old@x');
    expect(useSpeleoStore.getState().pendingAuth).not.toBeNull();
  });

  it('control: with no project open it asks at once', async () => {
    backend();
    useProjectStore.setState({ project: null });

    const result = await settle(useSpeleoStore.getState().applyPendingAuth('https://x/'));

    expect(result.ok).toBe(true);
    expect(calls(path.ask)).toBe(1);
    expect(calls(path.act)).toBe(1);
  });
});
