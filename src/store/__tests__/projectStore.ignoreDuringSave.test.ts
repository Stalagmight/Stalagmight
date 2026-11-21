import { beforeEach, expect, it, vi } from 'vitest';
import { useProjectStore } from '../projectStore';

/**
 * H-frontend-wire-7: an ignore is survey content written by the next save
 * (plan 6.5). A save issued before the ignore reached the backend did not
 * write it, so its completion must not clear `isDirty`: it compares the tree
 * generation it was issued for, and an ignore moves it as every edit does.
 * Without that, the project read clean while the backend still held the
 * unsaved ignore, and quitting did not prompt.
 */
const ipc = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: ipc }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  return { promise: new Promise<T>((yes) => { resolve = yes; }), resolve: (value: T) => resolve(value) };
}

beforeEach(() => {
  ipc.mockReset();
  useProjectStore.setState({
    project: { file_path: '/a.stalx', created_at: 'A' } as never,
    projectTreeGeneration: 10,
    isDirty: true,
    refreshUndoState: vi.fn(async () => {}),
  });
});

it('a save issued before an ignore leaves the project unsaved', async () => {
  let backendDirty = true;
  const save = deferred<{ success: boolean; saved_path: string }>();
  ipc.mockImplementation(async (command: string) => {
    if (command === 'save_project') {
      const result = await save.promise;
      return result;
    }
    if (command === 'resolve_warning') {
      backendDirty = true;
      return undefined;
    }
    if (command === 'has_unsaved_changes') return backendDirty;
    if (command === 'get_project_warnings') return [];
    return undefined;
  });
  // The autosave is issued, the backend writes the file (clean) ...
  const saving = useProjectStore.getState().saveProject(undefined, true);
  backendDirty = false;
  // ... then the ignore lands after it: the backend is dirty again.
  await useProjectStore.getState().resolveWarning(1, 2, 'SHORT_SHOT');
  expect(useProjectStore.getState().isDirty).toBe(true);
  save.resolve({ success: true, saved_path: '/a.stalx' });
  await saving;
  expect(backendDirty).toBe(true);
  expect(useProjectStore.getState().isDirty).toBe(true);
});

it('an ignore after the save completed still marks the project unsaved', async () => {
  ipc.mockImplementation(async (command: string) => {
    if (command === 'save_project') return { success: true, saved_path: '/a.stalx' };
    if (command === 'has_unsaved_changes') return true;
    if (command === 'get_project_warnings') return [];
    return undefined;
  });
  await useProjectStore.getState().saveProject();
  expect(useProjectStore.getState().isDirty).toBe(false);
  await useProjectStore.getState().unresolveWarning(1, 2, 'SHORT_SHOT');
  expect(useProjectStore.getState().isDirty).toBe(true);
});

/**
 * Pre-PR review finding 3: the reply to the ignore's `has_unsaved_changes` (and
 * the fail-safe of a failed one) was correlated by `created_at` alone, so a
 * reopen of the SAME file during the round trip (same path, same `created_at`,
 * a new load) took the previous session's "unsaved" and marked the reopened
 * project dirty; on a 0.6.0 file that arms autosave, which converts a file the
 * user only looked at. It is the load's now (`loading.clientOpId`). Injection:
 * dropping the clientOpId comparison sets isDirty in both cases.
 */
it.each(['answered', 'failed'] as const)('a reopen of the same file during the round trip does not take the reply (%s)', async (outcome) => {
  const reply = deferred<boolean>();
  ipc.mockImplementation(async (command: string) => {
    if (command === 'has_unsaved_changes') {
      const dirty = await reply.promise;
      if (outcome === 'failed') throw new Error('synthetic IPC failure');
      return dirty;
    }
    if (command === 'get_project_warnings') return [];
    return undefined;
  });
  const loading = useProjectStore.getState().loading;
  useProjectStore.setState({ isDirty: false, loading: { ...loading, clientOpId: 1 } });
  const resolving = useProjectStore.getState().resolveWarning(1, 2, 'SHORT_SHOT');
  await vi.waitFor(() => expect(ipc).toHaveBeenCalledWith('has_unsaved_changes', undefined, expect.anything()));
  // The same file opens again meanwhile: same path and created_at, a new load.
  useProjectStore.setState({
    project: { file_path: '/a.stalx', created_at: 'A' } as never,
    isDirty: false,
    loading: { ...loading, clientOpId: 2 },
  });
  reply.resolve(true);
  await resolving;
  expect(useProjectStore.getState().isDirty).toBe(false);
});

it('the same load still takes the reply', async () => {
  ipc.mockImplementation(async (command: string) => {
    if (command === 'has_unsaved_changes') return true;
    if (command === 'get_project_warnings') return [];
    return undefined;
  });
  useProjectStore.setState({ isDirty: false });
  await useProjectStore.getState().resolveWarning(1, 2, 'SHORT_SHOT');
  expect(useProjectStore.getState().isDirty).toBe(true);
});
