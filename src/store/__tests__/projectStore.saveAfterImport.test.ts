import { afterEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { accepted, refusedBy, type CommandOutcome } from '../commandResult';
import type { SaveOutcome } from '../../types/ipc/SaveOutcome';

const SAVED: CommandOutcome<SaveOutcome> = accepted<SaveOutcome>({ saved_path: null, unlinked_copy_path: null, warnings: [] });

/**
 * #92 + PR #104 review: a DMP import never marks the project dirty and a new
 * project's initial save is deferred, so the import is only persisted because
 * the completion handlers call `saveAfterImport`. These guard its contract:
 *  - NEW project → a real save (writes the .stalx + Recent Projects entry);
 *  - EXISTING project → only mark dirty, so unrelated pending edits aren't
 *    silently committed (Codex review);
 *  - a failed save → mark dirty (no phantom "saved") and signal the caller not
 *    to dismiss the error dialog.
 */
describe('projectStore.saveAfterImport (#92)', () => {
  // Capture the real action so overwriting it via setState in each test never
  // leaks the mock into other test files (execution-order independence).
  const realSaveProject = useProjectStore.getState().saveProject;

  afterEach(() => {
    vi.restoreAllMocks();
    useProjectStore.setState({ saveProject: realSaveProject, isDirty: false });
    // Failure cases open the error dialog (directly or via surfaceSaveFailure) -
    // reset dialog + import state so it can't leak into later tests.
    useUIStore.setState({ importContext: null, activeDialog: null, dialogData: {} });
  });

  it('persists a NEW-project import via saveProject and lets the caller proceed', async () => {
    useUIStore.setState({ importContext: 'new-project' });
    const saveMock = vi.fn(async (): Promise<CommandOutcome<SaveOutcome>> => SAVED);
    useProjectStore.setState({ saveProject: saveMock, isDirty: false });

    const proceed = await useProjectStore.getState().saveAfterImport();

    expect(saveMock).toHaveBeenCalledTimes(1);
    expect(proceed).toBe(true);
  });

  it('only marks dirty for an EXISTING-project import (does not auto-commit prior edits)', async () => {
    useUIStore.setState({ importContext: 'existing-project' });
    const saveMock = vi.fn(async (): Promise<CommandOutcome<SaveOutcome>> => SAVED);
    useProjectStore.setState({ saveProject: saveMock, isDirty: false });

    const proceed = await useProjectStore.getState().saveAfterImport();

    expect(saveMock).not.toHaveBeenCalled();
    expect(useProjectStore.getState().isDirty).toBe(true);
    expect(proceed).toBe(true);
  });

  it('on a failed new-project save: marks dirty, surfaces the error, and blocks navigation', async () => {
    useUIStore.setState({ importContext: 'new-project' });
    const saveMock = vi.fn(
      async (): Promise<CommandOutcome<SaveOutcome>> => refusedBy({ code: 'FAILED', message: 'disk full' }),
    );
    useProjectStore.setState({ saveProject: saveMock, isDirty: false });

    const proceed = await useProjectStore.getState().saveAfterImport();

    expect(proceed).toBe(false);
    expect(useProjectStore.getState().isDirty).toBe(true);
  });

  it('never throws when saveProject rejects (navigation is blocked, project stays dirty)', async () => {
    useUIStore.setState({ importContext: 'new-project' });
    const saveMock = vi.fn(async (): Promise<CommandOutcome<SaveOutcome>> => {
      throw new Error('ipc died');
    });
    useProjectStore.setState({ saveProject: saveMock, isDirty: false });
    const errSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    const proceed = await useProjectStore.getState().saveAfterImport();

    expect(proceed).toBe(false);
    expect(useProjectStore.getState().isDirty).toBe(true);
    expect(errSpy).toHaveBeenCalled();
  });
});
