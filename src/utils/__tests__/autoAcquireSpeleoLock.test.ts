import { beforeEach, expect, it, vi } from 'vitest';
import { autoAcquireSpeleoLock, useSpeleoReadOnlyNotice } from '../autoAcquireSpeleoLock';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import i18n from '../../i18n';

const ipc = vi.hoisted(() => vi.fn());
const replaceTree = vi.hoisted(() => vi.fn(async () => true));
vi.mock('../ipc', () => ({ invokeIpc: ipc, getErrorMessage: (error: {message?: string}) => error.message ?? 'Failed' }));
vi.mock('../speleoTreeReplacement', () => ({ applySpeleoTreeReplacement: replaceTree }));

beforeEach(() => {
  ipc.mockReset();
  replaceTree.mockClear();
  useProjectStore.setState({ project: { file_path: '/remote.stalx' } as never });
  useSpeleoStore.setState({ sessionGeneration: 0, session: null, remoteProjectId: 'remote', link: null,
    busy: false, refreshLinkStatus: vi.fn(async () => {}) });
  useSpeleoStore.getState().syncProjectLink(true, '/remote.stalx');
  useSpeleoReadOnlyNotice.setState({ notice: null });
});

it('acquires automatically and silently enables editing', async () => {
  ipc.mockResolvedValue({ kind: 'editing' });
  await autoAcquireSpeleoLock('/remote.stalx', () => true);
  expect(ipc).toHaveBeenCalledWith('speleodb_start_editing', { afterOpen: true });
  expect(useSpeleoStore.getState().link?.editState).toBe('editing');
  expect(useSpeleoReadOnlyNotice.getState().notice).toBeNull();
  expect(useSpeleoStore.getState().busy).toBe(false);
});

it('shows the actual competing editor and leaves the project read-only', async () => {
  ipc.mockResolvedValue({ kind: 'locked_by', email: 'editor@example.com' });
  await autoAcquireSpeleoLock('/remote.stalx', () => true);
  expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
  expect(useSpeleoReadOnlyNotice.getState().notice?.message).toContain('editor@example.com');
});

// sdbdemo2 issue 3 (screenshot 15): a second session of the account opened the
// project and took over the lock the first session was editing under, in
// silence. After an open the backend reports such a lock (this account's, never
// claimed by this run) as held by this account; the project stays read-only and
// the notice says what the held-lock prompt said, and that Resume editing edits
// it here too. The notice never names our own email as "currently editing".
it('a lock this account already holds opens read-only and says so in the prompt\'s words', async () => {
  useSpeleoStore.setState({ session: { loggedIn: true, userEmail: 'Diver@Example.com', endpoint: 'https://synthetic.example' } });
  ipc.mockResolvedValue({ kind: 'locked_by', email: 'diver@example.com' });
  await autoAcquireSpeleoLock('/remote.stalx', () => true);
  expect(ipc).toHaveBeenCalledWith('speleodb_start_editing', { afterOpen: true });
  expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
  const notice = useSpeleoReadOnlyNotice.getState().notice;
  expect(notice?.kind).toBe('opened');
  expect(notice?.message).toBe(i18n.t('speleodb.readOnlyNotice.heldByYourAccount'));
  expect(notice?.message).not.toContain('diver@example.com');
});

// Opened over our own un-uploaded work (a fast-forward local copy, or "Open my
// local copy"): someone else holding the lock must not read as "your work is gone".
it('says local changes are kept when someone else holds the lock over our own work', async () => {
  useSpeleoStore.setState((state) => ({ link: state.link ? { ...state.link, hasUnpushedEdits: true } : null }));
  ipc.mockResolvedValue({ kind: 'locked_by', email: 'editor@example.com' });
  await autoAcquireSpeleoLock('/remote.stalx', () => true);
  const message = useSpeleoReadOnlyNotice.getState().notice?.message ?? '';
  expect(message).toContain('editor@example.com');
  expect(message).toContain('Your changes are kept on this computer');
  expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
});

it('reconciles a fast-forward before completing the acquisition', async () => {
  ipc.mockResolvedValue({ kind: 'fast_forwarded', local_path: '/remote.stalx' });
  await autoAcquireSpeleoLock('/remote.stalx', () => true);
  expect(replaceTree).toHaveBeenCalledOnce();
});

it('does not invent an editor when acquisition fails offline', async () => {
  ipc.mockRejectedValue({ message: 'Network unavailable' });
  await autoAcquireSpeleoLock('/remote.stalx', () => true);
  expect(useSpeleoReadOnlyNotice.getState().notice?.message).toContain('read-only');
  expect(useSpeleoReadOnlyNotice.getState().notice?.message).not.toContain('currently editing');
  expect(useSpeleoStore.getState().busy).toBe(false);
});

