/**
 * [#576 FZ-FZ-INT-1] Project switches run one at a time, latest wins.
 *
 * A load, create or close used to send its IPC at once, while the previous switch's
 * IPC was still in flight. The backend could apply the two in either order, and a
 * newer load cancelled the older one's ANSWER, not its backend effect: load A, load
 * B, load A again could end with the store on A (or nothing) and the backend on B,
 * so edits went to B while A was shown. Now a switch waits for the previous
 * switch's answer before it sends its own; a switch still waiting when a newer one
 * is requested is dropped (a load resolves SUPERSEDED, a create 'Cancelled', a close
 * the second-close refusal), and the one in flight is cancelled at once.
 *
 * Store tier: the real store; a small fake backend applies each load when the test
 * executes it and holds the answer until the test delivers it, so the test chooses
 * the order. All data is synthetic.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { deferred, timerTurns, type Deferred } from '../../test/factories/async';
import { invokeIpcMock } from '../../test/factories/ipc';
import { makeProject } from '../../test/factories/survey';
import { useProjectStore } from '../projectStore';
import { switchUnderWay } from '../switchGate';

const loadBinaryMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc');
vi.mock('../../utils/loadProjectBinary', () => ({ loadProjectFullBinary: loadBinaryMock }));

const CAVE_A = '/caves/CaveA.stalx';
const CAVE_B = '/caves/CaveB.stalx';
const CREATED: Record<string, string> = { [CAVE_A]: '2026-05-01T10:00:00Z', [CAVE_B]: '2026-05-02T10:00:00Z' };

function loadResult(path: string) {
  return {
    loadingId: 1,
    isRemote: false,
    newerSchemaVersion: null,
    speleoLinkUnreadable: false,
    unreadableProjectSettings: [],
    loadRepairs: null,
    project: makeProject({ created_at: CREATED[path]!, file_path: path }),
    stations: [],
    surveyData: { shots: [], stations: [] },
    stats: null,
    warnings: [],
    frozen_shots: [],
    displaySettings: {},
  };
}

/** A load the fake backend received: applied when executed, answered when delivered. */
interface Load {
  path: string;
  reply: Deferred<ReturnType<typeof loadResult>>;
  cancelled: boolean;
  outcome: null | { ok: true } | { ok: false; error: { code: string; message: string } };
}

/** What the fake backend has open. */
let backendOpen: string | null;
let loads: Load[];
let sent: string[];

/** The backend runs the load: the project-slot commit, or the refusal (the answer is held). */
function execute(load: Load): void {
  if (load.outcome) return;
  if (load.cancelled) load.outcome = { ok: false, error: { code: 'CANCELLED', message: 'Loading cancelled' } };
  else if (backendOpen === load.path) load.outcome = { ok: false, error: { code: 'ALREADY_OPEN', message: 'already open' } };
  else {
    backendOpen = load.path;
    load.outcome = { ok: true };
  }
}

function deliver(load: Load): void {
  execute(load);
  const outcome = load.outcome!;
  if (outcome.ok) load.reply.resolve(loadResult(load.path));
  else load.reply.reject(outcome.error);
}

beforeEach(() => {
  vi.clearAllMocks();
  backendOpen = null;
  loads = [];
  sent = [];
  loadBinaryMock.mockImplementation((path: string) => {
    const load: Load = { path, reply: deferred(), cancelled: false, outcome: null };
    loads.push(load);
    sent.push(`load ${path}`);
    return load.reply.promise;
  });
  invokeIpcMock.mockImplementation(async (command: string) => {
    sent.push(command);
    // The cancel token stops a load the backend has not committed yet.
    if (command === 'cancel_loading') for (const load of loads) if (!load.outcome) load.cancelled = true;
    if (command === 'close_project') {
      backendOpen = null;
      return { remote_link: null };
    }
    if (command === 'create_project_full') return { loadingId: 2, project: makeProject(), displaySettings: {} };
    if (command === 'list_recent_projects' || command === 'take_unrecognised_shot_kinds') return [];
    return undefined;
  });
  useProjectStore.setState({
    project: null,
    isClosing: false,
    loading: { loadingId: 0, clientOpId: 0, isLoading: false, overallProgress: 0 },
    refreshUndoState: vi.fn(async () => {}),
  } as never);
});

