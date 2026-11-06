import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { shouldShowAutosaveBanner } from '../../utils/saveFeedback';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

/**
 * `saveProject(path, isAutosave)`'s second argument leaves the backend save
 * identical either way - it crosses IPC only to name the trigger in the Rust
 * log line (#429). Its real job is keeping the 30-second debounced background
 * save quiet:
 *
 *  1. a folder-access denial must NOT reopen the save-permission dialog on every
 *     retry (the user would be unable to do anything else while access is denied), and
 *  2. a clean autosave must NOT trigger a post-save integrity re-check, which on a
 *     large project is real churn for no new information.
 *
 * Neither guard had a test, which is how a refactor briefly removed both. This
 * pins them.
 */
describe('saveProject autosave quieting', () => {
  let openDialogSpy: ReturnType<typeof vi.fn>;
  let checkIntegritySpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    openDialogSpy = vi.fn();
    checkIntegritySpy = vi.fn().mockResolvedValue(undefined);
    useUIStore.setState({ openDialog: openDialogSpy } as never);
    useProjectStore.setState({
      project: { file_path: '/cave.stalx' } as never,
      // An autosave only ever runs on a dirty project, so that is the state the
      // failure paths below are exercised from.
      isDirty: true,
      autosaveFailure: null,
      integrityIssues: [],
      integritySaveBlocked: false,
      refreshUndoState: vi.fn().mockResolvedValue(undefined),
      refreshProjectState: vi.fn().mockResolvedValue(undefined),
      checkProjectIntegrity: checkIntegritySpy,
    } as never);
  });

  afterEach(() => {
    useProjectStore.setState({ project: null, isDirty: false, autosaveFailure: null } as never);
  });

  const resolveSave = () => {
    invokeIpcMock.mockImplementation(async (command: string) =>
      command === 'save_project' ? { saved_path: '/cave.stalx', unlinked_copy_path: null, warnings: [] } : undefined
    );
  };

  const denyPermission = () => {
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'save_project') {
        throw { code: 'FS_PERMISSION_DENIED', message: 'folder access denied' };
      }
      return undefined;
    });
  };

  it('does not reopen the save-permission dialog on an autosave', async () => {
    denyPermission();

    await useProjectStore.getState().saveProject('/cave.stalx', true);

    expect(openDialogSpy).not.toHaveBeenCalledWith('save-permission', expect.anything());
    // The failure is not swallowed - it feeds the autosave-failure banner, in its
    // reworded line (never the code).
    const banner = useProjectStore.getState().autosaveFailure;
    expect(banner).toBeTruthy();
    expect(banner).not.toContain('FS_PERMISSION_DENIED');
  });

  it('DOES open the save-permission dialog on a manual save', async () => {
    denyPermission();

    await useProjectStore.getState().saveProject('/cave.stalx');

    expect(openDialogSpy).toHaveBeenCalledWith('save-permission', expect.anything());
  });

  it('skips the post-save integrity re-check on a clean autosave', async () => {
    resolveSave();

    await useProjectStore.getState().saveProject('/cave.stalx', true);

    expect(checkIntegritySpy).not.toHaveBeenCalled();
  });

  it('still re-checks integrity on a manual save', async () => {
    resolveSave();

    await useProjectStore.getState().saveProject('/cave.stalx');

    expect(checkIntegritySpy).toHaveBeenCalled();
  });

  it('sends isAutosave over IPC so the backend log can name the trigger (#429)', async () => {
    resolveSave();

    await useProjectStore.getState().saveProject('/cave.stalx', true);
    await useProjectStore.getState().saveProject('/cave.stalx');

    const calls = invokeIpcMock.mock.calls.filter((c) => c[0] === 'save_project');
    // Always a boolean, never undefined: the Rust side reads it as "manual"
    // when absent, and a manual save should say so rather than default to it.
    // (#576 FZ-FZ-INT-4: and the project it was made for, none open here.)
    expect(calls[0]?.[1]).toEqual({ path: '/cave.stalx', isAutosave: true, expectedProject: null });
    expect(calls[1]?.[1]).toEqual({ path: '/cave.stalx', isAutosave: false, expectedProject: null });
  });

  it('feeds the banner a destination failure without leaking the code (#429)', async () => {
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'save_project') throw { code: 'FS_PATH_NOT_FOUND', message: 'Cannot save to Z:\\cave.stalx' };
      return undefined;
    });

    await useProjectStore.getState().saveProject('/cave.stalx', true);

    const banner = useProjectStore.getState().autosaveFailure;
    expect(banner).toBeTruthy();
    expect(banner).not.toContain('FS_PATH_NOT_FOUND');
    // A missing folder is not a permission problem, so no guidance dialog.
    expect(openDialogSpy).not.toHaveBeenCalledWith('save-permission', expect.anything());
  });

  // [#517] The banner's render gate hides an ordinary failure behind the
  // integrity banner, so the KIND has to reach the store - the message alone
  // cannot tell `shouldShowAutosaveBanner` that this is the one failure Review &
  // Repair cannot fix.
  it('records the unreadable-ignore-list kind so the banner can refuse to yield', async () => {
    useProjectStore.setState({
      integritySaveBlocked: true,
      integrityIssues: [{ category: 'dangling_merge_ref', message: 'x' }],
    } as never);
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'save_project') {
        throw {
          code: 'IGNORES_UNREADABLE',
          message: "Can't save: this project's list of ignored warnings could not be read (x).",
        };
      }
      return undefined;
    });

    await useProjectStore.getState().saveProject('/cave.stalx', true);

    const { autosaveFailure, autosaveFailureKind } = useProjectStore.getState();
    expect(autosaveFailureKind).toBe('ignores-unreadable');
    expect(autosaveFailure).not.toContain('IGNORES_UNREADABLE');
    expect(autosaveFailure).toContain('list of ignored warnings could not be read');
    // The whole point: this combination still renders.
    expect(
      shouldShowAutosaveBanner({
        autosaveFailure,
        autosaveFailureKind,
        integritySaveBlocked: useProjectStore.getState().integritySaveBlocked,
        integrityIssueCount: useProjectStore.getState().integrityIssues.length,
      })
    ).toBe(true);
  });

  it('raises the banner when the autosave fails and the changes really are unsaved', async () => {
    denyPermission();

    await useProjectStore.getState().saveProject('/cave.stalx', true);

    expect(useProjectStore.getState().autosaveFailure).toBeTruthy();
  });

  it('leaves the banner down when a manual save landed while the autosave was in flight (#429)', async () => {
    // The manual Ctrl+S completes mid-flight: the store goes clean before this
    // autosave's failure comes back. "Your latest changes are not on disk" would
    // be false, and the banner has no dismiss the user could reach for.
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'save_project') {
        useProjectStore.setState({ isDirty: false } as never);
        throw { code: 'FS_PERMISSION_DENIED', message: 'folder access denied' };
      }
      return undefined;
    });

    await useProjectStore.getState().saveProject('/cave.stalx', true);

    expect(useProjectStore.getState().autosaveFailure).toBeNull();
  });
});
