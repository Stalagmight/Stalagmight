import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';

const invokeIpcMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
}));

/**
 * `saveProject(path, isAutosave)`'s second argument leaves the backend save
 * identical either way - it crosses IPC only to name the trigger in the Rust
 * log line (#429). Its real job is keeping the 30-second debounced background
 * save quiet:
 *
 *  1. a folder-access denial must NOT reopen the save-permission dialog on every
 *     retry (the user would be unable to do anything else while access is denied), and
 *  2. a successful save must NOT re-fetch the project: the `.stalx` v2 save
 *     heals nothing, so there is no new information to pull. (0.6.0 re-checked
 *     integrity after every manual save; that check is gone with its banner.)
 *
 * Neither guard had a test, which is how a refactor briefly removed both. This
 * pins them.
 */
describe('saveProject autosave quieting', () => {
  let openDialogSpy: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    openDialogSpy = vi.fn();
    useUIStore.setState({ openDialog: openDialogSpy } as never);
    useProjectStore.setState({
      project: { file_path: '/cave.stalx' } as never,
      // An autosave only ever runs on a dirty project, so that is the state the
      // failure paths below are exercised from.
      isDirty: true,
      autosaveFailure: null,
      refreshUndoState: vi.fn().mockResolvedValue(undefined),
      refreshProjectState: vi.fn().mockResolvedValue(undefined),
    } as never);
  });

  afterEach(() => {
    useProjectStore.setState({ project: null, isDirty: false, autosaveFailure: null } as never);
  });

  const resolveSave = () => {
    invokeIpcMock.mockImplementation(async (command: string) =>
      command === 'save_project' ? { success: true, saved_path: '/cave.stalx' } : undefined
    );
  };

  const denyPermission = () => {
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'save_project') {
        return { success: false, error: 'FS_PERMISSION_DENIED: folder access denied' };
      }
      return undefined;
    });
  };

  it('does not reopen the save-permission dialog on an autosave', async () => {
    denyPermission();

    await useProjectStore.getState().saveProject('/cave.stalx', true);

    expect(openDialogSpy).not.toHaveBeenCalledWith('save-permission', expect.anything());
    // The failure is not swallowed - it feeds the autosave-failure banner,
    // without leaking the raw FS_PERMISSION_DENIED marker.
    const banner = useProjectStore.getState().autosaveFailure;
    expect(banner).toBeTruthy();
    expect(banner).not.toContain('FS_PERMISSION_DENIED');
  });

  it('DOES open the save-permission dialog on a manual save', async () => {
    denyPermission();

    await useProjectStore.getState().saveProject('/cave.stalx');

    expect(openDialogSpy).toHaveBeenCalledWith('save-permission', expect.anything());
  });

  it('re-fetches nothing after a successful autosave or manual save', async () => {
    resolveSave();

    await useProjectStore.getState().saveProject('/cave.stalx', true);
    await useProjectStore.getState().saveProject('/cave.stalx');

    expect(useProjectStore.getState().refreshProjectState).not.toHaveBeenCalled();
    // The save itself is the only IPC (undo availability goes through the
    // mocked store action).
    expect(invokeIpcMock.mock.calls.map((c) => c[0])).toEqual(['save_project', 'save_project']);
  });

  it('sends isAutosave over IPC so the backend log can name the trigger (#429)', async () => {
    resolveSave();

    await useProjectStore.getState().saveProject('/cave.stalx', true);
    await useProjectStore.getState().saveProject('/cave.stalx');

    const calls = invokeIpcMock.mock.calls.filter((c) => c[0] === 'save_project');
    // Always a boolean, never undefined: the Rust side reads it as "manual"
    // when absent, and a manual save should say so rather than default to it.
    expect(calls[0]?.[1]).toEqual({ path: '/cave.stalx', isAutosave: true });
    expect(calls[1]?.[1]).toEqual({ path: '/cave.stalx', isAutosave: false });
  });

  it('feeds the banner a destination failure without leaking the marker (#429)', async () => {
    invokeIpcMock.mockImplementation(async (command: string) =>
      command === 'save_project'
        ? { success: false, error: 'FS_PATH_NOT_FOUND: Cannot save to Z:\\cave.stalx' }
        : undefined
    );

    await useProjectStore.getState().saveProject('/cave.stalx', true);

    const banner = useProjectStore.getState().autosaveFailure;
    expect(banner).toBeTruthy();
    expect(banner).not.toContain('FS_PATH_NOT_FOUND');
    // A missing folder is not a permission problem, so no guidance dialog.
    expect(openDialogSpy).not.toHaveBeenCalledWith('save-permission', expect.anything());
  });

  // [#517] The KIND reaches the store beside the message (marker already
  // stripped): `useAutoSave` reads it to decide whether to pause.
  it('records the unreadable-ignore-list kind beside its message', async () => {
    invokeIpcMock.mockImplementation(async (command: string) =>
      command === 'save_project'
        ? {
            success: false,
            error:
              "IGNORES_UNREADABLE: Can't save: this project's list of ignored warnings could not be read (x).",
          }
        : undefined
    );

    await useProjectStore.getState().saveProject('/cave.stalx', true);

    const { autosaveFailure, autosaveFailureKind } = useProjectStore.getState();
    expect(autosaveFailureKind).toBe('ignores-unreadable');
    expect(autosaveFailure).not.toContain('IGNORES_UNREADABLE');
    expect(autosaveFailure).toContain('list of ignored warnings could not be read');
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
        return { success: false, error: 'FS_PERMISSION_DENIED: folder access denied' };
      }
      return undefined;
    });

    await useProjectStore.getState().saveProject('/cave.stalx', true);

    expect(useProjectStore.getState().autosaveFailure).toBeNull();
  });
});
