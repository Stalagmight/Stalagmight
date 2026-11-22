import { beforeEach, expect, it, vi } from 'vitest';
import { releaseOpenProjectLock } from '../releaseOpenProjectLock';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
const ipc = vi.hoisted(() => vi.fn());
vi.mock('../ipc', () => ({ invokeIpc: ipc, getErrorMessage: (e: Error) => e.message }));
const errorDialog = vi.fn();
const refresh = vi.fn();
const context = { session: 'synthetic' };
beforeEach(() => {
  vi.clearAllMocks();
  ipc.mockImplementation(async (cmd) => cmd === 'speleodb_project_access_at' ? { release_context: context } : null);
  useProjectStore.setState({ project: { file_path: '/synthetic/project.stalx' } as never, isDirty: false, projectTreeGeneration: 0 });
  useSpeleoStore.setState({ busy: false, sessionGeneration: 0, refreshLinkStatus: refresh,
    link: { editState: 'editing', hasUnpushedEdits: false } as never });
  useUIStore.setState({ openDialog: errorDialog });
});
it.each(['unsaved', 'unuploaded'])('refuses %s work without release or implicit upload', async (kind) => {
  if (kind === 'unsaved') useProjectStore.setState({ isDirty: true });
  else useSpeleoStore.setState({ link: { editState: 'editing', hasUnpushedEdits: true } as never });
  await releaseOpenProjectLock();
  expect(ipc).not.toHaveBeenCalled();
  expect(errorDialog).toHaveBeenCalledWith('error', expect.objectContaining({ message: expect.stringContaining('Save Changes') }));
  expect(useSpeleoStore.getState().link?.editState).toBe('editing');
});
it('releases a clean project with only its session context', async () => {
  await releaseOpenProjectLock();
  expect(ipc).toHaveBeenLastCalledWith('speleodb_release_current_lock', { expected: context });
  expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
  expect(refresh).toHaveBeenCalledOnce();
});
it.each(['edit', 'project', 'account'])('rejects a newer %s while inspecting release context', async (kind) => {
  ipc.mockImplementation(async () => {
    if (kind === 'edit') useProjectStore.setState({ projectTreeGeneration: 1, isDirty: true });
    if (kind === 'project') useProjectStore.setState({ project: { file_path: '/other.stalx' } as never });
    if (kind === 'account') useSpeleoStore.setState({ sessionGeneration: 1 });
    return { release_context: context };
  });
  await releaseOpenProjectLock();
  expect(ipc).toHaveBeenCalledTimes(1);
});
it('preserves work and editing state when release fails', async () => {
  ipc.mockImplementation(async (cmd) => {
    if (cmd === 'speleodb_project_access_at') return { release_context: context };
    throw new Error('Release failed');
  });
  await releaseOpenProjectLock();
  expect(errorDialog).toHaveBeenCalledWith('error', expect.objectContaining({ message: 'Release failed' }));
  expect(useSpeleoStore.getState().link?.editState).toBe('editing');
});
// Demo item 4: Switch to Read Only Mode whose release got no reply says why in
// the words every read-only notice uses for that cause, naming the server, never
// the raw diagnostic the backend keeps for the log.
it('words a release that got no reply by its cause, not the raw diagnostic', async () => {
  useSpeleoStore.setState({ session: { loggedIn: true, userEmail: 'diver@example.com', endpoint: 'https://synthetic.example' } });
  ipc.mockImplementation(async (cmd) => {
    if (cmd === 'speleodb_project_access_at') return { release_context: context };
    throw { message: 'Network error talking to SpeleoDB: error sending request', action: 'Retry', status: null,
      transport: 'Refused' };
  });
  await releaseOpenProjectLock();
  expect(errorDialog).toHaveBeenCalledWith('error', {
    title: 'Could not release editing lock',
    message: 'Could not connect to SpeleoDB at https://synthetic.example. Check your network connection, the server '
      + 'address, and whether the server is running.',
  });
});
