import { beforeEach, expect, it, vi } from 'vitest';
import { autoAcquireSpeleoLock, useSpeleoReadOnlyNotice } from '../autoAcquireSpeleoLock';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';

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
  expect(ipc).toHaveBeenCalledWith('speleodb_start_editing');
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
