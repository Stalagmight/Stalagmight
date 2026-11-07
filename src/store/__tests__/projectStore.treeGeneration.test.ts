/**
 * [#576 W75-NF7] A save names the tree generation its tree came with, so the backend
 * can refuse one made for a tree an in-place SpeleoDB swap replaced since (same file,
 * same `created_at`: the identity alone cannot tell the two trees apart). The store
 * learns the generation from the load and refresh envelopes (`tree_generation`) and
 * forgets it on a create and a close; `expectedProjectOf` sends it with the identity,
 * captured when the request is issued.
 *
 * Store tier: the real stores; the load, the state read and the IPC are mocked. All
 * data is synthetic.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { invokeIpcMock } from '../../test/factories/ipc';
import { makeProject } from '../../test/factories/survey';
import type { Project } from '../../types/project';
import { expectedOpenProject } from '../expectedOpenProject';
import { noteOpenTreeGeneration } from '../projectSession';
import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';

const { loadBinaryMock, fetchStateMock } = vi.hoisted(() => ({ loadBinaryMock: vi.fn(), fetchStateMock: vi.fn() }));
vi.mock('../../utils/ipc');
vi.mock('../../utils/loadProjectBinary', () => ({
  loadProjectFullBinary: loadBinaryMock,
  fetchProjectStateBinary: fetchStateMock,
}));

const CAVE = '/caves/CaveA.stalx';
const CREATED_AT = '2026-06-01T10:00:00Z';
const PROJECT = makeProject({ name: 'CaveA', created_at: CREATED_AT, file_path: CAVE });

const stateOf = (project: Project, treeGeneration?: number) => ({
  project,
  stations: [],
  surveyData: { shots: [], stations: [] },
  stats: null,
  frozen_shots: [],
  ...(treeGeneration === undefined ? {} : { treeGeneration }),
});

const loadResult = (treeGeneration?: number) => ({
  ...stateOf(PROJECT, treeGeneration),
  loadingId: 1,
  isRemote: false,
  warnings: [],
  displaySettings: {},
});

/** The `expectedProject` every `save_project` call carried, in order. */
const savesSent = () =>
  invokeIpcMock.mock.calls
    .filter(([command]) => command === 'save_project')
    .map(([, args]) => (args as { expectedProject: unknown }).expectedProject);

beforeEach(() => {
  vi.clearAllMocks();
  noteOpenTreeGeneration(null);
  fetchStateMock.mockResolvedValue(stateOf(PROJECT));
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'get_project_warnings' || command === 'list_recent_projects') return [];
    if (command === 'load_display_settings') return {};
    if (command === 'save_project') return { saved_path: CAVE, warnings: [] };
    if (command === 'take_unrecognised_shot_kinds') return [];
    if (command === 'check_project_integrity') return { messages: [] };
    return undefined;
  });
  useProjectStore.setState({
    project: null,
    databaseOpen: false,
    isDirty: false,
    loading: { loadingId: 0, clientOpId: 0, isLoading: false, overallProgress: 0 },
    refreshUndoState: vi.fn(async () => {}),
  } as never);
  useUIStore.setState({ view: 'global', appMode: 'survey', showToast: vi.fn() } as never);
});

describe('the tree generation a save names (#576 W75-NF7)', () => {
  it('is the one the load answer carried', async () => {
    loadBinaryMock.mockResolvedValue(loadResult(3));
    expect(await useProjectStore.getState().loadProject(CAVE)).toEqual({ success: true });

    await useProjectStore.getState().saveProject();

    expect(savesSent()).toEqual([{ createdAt: CREATED_AT, filePath: CAVE, treeGeneration: 3 }]);
  });

  it("moves with a refresh, and a save issued before the swap's refresh keeps the old one", async () => {
    loadBinaryMock.mockResolvedValue(loadResult(3));
    await useProjectStore.getState().loadProject(CAVE);

    // A save issued before the in-place swap's refresh answers is made for the old tree.
    let answerSave!: (v: unknown) => void;
    invokeIpcMock.mockImplementationOnce(async (command: string) => {
      expect(command).toBe('save_project');
      return new Promise((resolve) => (answerSave = resolve));
    });
    const earlySave = useProjectStore.getState().saveProject();
    // The swap's refresh brings the new tree and its generation.
    fetchStateMock.mockResolvedValueOnce(stateOf(PROJECT, 4));
    await useProjectStore.getState().refreshProjectState();
    answerSave({ saved_path: CAVE, warnings: [] });
    await earlySave;
    await useProjectStore.getState().saveProject();

    expect(savesSent()).toEqual([
      { createdAt: CREATED_AT, filePath: CAVE, treeGeneration: 3 },
      { createdAt: CREATED_AT, filePath: CAVE, treeGeneration: 4 },
    ]);
  });

  it('is what a dive-plan save names too (expectedOpenProject)', async () => {
    loadBinaryMock.mockResolvedValue(loadResult(7));
    await useProjectStore.getState().loadProject(CAVE);

    expect(expectedOpenProject()).toEqual({ createdAt: CREATED_AT, filePath: CAVE, treeGeneration: 7 });
  });

  it('is not named when the envelope names none (a backend that predates it)', async () => {
    loadBinaryMock.mockResolvedValue(loadResult());
    await useProjectStore.getState().loadProject(CAVE);

    expect(expectedOpenProject()).toEqual({ createdAt: CREATED_AT, filePath: CAVE });
  });

  it('is forgotten when the project closes', async () => {
    loadBinaryMock.mockResolvedValue(loadResult(3));
    await useProjectStore.getState().loadProject(CAVE);
    invokeIpcMock.mockImplementation(async (command: string) =>
      command === 'close_project' ? { success: true } : undefined,
    );

    await useProjectStore.getState().closeProject();
    useProjectStore.setState({ project: PROJECT });

    expect(expectedOpenProject()).toEqual({ createdAt: CREATED_AT, filePath: CAVE });
  });
});
