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
