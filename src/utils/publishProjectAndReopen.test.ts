import { beforeEach, expect, it, vi } from 'vitest';
import { publishProjectAndReopen } from './publishProjectAndReopen';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore } from '../store/speleoStore';
import type { SpeleoPublishParams } from '../types/ipc.generated';
const mock = vi.hoisted(() => ({ ipc: vi.fn(), acquire: vi.fn() }));
vi.mock('./ipc', () => ({ invokeIpc: mock.ipc, getErrorMessage: String }));
vi.mock('./autoAcquireSpeleoLock', () => ({ autoAcquireSpeleoLock: mock.acquire }));
const params: SpeleoPublishParams = { name: 'Synthetic cave', description: 'Synthetic description', message: 'Project Creation', country: 'US', visibility: 'PRIVATE', latitude: null, longitude: null };
const published = { project_id: 'remote', endpoint: 'https://synthetic.example', local_path: '/cache/remote/project.stalx' };
const status = { is_remote: true, project_id: 'remote', edit_enabled: false, has_unpushed_edits: false, permission: 'ADMIN', remote_state: 'up_to_date', unwound: false };
const load = vi.fn();
const removeRecent = vi.fn();
beforeEach(() => {
  vi.clearAllMocks();
  useProjectStore.setState({ project: { name: params.name, file_path: '/local.stalx', created_at: 'synthetic' } as never,
    isDirty: true, projectTreeGeneration: 0, loadProject: load, removeFromRecentProjects: removeRecent });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local', remoteProjectId: null, sessionGeneration: 0, busy: false });
  load.mockResolvedValue({ success: true });
  removeRecent.mockResolvedValue({ success: true });
  mock.acquire.mockResolvedValue(undefined);
  mock.ipc.mockImplementation(async (cmd) => {
    if (cmd === 'speleodb_publish') return published;
    if (cmd === 'speleodb_link_status') return status;
    if (cmd === 'speleodb_open_project') return { local_path: published.local_path, offline: false };
    throw new Error(`Unexpected command ${cmd}`);
  });
});
it('conversion saves and uploads before downloading a fresh copy and acquiring editing exactly once', async () => {
  load.mockImplementation(async () => {
    useProjectStore.setState((state) => ({ project: { ...state.project!, created_at: 'server-normalized' } }));
    return { success: true };
  });
  expect((await publishProjectAndReopen(params)).outcome).toBe('opened');
  expect(mock.ipc.mock.calls.map(([cmd]) => cmd)).toEqual(['speleodb_publish', 'speleodb_link_status', 'speleodb_open_project']);
  expect(mock.ipc).toHaveBeenCalledWith('speleodb_publish', { params });
  expect(mock.ipc).toHaveBeenLastCalledWith('speleodb_open_project', { projectId: 'remote', projectName: params.name, latestCommitSha: null });
  expect(load).toHaveBeenCalledWith(published.local_path, expect.objectContaining({ accessConfirmed: true, acquireLock: false }));
  expect(mock.acquire).toHaveBeenCalledOnce();
  expect(useProjectStore.getState().project?.file_path).toBe(published.local_path);
  expect(useProjectStore.getState().isDirty).toBe(false);
  expect(useSpeleoStore.getState().projectLinkKind).toBe('remote');
});
it('conversion preserves a linked copy after an unconfirmed initial upload without downloading over its work', async () => {
  mock.ipc.mockImplementation(async (cmd) => cmd === 'speleodb_publish' ? published : { ...status, has_unpushed_edits: true });
  expect((await publishProjectAndReopen(params)).outcome).toBe('upload_pending');
  expect(mock.ipc).toHaveBeenCalledTimes(2);
  expect(load).not.toHaveBeenCalled();
  expect(mock.acquire).not.toHaveBeenCalled();
  expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
  expect(useSpeleoStore.getState().projectLinkKind).toBe('remote');
});
it('conversion preserves newer edits made during creation instead of discarding them for a fresh download', async () => {
  mock.ipc.mockImplementation(async (cmd) => {
    if (cmd === 'speleodb_publish') {
      useProjectStore.setState({ projectTreeGeneration: 1, isDirty: true });
      return published;
    }
    return status;
  });
  expect((await publishProjectAndReopen(params)).outcome).toBe('upload_pending');
  expect(useProjectStore.getState().isDirty).toBe(true);
  expect(load).not.toHaveBeenCalled();
});
it.each(['download', 'offline', 'load'])('conversion keeps the created remote identity after a failed %s', async (failure) => {
  mock.ipc.mockImplementation(async (cmd) => {
    if (cmd === 'speleodb_publish') return published;
    if (cmd === 'speleodb_link_status') return status;
    if (failure === 'download') throw new Error('Download failed');
    return { local_path: published.local_path, offline: failure === 'offline' };
  });
  if (failure === 'load') load.mockResolvedValue({ success: false });
  expect((await publishProjectAndReopen(params)).outcome).toBe('reload_failed');
  expect(useSpeleoStore.getState().projectLinkKind).toBe('remote');
  expect(useProjectStore.getState().project?.file_path).toBe(published.local_path);
  expect(mock.acquire).not.toHaveBeenCalled();
  expect((await publishProjectAndReopen(params)).outcome).toBe('stale');
  expect(mock.ipc.mock.calls.filter(([cmd]) => cmd === 'speleodb_publish')).toHaveLength(1);
});
it.each(['project', 'account'])('conversion does not apply a late creation result after the %s changes', async (change) => {
  mock.ipc.mockImplementation(async () => {
    if (change === 'project') useProjectStore.setState({ project: { file_path: '/other.stalx' } as never });
    else useSpeleoStore.setState({ sessionGeneration: 1 });
    return published;
  });
  expect((await publishProjectAndReopen(params)).outcome).toBe('stale');
  expect(mock.ipc).toHaveBeenCalledTimes(1);
  expect(load).not.toHaveBeenCalled();
  expect(mock.acquire).not.toHaveBeenCalled();
});
it('conversion leaves the local project and unsaved work intact when server creation fails', async () => {
  mock.ipc.mockRejectedValue(new Error('Create failed'));
  await expect(publishProjectAndReopen(params)).rejects.toThrow('Create failed');
  expect(useProjectStore.getState().project?.file_path).toBe('/local.stalx');
  expect(useProjectStore.getState().isDirty).toBe(true);
  expect(useSpeleoStore.getState().link).toBeNull();
  expect(load).not.toHaveBeenCalled();
});

