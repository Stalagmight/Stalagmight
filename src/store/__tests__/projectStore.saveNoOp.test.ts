import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import type { SaveOutcome } from '../../types/ipc/SaveOutcome';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

/**
 * #mem flash-free publication, part 1: a successful save always echoes
 * `saved_path` (Rust `SaveOutcome::saved_path`). If saveProject
 * re-spread the project on every save, an autosave to the SAME path would change
 * the project object's identity - and MapCanvas's republish `useLayoutEffect`
 * keys on that identity, so a no-op autosave would trigger a full survey rebuild
 * + republish that briefly blanks all geometry. saveProject must therefore only
 * mint a new project object when the path actually changed (first save / Save As).
 */
describe('projectStore.saveProject no-op identity (#mem)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Neutralize the post-save side effects that are irrelevant here.
    useProjectStore.setState({
      refreshUndoState: vi.fn().mockResolvedValue(undefined),
      checkProjectIntegrity: vi.fn().mockResolvedValue(undefined),
      refreshProjectState: vi.fn().mockResolvedValue(undefined),
    } as Partial<ReturnType<typeof useProjectStore.getState>> as never);
  });

  afterEach(() => {
    useProjectStore.setState({ project: null, isDirty: false } as never);
  });

  it('keeps the same project reference on an autosave to the same path', async () => {
    const project = { file_path: '/cave.stalx' } as never;
    useProjectStore.setState({ project, integrityIssues: [] } as never);
    invokeIpcMock.mockResolvedValue({
      saved_path: '/cave.stalx',
      unlinked_copy_path: null,
      warnings: [],
    } satisfies SaveOutcome);

    // `true` = the quiet background save, and it is load-bearing here: the
    // autosave is the ONLY path that re-saves the same path every 30 s, so it is
    // the one that would republish the whole survey on every tick. A manual save
    // to the same path hits the same branch, but exercising only that leaves the
    // hazard's actual caller unpinned.
    await useProjectStore.getState().saveProject('/cave.stalx', true);

    // Identity preserved - no republish trigger for a no-op save.
    expect(useProjectStore.getState().project).toBe(project);
  });

  it('mints a new project object with the new path on a Save As', async () => {
    const project = { file_path: '/cave.stalx' } as never;
    useProjectStore.setState({ project, integrityIssues: [] } as never);
    invokeIpcMock.mockResolvedValue({
      saved_path: '/cave-copy.stalx',
      unlinked_copy_path: null,
      warnings: [],
    } satisfies SaveOutcome);

    await useProjectStore.getState().saveProject('/cave-copy.stalx');

    const next = useProjectStore.getState().project as unknown as { file_path: string };
    expect(next).not.toBe(project);
    expect(next.file_path).toBe('/cave-copy.stalx');
  });

  it('mints a new project object on the first save of a deferred (unsaved) project', async () => {
    const project = { file_path: undefined } as never;
    useProjectStore.setState({ project, integrityIssues: [] } as never);
    invokeIpcMock.mockResolvedValue({
      saved_path: '/new.stalx',
      unlinked_copy_path: null,
      warnings: [],
    } satisfies SaveOutcome);

    await useProjectStore.getState().saveProject('/new.stalx');

    const next = useProjectStore.getState().project as unknown as { file_path: string };
    expect(next).not.toBe(project);
    expect(next.file_path).toBe('/new.stalx');
  });
});
