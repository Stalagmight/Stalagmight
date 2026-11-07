/**
 * [#576 W75-NF1] An ALREADY_OPEN answer for a file the store does not show re-reads
 * the backend's project as a new session (`loadProject`), but three of the open
 * file's properties come only with a load answer: the newer-file read-only verdict
 * (`newerSchemaVersion`), the unreadable SpeleoDB link (`speleoLinkUnreadable`) and
 * the stored settings the open could not read (`unreadableProjectSettings`, which
 * refuse every save). The re-read kept whatever the store held - the previous
 * project's values, or a fresh page's defaults after a webview reload - so a newer
 * file opened editable, and a save the backend refuses showed no banner.
 *
 * The re-read now reads them from the backend's session (`speleodb_open_file_flags`),
 * as the in-place tree swap does. Each field is pinned both ways: CaveB's value is
 * taken, and CaveA's does not stay. Then the reload shape (no project shown), the
 * guard (ALREADY_OPEN for the project shown reads nothing), and a failed read.
 *
 * Store tier: the real stores; the load, the state read and the IPC are mocked. All
 * data is synthetic.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { invokeIpcMock } from '../../test/factories/ipc';
import { makeProject } from '../../test/factories/survey';
import type { OpenFileFlags } from '../../types/ipc.generated';
import type { Project } from '../../types/project';
import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';

const { loadBinaryMock, fetchStateMock } = vi.hoisted(() => ({ loadBinaryMock: vi.fn(), fetchStateMock: vi.fn() }));
vi.mock('../../utils/ipc');
vi.mock('../../utils/loadProjectBinary', () => ({
  loadProjectFullBinary: loadBinaryMock,
  fetchProjectStateBinary: fetchStateMock,
}));

const CAVE_A = '/caves/CaveA.stalx';
const CAVE_B = '/caves/CaveB.stalx';
const CAVE_A_PROJECT = makeProject({ name: 'CaveA', created_at: '2026-06-01T10:00:00Z', file_path: CAVE_A });
const CAVE_B_PROJECT = makeProject({ name: 'CaveB', created_at: '2026-07-01T10:00:00Z', file_path: CAVE_B });

const ALREADY_OPEN = { code: 'ALREADY_OPEN', message: 'already open' };

const CLEAN: OpenFileFlags = { newerSchemaVersion: null, speleoLinkUnreadable: false, unreadableProjectSettings: [] };

/** The backend's state, as `get_project_state_binary` decodes it. */
const stateOf = (project: Project) => ({
  project,
  stations: [],
  surveyData: { shots: [], stations: [] },
  stats: null,
  frozen_shots: [],
});

/** What `speleodb_open_file_flags` answers for the file the backend has open. */
let openFileFlags: OpenFileFlags | Error = CLEAN;

const flagsOf = () => {
  const s = useProjectStore.getState();
  return {
    newerSchemaVersion: s.newerSchemaVersion,
    speleoLinkUnreadable: s.speleoLinkUnreadable,
    unreadableProjectSettings: s.unreadableProjectSettings,
  };
};

interface FieldCase {
  /** The field, as the test title names it. */
  name: string;
  /** CaveB's file has it. */
  flagged: Partial<OpenFileFlags>;
}

const FIELDS: FieldCase[] = [
  { name: 'the newer-file read-only verdict', flagged: { newerSchemaVersion: 99 } },
  { name: 'the unreadable SpeleoDB link', flagged: { speleoLinkUnreadable: true } },
  { name: 'the unreadable project settings', flagged: { unreadableProjectSettings: ['gas_config', 'validation'] } },
];

beforeEach(() => {
  vi.clearAllMocks();
  openFileFlags = CLEAN;
  loadBinaryMock.mockRejectedValue(ALREADY_OPEN);
  fetchStateMock.mockResolvedValue(stateOf(CAVE_B_PROJECT));
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'get_project_warnings' || command === 'list_recent_projects') return [];
    if (command === 'load_display_settings') return {};
    if (command === 'speleodb_open_file_flags') {
      if (openFileFlags instanceof Error) throw openFileFlags;
      return { ...openFileFlags };
    }
    return undefined;
  });
  useProjectStore.setState({
    project: CAVE_A_PROJECT,
    databaseOpen: true,
    isDirty: false,
    ...CLEAN,
    loading: { loadingId: 0, clientOpId: 0, isLoading: false, overallProgress: 0 },
    refreshUndoState: vi.fn(async () => {}),
  } as never);
  useUIStore.setState({ view: 'single', appMode: 'survey', showToast: vi.fn() } as never);
});

describe('ALREADY_OPEN for CaveB while the store shows CaveA (#576 W75-NF1)', () => {
  it.each(FIELDS)("takes CaveB's $name", async ({ flagged }) => {
    openFileFlags = { ...CLEAN, ...flagged };

    const outcome = await useProjectStore.getState().loadProject(CAVE_B);

    expect(outcome).toEqual({ success: true });
    expect(useProjectStore.getState().project?.file_path).toBe(CAVE_B);
    expect(flagsOf()).toEqual({ ...CLEAN, ...flagged });
  });

  it.each(FIELDS)("does not keep CaveA's $name", async ({ flagged }) => {
    useProjectStore.setState({ ...flagged });

    const outcome = await useProjectStore.getState().loadProject(CAVE_B);

    expect(outcome).toEqual({ success: true });
    expect(useProjectStore.getState().project?.file_path).toBe(CAVE_B);
    expect(flagsOf()).toEqual(CLEAN);
  });
});

describe('ALREADY_OPEN after a webview reload (no project shown)', () => {
  it("takes the open file's flags into a store that shows no project", async () => {
    useProjectStore.setState({ project: null, databaseOpen: false });
    useUIStore.setState({ view: 'global' } as never);
    openFileFlags = { newerSchemaVersion: 42, speleoLinkUnreadable: true, unreadableProjectSettings: ['gas_config'] };

    const outcome = await useProjectStore.getState().loadProject(CAVE_B);

    expect(outcome).toEqual({ success: true });
    expect(useProjectStore.getState().project?.file_path).toBe(CAVE_B);
    expect(useUIStore.getState().view).toBe('single');
    expect(flagsOf()).toEqual({ newerSchemaVersion: 42, speleoLinkUnreadable: true, unreadableProjectSettings: ['gas_config'] });
  });
});

describe('ALREADY_OPEN for the project the store already shows (the guard)', () => {
  it("reads no flags and keeps the live project's", async () => {
    useProjectStore.setState({ newerSchemaVersion: 7 });
    openFileFlags = CLEAN;

    const outcome = await useProjectStore.getState().loadProject(CAVE_A);

    expect(outcome).toEqual({ success: true });
    expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_open_file_flags', undefined, expect.anything());
    expect(useProjectStore.getState().newerSchemaVersion).toBe(7);
  });
});

describe('a failed flags read', () => {
  it('still opens CaveB, keeping the flags the store showed (as the tree swap does)', async () => {
    useProjectStore.setState({ newerSchemaVersion: 7 });
    openFileFlags = new Error('synthetic IPC failure');
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});

    const outcome = await useProjectStore.getState().loadProject(CAVE_B);

    expect(outcome).toEqual({ success: true });
    expect(useProjectStore.getState().project?.file_path).toBe(CAVE_B);
    expect(useProjectStore.getState().newerSchemaVersion).toBe(7);
    expect(consoleError).toHaveBeenCalled();
    consoleError.mockRestore();
  });
});
