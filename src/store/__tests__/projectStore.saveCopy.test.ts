import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import type { CommandResult } from '../../types/commands';

const invokeIpcMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
}));

/**
 * Save a Copy: an explicit destination on a SpeleoDB-managed project writes a
 * detached UNLINKED copy and the session stays on the managed file. The backend
 * says so via `unlinked_copy_path` (and deliberately leaves `saved_path`
 * absent), and the store must treat that result as "the open project was NOT
 * saved": no file_path re-point, no isDirty clear - only the toast that tells
 * the user where the copy went.
 */
describe('projectStore.saveProject detached-copy result', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useProjectStore.setState({
      refreshUndoState: vi.fn().mockResolvedValue(undefined),
      refreshProjectState: vi.fn().mockResolvedValue(undefined),
    } as Partial<ReturnType<typeof useProjectStore.getState>> as never);
  });

  afterEach(() => {
    useProjectStore.setState({ project: null, isDirty: false } as never);
  });

  it('does not re-point the project, does not clear isDirty, and toasts the copy path', async () => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast } as never);
    const project = { file_path: '/cache/p1/project.stalx' } as never;
    useProjectStore.setState({ project, isDirty: true } as never);
    invokeIpcMock.mockResolvedValue({
      success: true,
      unlinked_copy_path: '/home/diver/copy.stalx',
    } as CommandResult);

    const result = await useProjectStore.getState().saveProject('/home/diver/copy.stalx');

    expect(result?.unlinked_copy_path).toBe('/home/diver/copy.stalx');
    // The open project is untouched - same object, same path, still dirty.
    expect(useProjectStore.getState().project).toBe(project);
    expect(useProjectStore.getState().isDirty).toBe(true);
    // The user is told where the copy went (the session visibly did not move).
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('/home/diver/copy.stalx'),
      'speleodb-unlinked-copy',
      'warning'
    );
    // The save heals nothing, so there is nothing to re-fetch.
    expect(useProjectStore.getState().refreshProjectState).not.toHaveBeenCalled();
  });

  it('leaves a clean session clean and unrefreshed: the v2 save heals nothing', async () => {
    useUIStore.setState({ showToast: vi.fn() } as never);
    const project = { file_path: '/cache/p1/project.stalx' } as never;
    // 0.6.0's save healed the LIVE project (duplicate shot UUIDs, non-finite
    // geometry) before it decided to write only a copy, so a flagged session had
    // to be re-pulled and marked dirty. The `.stalx` v2 save writes the survey
    // as every edit validated it, so the copy changes nothing in the session.
    useProjectStore.setState({ project, isDirty: false } as never);
    invokeIpcMock.mockResolvedValue({
      success: true,
      unlinked_copy_path: '/home/diver/copy.stalx',
    } as CommandResult);

    await useProjectStore.getState().saveProject('/home/diver/copy.stalx');

    expect(useProjectStore.getState().refreshProjectState).not.toHaveBeenCalled();
    expect(useProjectStore.getState().isDirty).toBe(false);
    // No re-point - the open project remains the managed file.
    expect(useProjectStore.getState().project).toBe(project);
  });
});
