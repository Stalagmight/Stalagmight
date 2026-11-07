/**
 * [#576 FZ-FZ-INT-4, FZ-FZ-INT-7] Saves and edits around a project switch.
 *
 *  - A switch waits for the saves already sent (and, since D23, the edits already
 *    queued) before it sends its own IPC, so each lands in the project it was made
 *    for.
 *  - A save or an edit REQUESTED once a switch has started is refused locally with
 *    the store's PROJECT_CHANGED text, nothing sent, until the switch settles:
 *    before the switch answers nothing knows which project it would reach.
 *  - The backend's PROJECT_CHANGED answer is handled like that local refusal: no
 *    error toast, no error dialog, and an autosave refused this way is not an
 *    autosave failure.
 *
 * Store tier: the real store; the IPC is the shared mock, with the calls the test
 * holds. All data is synthetic.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { deferred, timerTurns, type Deferred } from '../../test/factories/async';
import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';
import { makeProject } from '../../test/factories/survey';
import { surfaceSaveFailure } from '../../utils/saveFeedback';
import { PROJECT_CHANGED_REFUSAL } from '../commandResult';
import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';

const loadBinaryMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc');
vi.mock('../../utils/loadProjectBinary', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/loadProjectBinary')>()),
  loadProjectFullBinary: loadBinaryMock,
}));
emptyBinaryRefresh();

const CAVE_A = '/caves/CaveA.stalx';
const BACKEND_PROJECT_CHANGED = { code: 'PROJECT_CHANGED', message: 'Save was not applied: another project is open now.' };
const SECTION = { name: 'S', color: '#336699', shots: [] } as never;

/** The held answers, by command. */
let held: Map<string, Deferred<unknown>>;
let sent: string[];
/** The loads in flight: the switch stays under way until the test ends it. */
let loads: Deferred<unknown>[];

beforeEach(() => {
  vi.clearAllMocks();
  held = new Map();
  sent = [];
  loads = [];
  loadBinaryMock.mockImplementation(() => {
    sent.push('load_project_full_binary');
    const load = deferred<unknown>();
    loads.push(load);
    return load.promise;
  });
  invokeIpcMock.mockImplementation((command: string) => {
    sent.push(command);
    const hold = held.get(command);
    if (hold) return hold.promise;
    if (command === 'check_project_integrity') return Promise.resolve({ messages: [], save_blocked: false });
    if (command === 'get_project_warnings') return Promise.resolve([]);
    return Promise.resolve({});
  });
  useUIStore.setState({ activeDialog: null, toasts: [] });
  useProjectStore.setState({
    project: makeProject({ created_at: '2026-07-01T10:00:00Z', file_path: CAVE_A }),
    isDirty: true,
    autosaveFailure: null,
    integrityIssues: [],
    loading: { loadingId: 0, clientOpId: 0, isLoading: false, overallProgress: 0 },
  } as never);
});

afterEach(async () => {
  // End every switch the test left under way: the gate is module state.
  for (const load of loads) load.reject({ code: 'CANCELLED', message: 'Loading cancelled' });
  await timerTurns(10);
});

