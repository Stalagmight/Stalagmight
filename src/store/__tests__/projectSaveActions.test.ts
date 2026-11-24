import { beforeEach, expect, it, vi } from 'vitest';
import { createSaveProjectAction } from '../projectSaveActions';
import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { useSpeleoStore } from '../speleoStore';
import { OLDER_BUILDS_DROP_GPS_FIXES, resetOlderVersionsNoticeForTests } from '../../utils/gpsFixSaveNotice';
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
    projectTreeGeneration: 10, isDirty: true,
    refreshUndoState: vi.fn(async () => {}),
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
it('tells once per project that older versions drop its GPS fixes (#741)', async () => {
  resetOlderVersionsNoticeForTests();
  const showToast = vi.fn();
  useUIStore.setState({ showToast } as never);
  ipc.mockResolvedValue({
    success: true,
    saved_path: '/a.stalx',
    warnings: [{ code: OLDER_BUILDS_DROP_GPS_FIXES, message: 'x', severity: 'warning' }],
  });
  await save();
  await save(undefined, true);
  expect(showToast).toHaveBeenCalledTimes(1);
  expect(showToast.mock.calls[0]?.[2]).toBe('warning');
});
// S2: the save cleared the SpeleoDB copy's stale un-uploaded mark (its survey is
// BASE's again); the exit prompt reads the store's mirror of it.
it('mirrors an un-uploaded mark the save cleared', async () => {
  const clearUnpushedMark = vi.fn();
  useSpeleoStore.setState({ clearUnpushedMark } as never);
  ipc.mockResolvedValue({ success: true, saved_path: '/a.stalx', speleodb_unpushed_cleared: true });
  await save();
  expect(clearUnpushedMark).toHaveBeenCalledTimes(1);
  ipc.mockResolvedValue({ success: true, saved_path: '/a.stalx' });
  await save();
  expect(clearUnpushedMark).toHaveBeenCalledTimes(1);
});
it('leaves the mark of an edit made while the save ran', async () => {
  const clearUnpushedMark = vi.fn();
  useSpeleoStore.setState({ clearUnpushedMark } as never);
  const pending = deferred<{ success: boolean; speleodb_unpushed_cleared: boolean }>();
  ipc.mockReturnValue(pending.promise);
  const operation = save();
  useProjectStore.setState({ projectTreeGeneration: 11 });
  pending.resolve({ success: true, speleodb_unpushed_cleared: true });
  await operation;
  expect(clearUnpushedMark).not.toHaveBeenCalled();
});
