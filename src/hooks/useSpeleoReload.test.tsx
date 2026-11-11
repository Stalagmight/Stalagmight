import { act, renderHook } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { useSpeleoActions } from './useSpeleoActions';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore } from '../store/speleoStore';
const mocks = vi.hoisted(() => ({ ipc: vi.fn(), confirm: vi.fn(), replace: vi.fn() }));
vi.mock('../utils/ipc', () => ({ invokeIpc: mocks.ipc, getErrorMessage: String }));
vi.mock('../utils/speleoReloadConfirm', () => ({ confirmSpeleoReloadDiscard: mocks.confirm }));
vi.mock('../utils/speleoTreeReplacement', () => ({ applySpeleoTreeReplacement: mocks.replace }));
const expected = { session_generation: 'synthetic', project_modified_at: 'synthetic' };
const load = vi.fn();
beforeEach(() => {
  vi.clearAllMocks();
  mocks.confirm.mockResolvedValue(true);
  mocks.replace.mockResolvedValue(true);
  load.mockResolvedValue({ success: true });
  useProjectStore.setState({ project: { file_path: '/remote.stalx' } as never, isDirty: false, projectTreeGeneration: 0, loadProject: load });
  useSpeleoStore.setState({ busy: false, remoteProjectId: 'remote', sessionGeneration: 0,
    link: { hasUnpushedEdits: false } as never, applyUpdate: vi.fn(), refreshLinkStatus: vi.fn() });
  mocks.ipc.mockImplementation(async (cmd) => cmd === 'speleodb_project_access_at' ? { release_context: expected } : { kind: 'up_to_date' });
});
it('reload cancellation preserves pending work without network, save or release', async () => {
  useProjectStore.setState({ isDirty: true });
  mocks.confirm.mockResolvedValue(false);
  const { result } = renderHook(useSpeleoActions);
  await act(() => result.current.update());
  expect(mocks.ipc).not.toHaveBeenCalled();
  expect(useProjectStore.getState().isDirty).toBe(true);
  expect(mocks.confirm).toHaveBeenCalledOnce();
});
it('explicit discard is bound to the inspected session and reload suppresses acquisition', async () => {
  useProjectStore.setState({ isDirty: true });
  mocks.ipc.mockImplementation(async (cmd) => cmd === 'speleodb_project_access_at' ? { release_context: expected } : { kind: 'fast_forwarded', local_path: '/remote.stalx' });
  const { result } = renderHook(useSpeleoActions);
  await act(() => result.current.update());
  expect(mocks.ipc.mock.calls.map(([cmd]) => cmd)).toEqual(['speleodb_project_access_at', 'speleodb_update']);
  expect(mocks.ipc).toHaveBeenLastCalledWith('speleodb_update', { discardChanges: true, expected });
  expect(load).toHaveBeenCalledWith('/remote.stalx', { acquireLock: false });
  expect(mocks.replace).toHaveBeenCalledOnce();
});
it('normal reload never acknowledges discard', async () => {
  const { result } = renderHook(useSpeleoActions);
  await act(() => result.current.update());
  expect(mocks.confirm).not.toHaveBeenCalled();
  expect(mocks.ipc).toHaveBeenLastCalledWith('speleodb_update', { discardChanges: false, expected });
});
it.each(['edit', 'project', 'account'])('reload rejects a newer %s during confirmation', async (kind) => {
  useProjectStore.setState({ isDirty: true });
  mocks.confirm.mockImplementation(async () => {
    if (kind === 'edit') useProjectStore.setState({ projectTreeGeneration: 1 });
    if (kind === 'project') useProjectStore.setState({ project: { file_path: '/other.stalx' } as never });
    if (kind === 'account') useSpeleoStore.setState({ sessionGeneration: 1 });
    return true;
  });
  const { result } = renderHook(useSpeleoActions);
  await act(() => result.current.update());
  expect(mocks.ipc).not.toHaveBeenCalled();
});
it('reload failure preserves pending work and never reconciles a failed replacement', async () => {
  useProjectStore.setState({ isDirty: true });
  mocks.ipc.mockImplementation(async (cmd) => {
    if (cmd === 'speleodb_project_access_at') return { release_context: expected };
    throw new Error('Invalid native project');
  });
  const { result } = renderHook(useSpeleoActions);
  await act(() => result.current.update());
  expect(load).not.toHaveBeenCalled();
  expect(mocks.replace).not.toHaveBeenCalled();
  expect(useProjectStore.getState().isDirty).toBe(true);
});
