/**
 * H-frontend-wire-8: autosave pauses after a failure no retry can fix (a file
 * another program changed, a read-only file) and resumes when the store's
 * `autosaveFailureKind` leaves that failure. The save action records the kind
 * only while the project is still dirty, so a failing autosave that resolved
 * after a concurrent save made the project clean paused with nothing recorded:
 * no banner, and no transition could ever resume it. Every later edit stayed
 * unsaved until the project was closed.
 *
 * `.tsx` is load-bearing - `vite.config.ts` routes only `*.test.tsx` to jsdom.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';

import { useAutoSave } from '../useAutoSave';
import { useProjectStore } from '../../store/projectStore';
import type { CommandResult } from '../../types/commands';

const CHANGED: CommandResult = {
  success: false,
  error: 'EXTERNALLY_MODIFIED: /cave.stalx was changed by another program since it was opened',
  warnings: [],
  line_ids: [],
};
const SAVED: CommandResult = { success: true, error: null, warnings: [], line_ids: [] };

describe('useAutoSave pause', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    useProjectStore.setState({
      project: { file_path: '/cave.stalx' } as never,
      config: { general: { auto_save: true, auto_save_interval_seconds: 1 } } as never,
      isDirty: false,
      autosaveFailure: null,
      autosaveFailureKind: null,
    } as never);
  });

  afterEach(() => {
    vi.useRealTimers();
    useProjectStore.setState({ project: null, isDirty: false, autosaveFailureKind: null } as never);
  });

  async function edit() {
    act(() => {
      useProjectStore.setState({ isDirty: true });
    });
    await act(async () => {
      vi.advanceTimersByTime(1000);
    });
  }

  it('does not stay paused when the failure was never recorded (the project went clean first)', async () => {
    const save = vi.fn(async (): Promise<CommandResult> => {
      if (save.mock.calls.length === 1) {
        // A manual save ("Keep my version") landed while this autosave was in
        // flight: the project is clean, so no failure kind is recorded.
        useProjectStore.setState({ isDirty: false });
        return CHANGED;
      }
      useProjectStore.setState({ isDirty: false });
      return SAVED;
    });
    useProjectStore.setState({ saveProject: save } as never);
    renderHook(() => useAutoSave());
    await edit();
    expect(save).toHaveBeenCalledTimes(1);
    await edit();
    expect(save).toHaveBeenCalledTimes(2);
  });

  it('pauses on a recorded failure and resumes once it clears', async () => {
    const save = vi.fn(async (): Promise<CommandResult> => {
      if (save.mock.calls.length === 1) {
        useProjectStore.setState({ autosaveFailureKind: 'externally-modified' } as never);
        return CHANGED;
      }
      useProjectStore.setState({ isDirty: false });
      return SAVED;
    });
    useProjectStore.setState({ saveProject: save } as never);
    renderHook(() => useAutoSave());
    await edit();
    await edit();
    // Paused: a retry would fail the same way until the user decides.
    expect(save).toHaveBeenCalledTimes(1);
    // The save prompt's answer landed (a manual save cleared the failure).
    act(() => {
      useProjectStore.setState({ isDirty: false, autosaveFailureKind: null } as never);
    });
    await edit();
    expect(save).toHaveBeenCalledTimes(2);
  });
});