// After an offline open (F1) the automatic Start Editing loses its reply the same
// way the open did. The backend keeps its raw diagnostic in `message` and the
// cause in `transport` (pinned by the core test
// `the_automatic_start_editing_after_an_offline_open_names_the_lost_reply`): the
// notice is the worded line for that cause, naming the server, never the raw text.
it.each([
  ['Dropped', /^The connection to SpeleoDB at https:\/\/synthetic\.example was lost before it finished answering\./],
  ['Refused', /^Could not connect to SpeleoDB at https:\/\/synthetic\.example\./],
  ['Timeout', /^SpeleoDB at https:\/\/synthetic\.example did not respond in time\./],
  ['Dns', /^Could not find the SpeleoDB server at https:\/\/synthetic\.example\./],
] as const)('an automatic Start Editing that got no reply (%s) is worded for its cause, not the raw error', async (transport, line) => {
  useSpeleoStore.setState({ session: { loggedIn: true, userEmail: 'diver@example.com', endpoint: 'https://synthetic.example' } });
  ipc.mockRejectedValue({
    message: 'Network error talking to SpeleoDB: error sending request', action: 'Retry', status: null, transport,
  });
  await autoAcquireSpeleoLock('/remote.stalx', () => true);
  const message = useSpeleoReadOnlyNotice.getState().notice?.message ?? '';
  expect(message).toMatch(line);
  expect(message).not.toMatch(/network error|error sending request/i);
  expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
  // A lost reply is no reason to sign out.
  expect(useSpeleoStore.getState().session?.loggedIn).toBe(true);
});

// Demo item 3. A lock request that may have reached SpeleoDB, which nothing could
// confirm, leaves the app a pointer to a lock that may be this account's; the next
// close hands it back or asks "Couldn't hand back the lock". The notice says so in
// one sentence, so that question is no surprise - only when the backend says so
// (`lock_may_be_held`), never from the cause alone. An automatic Start Editing
// runs after an open, so its notice keeps the open's title (demo item 7).
const LOCK_SENTENCE = 'The lock request may have reached SpeleoDB; if it did, Stalagmight hands the lock back '
  + 'when you close the project, or you can try again.';
it.each([
  ['Dropped', true, true],
  ['Timeout', true, true],
  ['Dropped', undefined, false],
  ['Refused', undefined, false],
] as const)('an automatic Start Editing that failed (%s, lock_may_be_held %s) says the lock may be held: %s',
  async (transport, flag, said) => {
    useSpeleoStore.setState({ session: { loggedIn: true, userEmail: 'diver@example.com', endpoint: 'https://synthetic.example' } });
    ipc.mockRejectedValue({
      message: 'Network error talking to SpeleoDB: error sending request', action: 'Retry', status: null, transport,
      ...(flag ? { lock_may_be_held: flag } : {}),
    });
    await autoAcquireSpeleoLock('/remote.stalx', () => true);
    const notice = useSpeleoReadOnlyNotice.getState().notice;
    expect(notice?.kind).toBe('opened');
    expect(notice?.message.endsWith(` ${LOCK_SENTENCE}`)).toBe(said);
    expect(notice?.message).not.toMatch(/network error|error sending request/i);
  });

it('ignores a late response after switching projects', async () => {
  let resolve!: (value: unknown) => void;
  ipc.mockReturnValue(new Promise(r => { resolve = r; }));
  const pending = autoAcquireSpeleoLock('/remote.stalx', () => true);
  useProjectStore.setState({ project: { file_path: '/other.stalx' } as never });
  resolve({ kind: 'locked_by', email: 'editor@example.com' });
  await pending;
  expect(useSpeleoReadOnlyNotice.getState().notice).toBeNull();
});

it('does not acquire for a superseded open request', async () => {
  await autoAcquireSpeleoLock('/remote.stalx', () => false);
  expect(ipc).not.toHaveBeenCalled();
});

it('keeps editing disabled until a fast-forward finishes reconciling', async () => {
  ipc.mockResolvedValue({ kind: 'fast_forwarded', local_path: '/remote.stalx' });
  let finish!: (value: boolean) => void;
  replaceTree.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const pending = autoAcquireSpeleoLock('/remote.stalx', () => true);
  await vi.waitFor(() => expect(replaceTree).toHaveBeenCalled());
  expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
  finish(false);
  await pending;
  expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
});

it('removes the current acquired project from the recovery prompt', async () => {
  ipc.mockResolvedValue({ kind: 'editing' });
  useSpeleoStore.setState({ remoteProjectId: 'remote',
    heldLocks: [{ projectId: 'remote', projectName: 'Remote' }], heldLockPromptOpen: true });
  await autoAcquireSpeleoLock('/remote.stalx', () => true);
  expect(useSpeleoStore.getState().heldLockPromptOpen).toBe(false);
  expect(useSpeleoStore.getState().heldLocks).toBeNull();
});
