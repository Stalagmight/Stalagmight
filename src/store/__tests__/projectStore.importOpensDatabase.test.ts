/**
 * A successful import means the project is ON DISK, and the store has to know
 * it (#394 review claim 2).
 *
 * The import dialogs create their project with `deferInitialSave`, so nothing is
 * written until an import lands; the backend's `finalize_deferred_import` then
 * writes it for EVERY import command. But only the DMP route ever told the
 * store, because only it goes on to call `saveAfterImport` - the
 * TML/Compass/Walls/GPX dialogs finish through `completeImportToSingleView`,
 * which touches no save state. That left `databaseOpen` false over a file that
 * exists, and it is exactly the flag the DB-backed surfaces read
 * (`WarningResolution`'s resolve actions, `DivePlanBar`'s plan persistence), so
 * they stayed disabled for the rest of the session.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeIpcMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown) => String(e),
}));

import { useProjectStore } from '../projectStore';

const deferred = () => {
  useProjectStore.setState({
    databaseOpen: false,
    refreshProjectState: vi.fn().mockResolvedValue(undefined),
    checkProjectIntegrity: vi.fn().mockResolvedValue(undefined),
  } as Partial<ReturnType<typeof useProjectStore.getState>> as never);
};

describe('an import resolves the deferred first save (#394)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockResolvedValue({ success: true, warnings: [] });
    deferred();
  });

  /**
   * The non-DMP path, which has no `saveAfterImport` of its own.
   *
   * Injection: drop the `noteImportResolvedDeferredSave` call from
   * `executeCommandInner` and `databaseOpen` stays false after a TML import that
   * really did write the file.
   */
  it('opens the database flag after a TML import', async () => {
    await useProjectStore.getState().executeTmlImport('/caves/x.tml');

    expect(useProjectStore.getState().databaseOpen).toBe(true);
  });

  it('does the same for Compass, Walls, GPX and DMP sections', async () => {
    const runs: Array<() => Promise<unknown>> = [
      () => useProjectStore.getState().executeCompassImport('/caves/x.mak'),
      () => useProjectStore.getState().executeWallsImport('/caves/x.wpj'),
      () => useProjectStore.getState().executeGpxImport('/caves/x.gpx', null, true, true, true, [], false),
      () => useProjectStore.getState().importSections([]),
    ];
    for (const run of runs) {
      deferred();
      await run();
      expect(useProjectStore.getState().databaseOpen).toBe(true);
    }
  });

  /**
   * #674: adding a `.stalx` into a never-saved project goes through the same
   * `finalize_deferred_import` (`add_project_file`), so it writes the file too.
   *
   * Injection: drop `add_project_file` from `IMPORT_COMMANDS` and the flag stays
   * false after an add that really did write the file.
   */
  it('does the same for adding a .stalx to the open project', async () => {
    await useProjectStore.getState().addProjectFile('/caves/other.stalx');

    expect(useProjectStore.getState().databaseOpen).toBe(true);
  });

  /**
   * The deferred save reports its own failure as a `SYSTEM` warning ("Imported,
   * but could not save the new project to disk"), and then there IS no file -
   * opening the flag would enable surfaces that write to nothing.
   *
   * Injection: drop the warning check and this reports an open database over an
   * import that never reached disk.
   */
  it('leaves the flag down when the deferred save failed', async () => {
    invokeIpcMock.mockResolvedValue({
      success: true,
      warnings: [
        {
          code: 'SYSTEM',
          message: 'Imported, but could not save the new project to disk: permission denied.',
          severity: 'warning',
        },
      ],
    });

    await useProjectStore.getState().executeTmlImport('/caves/x.tml');

    expect(useProjectStore.getState().databaseOpen).toBe(false);
  });

  /** Written, but the handle could not be reopened - `saveProject` refuses to
   *  open the gate on this too, and for the same reason. */
  it('leaves the flag down when the database could not be reopened', async () => {
    invokeIpcMock.mockResolvedValue({
      success: true,
      warnings: [
        { code: 'DATABASE_NOT_REOPENED', message: 'Reopen failed', severity: 'warning' },
      ],
    });

    await useProjectStore.getState().executeTmlImport('/caves/x.tml');

    expect(useProjectStore.getState().databaseOpen).toBe(false);
  });

  /** A non-import command says nothing about the file's existence. */
  it('is not flipped by an ordinary edit command', async () => {
    await useProjectStore.getState().executeBatch([]);

    expect(useProjectStore.getState().databaseOpen).toBe(false);
  });
});
