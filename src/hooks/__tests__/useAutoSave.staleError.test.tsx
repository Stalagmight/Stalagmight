/**
 * [#429] The auto-save 'error' status is the one status with no timer under it -
 * it has to persist while the changes really are not on disk, and it is what
 * paints the Save menu item red (`dangerBg: saveFailed`, useProjectMenuItems).
 * So it must not outlive the condition: a save that succeeded while a failing
 * autosave was still in flight leaves the project clean, and a red Save item
 * with nothing to save is a lie the user cannot clear.
 *
 * `.tsx` is load-bearing - `vite.config.ts` routes only `*.test.tsx` to jsdom,
 * and `renderHook` needs it.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';

import { useAutoSave } from '../useAutoSave';
import { useProjectStore } from '../../store/projectStore';
import type { CommandResult } from '../../types/commands';
import { deferred } from '../../test/factories/async';

const FAILED: CommandResult = {
  success: false,
  error: 'Disk full: no space left on device',
  warnings: [],
  section_ids: [],
};

describe('useAutoSave error status', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    useProjectStore.setState({
      project: { file_path: '/cave.stalx' } as never,
      config: { general: { auto_save: true, auto_save_interval_seconds: 1 } } as never,
      isDirty: false,
      integritySaveBlocked: false,
    } as never);
  });

  afterEach(() => {
    vi.useRealTimers();
    useProjectStore.setState({ project: null, isDirty: false } as never);
  });

  /** Drive the hook to the point where the debounced save is in flight. */
  async function startPendingSave(save: () => Promise<CommandResult>) {
    useProjectStore.setState({ saveProject: save } as never);
    const hook = renderHook(() => useAutoSave());
    act(() => {
      useProjectStore.setState({ isDirty: true });
    });
    expect(hook.result.current.status).toBe('pending');
    await act(async () => {
      vi.advanceTimersByTime(1000);
    });
    return hook;
  }

  it('reports a failed autosave while the changes are still unsaved', async () => {
    const pending = deferred<CommandResult>();
    const hook = await startPendingSave(() => pending.promise);

    await act(async () => {
      pending.resolve(FAILED);
    });

    expect(hook.result.current.status).toBe('error');
  });

  it('does not go red when another save landed while it was in flight', async () => {
    const pending = deferred<CommandResult>();
    const hook = await startPendingSave(() => pending.promise);

    // A manual Ctrl+S to a working folder completes first: the store goes
    // clean, so the autosave's own failure is about changes already on disk.
    act(() => {
      useProjectStore.setState({ isDirty: false });
    });
    await act(async () => {
      pending.resolve(FAILED);
    });

    expect(hook.result.current.status).not.toBe('error');
  });

  it('drops the error once the project goes clean (the save-completed path)', async () => {
    const pending = deferred<CommandResult>();
    const hook = await startPendingSave(() => pending.promise);
    await act(async () => {
      pending.resolve(FAILED);
    });
    expect(hook.result.current.status).toBe('error');

    // A later successful save clears it, through the dirty -> clean flash.
    act(() => {
      useProjectStore.setState({ isDirty: false });
    });
    act(() => {
      vi.advanceTimersByTime(3000);
    });

    expect(hook.result.current.status).not.toBe('error');
  });

  /**
   * [#576 FZ-FZ-INT-4] An autosave refused because the project changed - or was
   * being switched - under it (the backend's PROJECT_CHANGED, or the store's own
   * refusal during a switch) is not an autosave failure: the switch is the user's
   * own action, and nothing about their changes failed.
   */
  it.each([
    ['the backend', { success: false, error: 'Save was not applied: another project is open now.', code: 'PROJECT_CHANGED' }],
    ['the store', { success: false, error: 'The project changed before this action could run.', code: null }],
  ])('does not go red for a PROJECT_CHANGED refusal from %s', async (_by, refusal) => {
    const pending = deferred<CommandResult>();
    const hook = await startPendingSave(() => pending.promise);

    await act(async () => {
      pending.resolve(refusal as unknown as CommandResult);
    });

    expect(hook.result.current.status).not.toBe('error');
  });
});
