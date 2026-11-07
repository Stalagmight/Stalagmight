import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import type { SaveOutcome } from '../../types/ipc/SaveOutcome';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

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
      checkProjectIntegrity: vi.fn().mockResolvedValue(undefined),
      refreshProjectState: vi.fn().mockResolvedValue(undefined),
    } as Partial<ReturnType<typeof useProjectStore.getState>> as never);
  });

  afterEach(() => {
    useProjectStore.setState({ project: null, isDirty: false, integrityIssues: [] } as never);
  });

  it('does not re-point the project, does not clear isDirty, and toasts the copy path', async () => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast } as never);
    const project = { file_path: '/cache/p1/project.stalx' } as never;
    useProjectStore.setState({ project, isDirty: true, integrityIssues: [] } as never);
    invokeIpcMock.mockResolvedValue({
      saved_path: null,
      unlinked_copy_path: '/home/diver/copy.stalx',
      warnings: [],
    } satisfies SaveOutcome);

    const result = await useProjectStore.getState().saveProject('/home/diver/copy.stalx');

    expect(result).toMatchObject({ success: true, unlinked_copy_path: '/home/diver/copy.stalx' });
    // The open project is untouched - same object, same path, still dirty.
    expect(useProjectStore.getState().project).toBe(project);
    expect(useProjectStore.getState().isDirty).toBe(true);
    // The user is told where the copy went (the session visibly did not move).
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('/home/diver/copy.stalx'),
      'speleodb-unlinked-copy',
      'warning'
    );
    // Nothing was flagged, so nothing could have been healed - no extra fetch.
    expect(useProjectStore.getState().refreshProjectState).not.toHaveBeenCalled();
  });

  it('refreshes the mirror and marks the session dirty when the save healed the live project', async () => {
    useUIStore.setState({ showToast: vi.fn() } as never);
    const project = { file_path: '/cache/p1/project.stalx' } as never;
    // Clean session, flagged project: the save pipeline heals the LIVE project
    // (duplicate shot UUIDs, non-finite geometry) before it decides to write only
    // a copy, so the healed state never reaches the managed file on disk.
    useProjectStore.setState({
      project,
      isDirty: false,
      integrityIssues: ['duplicate_shot_uuid'],
    } as never);
    invokeIpcMock.mockResolvedValue({
      saved_path: null,
      unlinked_copy_path: '/home/diver/copy.stalx',
      warnings: [],
    } satisfies SaveOutcome);

    await useProjectStore.getState().saveProject('/home/diver/copy.stalx');

    // The mirror is re-pulled from the mutated backend project...
    expect(useProjectStore.getState().refreshProjectState).toHaveBeenCalled();
    // ...and the managed session is dirty: its healed state is unsaved.
    expect(useProjectStore.getState().isDirty).toBe(true);
    // Still no re-point - the open project remains the managed file.
    expect(useProjectStore.getState().project).toBe(project);
  });
});