describe('project switches run one at a time, latest wins (#576 FZ-FZ-INT-1)', () => {
  it('a load waits for the previous switch to answer before it sends its own', async () => {
    const first = useProjectStore.getState().loadProject(CAVE_A);
    await timerTurns(5);
    const second = useProjectStore.getState().loadProject(CAVE_B);
    await timerTurns(5);
    expect(sent.filter((s) => s.startsWith('load '))).toEqual([`load ${CAVE_A}`]);

    deliver(loads[0]!);
    await first;
    await timerTurns(5);
    expect(sent.filter((s) => s.startsWith('load '))).toEqual([`load ${CAVE_A}`, `load ${CAVE_B}`]);
    deliver(loads[1]!);
    expect(await second).toEqual({ success: true });
  });

  it('load A, load B, load A: the store and the backend end on the same project', async () => {
    const opA = useProjectStore.getState().loadProject(CAVE_A);
    await timerTurns(5);
    // The backend commits A; its answer is still on the way.
    execute(loads[0]!);
    const opB = useProjectStore.getState().loadProject(CAVE_B);
    const opA2 = useProjectStore.getState().loadProject(CAVE_A);
    await timerTurns(5);

    // The adversarial order: run the newest call first, deliver the oldest answer first.
    for (let round = 0; round < 20; round += 1) {
      for (const load of [...loads].reverse()) execute(load);
      const next = loads.find((load) => load.outcome && !('delivered' in load));
      if (next) {
        Object.assign(next, { delivered: true });
        deliver(next);
      }
      await timerTurns(5);
    }
    await Promise.all([opA, opB, opA2]);

    const trace = `sent: ${sent.join(', ')}; backend: ${backendOpen}; store: ${useProjectStore.getState().project?.file_path}`;
    expect(await opB, trace).toEqual({ success: false, superseded: true });
    expect(useProjectStore.getState().project?.file_path, trace).toBe(backendOpen);
    expect(backendOpen, trace).toBe(CAVE_A);
  });

  it('a create or a close still waiting when a newer switch is requested is dropped', async () => {
    const load = useProjectStore.getState().loadProject(CAVE_A);
    await timerTurns(5);
    const create = useProjectStore.getState().createProject('New cave');
    const close = useProjectStore.getState().closeProject();
    const newest = useProjectStore.getState().loadProject(CAVE_B);

    expect(await create).toMatchObject({ success: false, error: 'Cancelled' });
    expect(await close).toMatchObject({ success: false, error: 'Already closing' });
    expect(useProjectStore.getState().isClosing).toBe(false);

    deliver(loads[0]!);
    await load;
    await timerTurns(5);
    deliver(loads[1]!);
    expect(await newest).toEqual({ success: true });
    expect(sent).not.toContain('create_project_full');
    expect(sent).not.toContain('close_project');
  });

  it('a create a close supersedes, which then fails, takes its loading state down (#576 RVF-c-A1-1)', async () => {
    // A close never touches the loading state, so the superseded create must end its own.
    useProjectStore.setState({
      project: makeProject({ created_at: '2026-05-01T10:00:00Z', file_path: '/caves/Open.stalx' }),
    } as never);
    const createReply = deferred<never>();
    const backend = invokeIpcMock.getMockImplementation()!;
    invokeIpcMock.mockImplementation(async (command: string, ...rest: unknown[]) => {
      if (command !== 'create_project_full') return backend(command, ...rest);
      sent.push(command);
      return createReply.promise;
    });

    const create = useProjectStore.getState().createProject('New cave');
    await timerTurns(5);
    expect(sent).toContain('create_project_full');
    expect(useProjectStore.getState().loading.isLoading).toBe(true);
    const close = useProjectStore.getState().closeProject();
    await timerTurns(5);
    createReply.reject({ code: 'IO', message: 'disk full' });

    expect(await create).toMatchObject({ success: false, error: 'disk full' });
    expect(await close).toMatchObject({ success: true });
    await timerTurns(5);
    const { loading } = useProjectStore.getState();
    expect(loading.isLoading, `loading after the failed create: ${JSON.stringify(loading)}`).toBe(false);
    expect(loading.error).toBeUndefined();
    expect(switchUnderWay()).toBe(false);
    expect(sent).toEqual(['cancel_loading', 'create_project_full', 'close_project']);
  });
});