it.each(['project', 'account'])('conversion ignores a late fresh-download reply after its %s changes', async (change) => {
  mock.ipc.mockImplementation(async (cmd) => {
    if (cmd === 'speleodb_publish') return published;
    if (cmd === 'speleodb_link_status') return status;
    if (change === 'project') {
      useProjectStore.setState({ project: { file_path: '/other.stalx' } as never });
      useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
    } else useSpeleoStore.setState({ sessionGeneration: 1 });
    return { local_path: published.local_path, offline: false };
  });
  expect((await publishProjectAndReopen(params)).outcome).toBe('stale');
  expect(load).not.toHaveBeenCalled();
  expect(mock.acquire).not.toHaveBeenCalled();
});

it('reports a first upload that left the undo history out (sdk#3), beside the outcome', async () => {
  expect(await publishProjectAndReopen(params)).toEqual({ outcome: 'opened', undoHistoryOmitted: false });
  mock.ipc.mockImplementation(async (cmd) => {
    if (cmd === 'speleodb_publish') return { ...published, undo_history_omitted: true };
    if (cmd === 'speleodb_link_status') return status;
    if (cmd === 'speleodb_open_project') return { local_path: published.local_path, offline: false };
    throw new Error(`Unexpected command ${cmd}`);
  });
  useProjectStore.setState({ project: { name: params.name, file_path: '/local.stalx', created_at: 'synthetic' } as never });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local', remoteProjectId: null });
  expect(await publishProjectAndReopen(params)).toEqual({ outcome: 'opened', undoHistoryOmitted: true });
});

it('conversion removes only the original local recent reference after fresh reopening', async () => {
  expect((await publishProjectAndReopen(params)).outcome).toBe('opened');
  expect(removeRecent).toHaveBeenCalledExactlyOnceWith('/local.stalx');
  expect(removeRecent.mock.invocationCallOrder[0]).toBeGreaterThan(load.mock.invocationCallOrder[0]!);
  expect(removeRecent.mock.invocationCallOrder[0]).toBeGreaterThan(mock.acquire.mock.invocationCallOrder[0]!);
  expect(mock.ipc.mock.calls.some(([command]) => command === 'delete_project')).toBe(false);
  expect(useProjectStore.getState().project?.file_path).toBe(published.local_path);
});
it('conversion reports recent-reference cleanup failure without retrying publication or deleting files', async () => {
  removeRecent.mockResolvedValue({ success: false, error: 'Synthetic config write failure' });
  expect((await publishProjectAndReopen(params)).outcome).toBe('recent_cleanup_failed');
  expect(useProjectStore.getState().project?.file_path).toBe(published.local_path);
  expect(removeRecent).toHaveBeenCalledExactlyOnceWith('/local.stalx');
  expect((await publishProjectAndReopen(params)).outcome).toBe('stale');
  expect(mock.ipc.mock.calls.filter(([command]) => command === 'speleodb_publish')).toHaveLength(1);
});
it.each(['pending', 'failed', 'stale'])('conversion keeps the local recent reference when %s', async (outcome) => {
  if (outcome === 'pending') mock.ipc.mockImplementation(async (cmd) => cmd === 'speleodb_publish' ? published : { ...status, has_unpushed_edits: true });
  if (outcome === 'failed') load.mockResolvedValue({ success: false });
  if (outcome === 'stale') mock.acquire.mockImplementation(async () => useSpeleoStore.setState({ sessionGeneration: 1 }));
  expect((await publishProjectAndReopen(params)).outcome).not.toBe('opened');
  expect(removeRecent).not.toHaveBeenCalled();
});