describe('saves and edits around a project switch (#576 FZ-FZ-INT-4/7)', () => {
  it('a switch waits for a save already sent before it sends its own IPC', async () => {
    // The load imports these lazily; loaded first so "not sent yet" below is not just slow.
    await Promise.all([import('../../utils/memoryProbe'), import('../../utils/loadProjectBinary')]);
    const save = deferred<unknown>();
    held.set('save_project', save);
    const saving = useProjectStore.getState().saveProject();
    await timerTurns(3);
    void useProjectStore.getState().loadProject('/caves/CaveB.stalx');
    await timerTurns(5);
    expect(sent).not.toContain('load_project_full_binary');

    save.resolve({ saved_path: CAVE_A, warnings: [] });
    await saving;
    await vi.waitFor(() => expect(sent).toContain('load_project_full_binary'));
  });

  // [#576 Q-RVFc-1-D1] Refused with a quiet toast saying to wait (it used to be silent).
  it('an edit requested while a switch is under way is refused, nothing sent, a quiet toast says to wait', async () => {
    void useProjectStore.getState().loadProject('/caves/CaveB.stalx');
    await timerTurns(3);

    const edit = await useProjectStore.getState().createSection(SECTION);
    const undo = await useProjectStore.getState().undo();

    expect(edit).toMatchObject({ success: false, code: null, error: PROJECT_CHANGED_REFUSAL });
    expect(undo).toMatchObject({ success: false, code: null, error: PROJECT_CHANGED_REFUSAL });
    expect(sent).not.toContain('create_section');
    expect(sent).not.toContain('undo');
    // One toast for both refusals (one dedup key), quiet, and no error.
    expect(useUIStore.getState().toasts).toEqual([
      expect.objectContaining({ message: 'Wait for the project to finish opening.', key: 'edit-during-switch', kind: 'info' }),
    ]);
  });

  it('an edit made once the loaded project is on screen is sent, while the rest of the load runs', async () => {
    // The load's aftermath (the recent list) is still running when the user edits.
    const recents = deferred<unknown>();
    held.set('list_recent_projects', recents);
    const loading = useProjectStore.getState().loadProject('/caves/CaveB.stalx');
    await vi.waitFor(() => expect(loads).toHaveLength(1));
    loads[0]!.resolve({
      loadingId: 1, isRemote: false, newerSchemaVersion: null, speleoLinkUnreadable: false,
      unreadableProjectSettings: [], loadRepairs: null,
      project: makeProject({ created_at: '2026-07-02T10:00:00Z', file_path: '/caves/CaveB.stalx' }),
      stations: [], surveyData: { shots: [], stations: [] }, stats: null, warnings: [], frozen_shots: [],
      displaySettings: {},
    });
    await vi.waitFor(() => expect(sent).toContain('list_recent_projects'));
    expect(useProjectStore.getState().project?.file_path).toBe('/caves/CaveB.stalx');

    const edit = await useProjectStore.getState().createSection(SECTION);

    expect(edit).not.toMatchObject({ error: PROJECT_CHANGED_REFUSAL });
    expect(sent).toContain('create_section');
    recents.resolve([]);
    await loading;
  });

  it('a save requested while a switch is under way is refused, nothing sent', async () => {
    void useProjectStore.getState().loadProject('/caves/CaveB.stalx');
    await timerTurns(3);

    const save = await useProjectStore.getState().saveProject(undefined, true);

    expect(save).toMatchObject({ success: false, code: null, error: PROJECT_CHANGED_REFUSAL });
    expect(sent).not.toContain('save_project');
    expect(useProjectStore.getState().autosaveFailure).toBeNull();
  });

  it("the backend's PROJECT_CHANGED to an autosave is not an autosave failure", async () => {
    invokeIpcMock.mockImplementation((command: string) => {
      if (command === 'save_project') return Promise.reject(BACKEND_PROJECT_CHANGED);
      return Promise.resolve({});
    });

    const save = await useProjectStore.getState().saveProject(undefined, true);

    expect(save).toMatchObject({ success: false, code: 'PROJECT_CHANGED' });
    expect(useProjectStore.getState().autosaveFailure).toBeNull();
    expect(useProjectStore.getState().autosaveFailureKind).toBeNull();
  });

  it("the backend's PROJECT_CHANGED to an edit shows no error toast", async () => {
    invokeIpcMock.mockImplementation((command: string) => {
      if (command === 'create_section') return Promise.reject({ ...BACKEND_PROJECT_CHANGED, message: 'Create section was not applied: another project is open now.' });
      if (command === 'check_project_integrity') return Promise.resolve({ messages: [], save_blocked: false });
      return Promise.resolve([]);
    });

    const edit = await useProjectStore.getState().createSection(SECTION);

    expect(edit).toMatchObject({ success: false, code: 'PROJECT_CHANGED' });
    expect(useUIStore.getState().toasts).toEqual([]);
  });

  it('a warning resolve requested during a switch is refused, nothing sent; a backend PROJECT_CHANGED shows no toast', async () => {
    void useProjectStore.getState().loadProject('/caves/CaveB.stalx');
    await timerTurns(3);
    expect(await useProjectStore.getState().resolveWarning(1, 2, 'W')).toBe(false);
    expect(sent).not.toContain('resolve_warning');
    expect(useUIStore.getState().toasts).toEqual([expect.objectContaining({ key: 'edit-during-switch', kind: 'info' })]);
    useUIStore.setState({ toasts: [] });
    loads[0]!.reject({ code: 'CANCELLED', message: 'Loading cancelled' });
    await timerTurns(10);

    invokeIpcMock.mockImplementation((command: string) =>
      command === 'resolve_warning' ? Promise.reject(BACKEND_PROJECT_CHANGED) : Promise.resolve([]),
    );
    expect(await useProjectStore.getState().resolveWarning(1, 2, 'W')).toBe(false);
    expect(useUIStore.getState().toasts).toEqual([]);
  });

  it.each([
    ['the backend', { code: 'PROJECT_CHANGED' as const, error: BACKEND_PROJECT_CHANGED.message }],
    ['the store', { code: null, error: PROJECT_CHANGED_REFUSAL }],
  ])('a manual save refused with PROJECT_CHANGED by %s opens no error dialog', (_by, failure) => {
    const openDialog = vi.fn();
    surfaceSaveFailure(failure, openDialog, { integritySaveBlocked: false, openIntegrityDialog: vi.fn() });
    expect(openDialog).not.toHaveBeenCalled();
  });
});
