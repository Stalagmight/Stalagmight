import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useAutoSave } from '../useAutoSave';
import { useProject } from '../useProject';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';

afterEach(() => { vi.useRealTimers(); });
it.each([false, true])('keeps pending local autosave running while the message dialog is open or cancelled (%s)', async (cancel) => {
  vi.useFakeTimers();
  const save = vi.fn(async () => { useProjectStore.setState({ isDirty: false }); return { success: true }; });
  useProjectStore.setState({ project: { file_path: '/synthetic.stalx' } as never,
    config: { general: { auto_save: true, auto_save_interval_seconds: 1 } } as never,
    isDirty: false, integritySaveBlocked: false, saveProject: save as never,
  });
  useSpeleoStore.setState({ link: { editState: 'editing', permission: 'READ_AND_WRITE', noAccess: false } as never,
    session: { loggedIn: true, endpoint: 'https://synthetic.example', userEmail: 'user@example.com' } });
  const hook = renderHook(() => ({ autosave: useAutoSave(), actions: useProject() }));
  act(() => useProjectStore.setState({ isDirty: true }));
  expect(hook.result.current.autosave.status).toBe('pending');
  await act(async () => { await hook.result.current.actions.handleSaveProject(); });
  expect(useUIStore.getState().activeDialog).toBe('speleodb-done');
  expect(save).not.toHaveBeenCalled();
  if (cancel) act(() => useUIStore.getState().closeDialog());
  expect(hook.result.current.autosave.status).toBe('pending');
  await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
  expect(save).toHaveBeenCalledExactlyOnceWith(undefined, true);
  expect(useProjectStore.getState().isDirty).toBe(false);
});
