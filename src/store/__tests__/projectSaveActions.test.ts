import { beforeEach, expect, it, vi } from 'vitest';
import { createSaveProjectAction } from '../projectSaveActions';
import { useProjectStore } from '../projectStore';
const ipc = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: ipc }));
function deferred<T>() {
  let resolve!: (value: T) => void;
  return { promise: new Promise<T>((yes) => { resolve = yes; }), resolve: (value: T) => resolve(value) };
}
const save = createSaveProjectAction(useProjectStore.setState, useProjectStore.getState);
beforeEach(() => {
  ipc.mockReset();
  useProjectStore.setState({ project: { file_path: '/a.stalx', created_at: 'A' } as never,
    projectTreeGeneration: 10, isDirty: true, integrityIssues: [],
    refreshUndoState: vi.fn(async () => {}), checkProjectIntegrity: vi.fn(async () => []),
  });
});
it('never clears edits made after a local save started', async () => {
  const pending = deferred<{ success: boolean }>();
  ipc.mockReturnValue(pending.promise);
  const operation = save();
  useProjectStore.setState({ projectTreeGeneration: 11 });
  pending.resolve({ success: true });
  await operation;
  expect(useProjectStore.getState().isDirty).toBe(true);
});
it('never changes the path or dirty state of a replacement project', async () => {
  const pending = deferred<{ success: boolean; saved_path: string }>();
  ipc.mockReturnValue(pending.promise);
  const operation = save();
  const replacement = { file_path: '/b.stalx', created_at: 'B' };
  useProjectStore.setState({ project: replacement as never });
  pending.resolve({ success: true, saved_path: '/a.stalx' });
  await operation;
  expect(useProjectStore.getState().project).toBe(replacement);
  expect(useProjectStore.getState().isDirty).toBe(true);
  expect(useProjectStore.getState().refreshUndoState).not.toHaveBeenCalled();
});
it('keeps an ordinary save-as owned after its returned path changes', async () => {
  ipc.mockResolvedValue({ success: true, saved_path: '/new.stalx' });
  await save('/new.stalx');
  expect(useProjectStore.getState().project?.file_path).toBe('/new.stalx');
  expect(useProjectStore.getState().isDirty).toBe(false);
});
it('drops completion after a switch away and back to the same project', async () => {
  const pending = deferred<{ success: boolean }>();
  ipc.mockReturnValue(pending.promise);
  const operation = save();
  useProjectStore.setState((state) => ({ loading: { ...state.loading, clientOpId: state.loading.clientOpId + 1 } }));
  pending.resolve({ success: true });
  await operation;
  expect(useProjectStore.getState().isDirty).toBe(true);
});
it('rechecks ownership after an asynchronous undo-state refresh', async () => {
  const pending = deferred<void>();
  ipc.mockResolvedValue({ success: true });
  const refresh = vi.fn(() => pending.promise);
  useProjectStore.setState({ refreshUndoState: refresh });
  const operation = save();
  await vi.waitFor(() => expect(refresh).toHaveBeenCalled());
  useProjectStore.setState({ project: { file_path: '/b.stalx' } as never });
  pending.resolve();
  await operation;
  expect(useProjectStore.getState().isDirty).toBe(true);
});
