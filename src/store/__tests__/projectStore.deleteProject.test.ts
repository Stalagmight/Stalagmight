import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

const PATH = '/projects/cave-alpha.stalx';

describe('projectStore.deleteProject', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Default: the delete succeeds and closed nothing - the shape of the common
    // global-view delete of a project that is not the open one. The outcome always
    // carries `session_closed` (#576 A12, `DeleteProjectOutcome`): false here.
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'delete_project') return { session_closed: false, warnings: [] };
      if (command === 'list_recent_projects') return [];
      if (command === 'get_config') return { general: { recent_projects: [] } };
      return undefined;
    });
    useProjectStore.setState({ recentProjects: [], config: null, project: null });
  });

  it('calls delete_project with the path and refreshes recents + config on success', async () => {
    const result = await useProjectStore.getState().deleteProject(PATH);

    expect(result.success).toBe(true);
    expect(invokeIpcMock).toHaveBeenCalledWith('delete_project', { path: PATH }, expect.any(Object));
    // The refresh (so the deleted project leaves both available and "missing" rows)
    // fires only on success.
    expect(invokeIpcMock).toHaveBeenCalledWith('list_recent_projects', undefined, expect.any(Object));
    expect(invokeIpcMock).toHaveBeenCalledWith('get_config', undefined, expect.any(Object));
    // The teardown verdict rides on the delete's own result, so there is no
    // follow-up "did that close the open project?" round trip to make - not even
    // when a project IS open. `get_project_state` clones the project and recomputes
    // coordinates/stats/penetration, which is far too much work for one bit.
    expect(invokeIpcMock).not.toHaveBeenCalledWith('get_project_state', undefined, expect.any(Object));
    // ...nor its binary form, which is what the state refresh calls (#576 A9-D2).
    expect(invokeIpcMock).not.toHaveBeenCalledWith('get_project_state_binary', undefined, expect.any(Object));
  });

  it('does not refresh recents when the delete fails', async () => {
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'delete_project') throw { code: 'FAILED', message: 'boom' };
      return undefined;
    });

    const result = await useProjectStore.getState().deleteProject(PATH);

    expect(result.success).toBe(false);
    expect(result.error).toBe('boom');
    expect(invokeIpcMock).not.toHaveBeenCalledWith('list_recent_projects', undefined, expect.any(Object));
  });

  it('closes the project when the deleted file is the OPEN one', async () => {
    // The backend tears its own session down (project, SQLite handle, undo stack)
    // before handing the `.stalx` to the OS delete, and says so with
    // `session_closed`, so the frontend has to follow. Leaving `project` set kept
    // the UI rendering a project whose file is gone - and the next autosave tick
    // failed with "No project open".
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'delete_project') return { session_closed: true, warnings: [] };
      if (command === 'list_recent_projects') return [];
      if (command === 'get_config') return { general: { recent_projects: [] } };
      return undefined;
    });
    useProjectStore.setState({
      project: { file_path: PATH } as never,
      databaseOpen: true,
      isDirty: true,
      stations: [{} as never],
      integrityIssues: ['something'],
    } as never);

    const result = await useProjectStore.getState().deleteProject(PATH);

    expect(result.success).toBe(true);
    const state = useProjectStore.getState();
    expect(state.project).toBeNull();
    expect(state.databaseOpen).toBe(false);
    expect(state.isDirty).toBe(false);
    expect(state.stations).toEqual([]);
    expect(state.integrityIssues).toEqual([]);
  });

  it('closes the project when the delete FAILS after the backend already closed it', async () => {
    // The backend closes BEFORE handing the file to the OS trash - it must, since
    // SQLite holds the file open without FILE_SHARE_DELETE and the Windows trash
    // would be denied while that handle lives. So a trash that is then denied for
    // some other reason returns FAILURE with the session already gone, and the
    // backend sets `session_closed` on that failure result too. Guarding the
    // frontend teardown on `result.success` left a phantom project behind, and the
    // next save or edit failed with "No project open".
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'delete_project') {
        throw { code: 'FAILED', message: 'Access is denied', session_closed: true };
      }
      return undefined;
    });
    useProjectStore.setState({
      project: { file_path: PATH } as never,
      databaseOpen: true,
      isDirty: true,
      stations: [{} as never],
    } as never);

    const result = await useProjectStore.getState().deleteProject(PATH);

    expect(result.success).toBe(false);
    expect(result.error).toBe('Access is denied');
    const state = useProjectStore.getState();
    expect(state.project).toBeNull();
    expect(state.databaseOpen).toBe(false);
    expect(state.stations).toEqual([]);
    // The file survives a failed delete, so recents must still NOT be refreshed.
    expect(invokeIpcMock).not.toHaveBeenCalledWith('list_recent_projects', undefined, expect.any(Object));
  });

  it('trusts the flag over the path: closes even when the deleted path is not the open one', async () => {
    // Whether the deleted `.stalx` IS the open project is the BACKEND's question,
    // and it answers it with `same_file` (canonicalizing, so a symlink, a `..`
    // segment or a Windows case / short-name variant still matches - covered by the
    // Rust tests). The frontend must therefore act on the flag alone and never
    // second-guess it by comparing path strings: `===` on the raw strings says
    // these two are different files, and a frontend that believed that kept
    // rendering a project the backend no longer had.
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'delete_project') return { session_closed: true, warnings: [] };
      if (command === 'list_recent_projects') return [];
      if (command === 'get_config') return { general: { recent_projects: [] } };
      return undefined;
    });
    useProjectStore.setState({
      project: { file_path: '/projects/cave.stalx' } as never,
      databaseOpen: true,
      stations: [{} as never],
    } as never);

    const result = await useProjectStore.getState().deleteProject('/projects/../projects/cave.stalx');

    expect(result.success).toBe(true);
    const state = useProjectStore.getState();
    expect(state.project).toBeNull();
    expect(state.databaseOpen).toBe(false);
    expect(state.stations).toEqual([]);
  });

  it('leaves the open project alone when the backend did not close it', async () => {
    // Deleting a recent that is not the open project must not close the one the
    // user is working in - that would throw away its undo history for nothing. The
    // backend answering `session_closed: false` is what proves the delete hit some
    // other file, so this is the case the frontend must NOT act on.
    const project = { file_path: '/projects/other.stalx' } as never;
    useProjectStore.setState({ project, databaseOpen: true } as never);

    const result = await useProjectStore.getState().deleteProject(PATH);

    expect(result.success).toBe(true);
    expect(useProjectStore.getState().project).toBe(project);
    expect(useProjectStore.getState().databaseOpen).toBe(true);
  });

  it('surfaces a thrown IPC error as a failed result', async () => {
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'delete_project') throw new Error('ipc exploded');
      return undefined;
    });

    const result = await useProjectStore.getState().deleteProject(PATH);

    expect(result.success).toBe(false);
    expect(result.error).toContain('ipc exploded');
  });
});
