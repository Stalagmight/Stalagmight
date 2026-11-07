/**
 * [#576 W71-NF4] An ALREADY_OPEN answer for a file the store does not show re-reads
 * the backend's project, and must not leave the SpeleoDB edit gate as the PREVIOUS
 * project left it.
 *
 * A load resolves the gate for the project it opens before the view paints
 * (`syncProjectLink(result.isRemote, path)`, #270). The ALREADY_OPEN re-read has no
 * load answer, and it seeded the gate only when nothing had resolved it (`unknown`).
 * So after a LOCAL CaveA the gate stayed open (`local`) for a CaveB the backend has
 * open as a SpeleoDB project, and after a remote CaveA we held the lock on, CaveB
 * showed CaveA's editing link - until the unawaited, error-swallowing
 * `refreshLinkStatus` answered, which offline is never. The backend's own gate still
 * refuses the edit (EDIT_DISABLED), but every edit affordance was on.
 *
 * Now the re-read closes the gate the pessimistic way the unresolved case already
 * did - read-only for CaveB, with a status button to recover through - and the
 * status reply resolves it either way. A link seeded FOR CaveB on the way in (the
 * SpeleoDB open's `setLinkOnOpen`) is kept, as a load keeps it.
 *
 * Store tier: the real stores; the load, the state read and the status call are
 * mocked (the status call never answers, as offline). All data is synthetic.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { invokeIpcMock } from '../../test/factories/ipc';
import { makeSpeleoLink } from '../../test/factories/speleo';
import { makeProject } from '../../test/factories/survey';
import type { Project } from '../../types/project';
import { useProjectStore } from '../projectStore';
import { selectSpeleoReadOnly, useSpeleoStore } from '../speleoStore';
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

/** CaveA's link: a SpeleoDB project we hold the lock on. */
const CAVE_A_EDITING = makeSpeleoLink({ editState: 'editing', lockHolder: 'me@example.com', hasUnpushedEdits: true });

const stateOf = (project: Project) => ({
  project,
  stations: [],
  surveyData: { shots: [], stations: [] },
  stats: null,
  frozen_shots: [],
});

/** The gate as the shell reads it, for CaveB. */
const gate = () => {
  const s = useSpeleoStore.getState();
  return {
    readOnly: selectSpeleoReadOnly(s),
    kind: s.projectLinkKind,
    linkPath: s.linkPath,
    editState: s.link?.editState ?? null,
    lockHolder: s.link?.lockHolder ?? null,
    remoteProjectId: s.remoteProjectId,
  };
};

/** Closed for CaveB: read-only, pessimistically remote, nothing of CaveA's link left. */
const CLOSED_FOR_B = {
  readOnly: true,
  kind: 'remote',
  linkPath: CAVE_B,
  editState: 'read-only',
  lockHolder: null,
  remoteProjectId: null,
};

let refreshLinkStatusMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.clearAllMocks();
  loadBinaryMock.mockRejectedValue({ code: 'ALREADY_OPEN', message: 'already open' });
  fetchStateMock.mockResolvedValue(stateOf(CAVE_B_PROJECT));
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'get_project_warnings' || command === 'list_recent_projects') return [];
    return undefined;
  });
  // The status call never answers (offline): what the view paints is what stays.
  refreshLinkStatusMock = vi.fn(() => new Promise<void>(() => {}));
  useSpeleoStore.setState({
    link: null,
    linkPath: null,
    projectLinkKind: 'local',
    remoteProjectId: null,
    refreshLinkStatus: refreshLinkStatusMock,
    refreshSession: vi.fn(async () => {}),
  } as never);
  useUIStore.setState({ view: 'single', appMode: 'survey' });
  useProjectStore.setState({
    project: CAVE_A_PROJECT,
    loading: { loadingId: 0, clientOpId: 0, isLoading: false, overallProgress: 0 },
    refreshUndoState: vi.fn(async () => {}),
  } as never);
});

describe('a normal load of CaveB resolves the gate from its answer (the control)', () => {
  it('a remote CaveB after a local CaveA is read-only from the first frame', async () => {
    loadBinaryMock.mockResolvedValue({
      ...stateOf(CAVE_B_PROJECT),
      loadingId: 1,
      isRemote: true,
      warnings: [],
      displaySettings: {},
    });

    await useProjectStore.getState().loadProject(CAVE_B);

    expect(gate()).toEqual(CLOSED_FOR_B);
  });
});

describe('ALREADY_OPEN for CaveB while the store shows CaveA (#576 W71-NF4)', () => {
  it('closes the gate a local CaveA left open', async () => {
    expect(selectSpeleoReadOnly(useSpeleoStore.getState())).toBe(false);

    const outcome = await useProjectStore.getState().loadProject(CAVE_B);

    expect(outcome).toEqual({ success: true });
    expect(useProjectStore.getState().project?.file_path).toBe(CAVE_B);
    expect(gate()).toEqual(CLOSED_FOR_B);
    // The status reply still refines it (a local CaveB reopens the gate then).
    expect(refreshLinkStatusMock).toHaveBeenCalledTimes(1);
  });

  it("drops the editing link of a remote CaveA we held the lock on", async () => {
    useSpeleoStore.setState({
      link: { ...CAVE_A_EDITING },
      linkPath: CAVE_A,
      projectLinkKind: 'remote',
      remoteProjectId: 'speleodb-cave-a',
    });
    expect(selectSpeleoReadOnly(useSpeleoStore.getState())).toBe(false);

    await useProjectStore.getState().loadProject(CAVE_B);

    expect(gate()).toEqual(CLOSED_FOR_B);
    expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);
  });

  it('keeps a link seeded for CaveB itself on the way in, as a load does', async () => {
    // The SpeleoDB open seeds CaveB's link (`setLinkOnOpen`) before it loads CaveB.
    useSpeleoStore.setState({
      link: makeSpeleoLink({ editState: 'editing', lockHolder: 'me@example.com' }),
      linkPath: CAVE_B,
      projectLinkKind: 'remote',
      remoteProjectId: 'speleodb-cave-b',
    });

    await useProjectStore.getState().loadProject(CAVE_B);

    expect(gate()).toEqual({
      readOnly: false,
      kind: 'remote',
      linkPath: CAVE_B,
      editState: 'editing',
      lockHolder: 'me@example.com',
      remoteProjectId: 'speleodb-cave-b',
    });
  });
});

describe('ALREADY_OPEN for the project the store already shows (the guard)', () => {
  it('leaves a resolved gate alone', async () => {
    const outcome = await useProjectStore.getState().loadProject(CAVE_A);

    expect(outcome).toEqual({ success: true });
    expect(fetchStateMock).not.toHaveBeenCalled();
    expect(gate()).toEqual({
      readOnly: false,
      kind: 'local',
      linkPath: null,
      editState: null,
      lockHolder: null,
      remoteProjectId: null,
    });
  });
});
