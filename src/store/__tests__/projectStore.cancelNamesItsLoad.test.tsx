/**
 * [#576 W44-NF3] A Cancel cancels only the load it was made for.
 *
 * `cancel_loading` took no name: the backend cancelled whatever load was active
 * when the call ran. The loading modal's Cancel for load A, still on its way to
 * the backend when A finished on its own and the user opened B, ran after B had
 * started and cancelled B. Now every load, create and file import carries the
 * name the frontend gave it (`loadToken`), each Cancel names its load, and the
 * backend cancels only the load in progress that carries that name.
 *
 * Store tier: the real store and Cancel button; a small fake backend that applies
 * a Cancel when the test says so and matches names as app_state.rs
 * `cancel_loading` does. A Cancel sent with no name at all is the call the store
 * made before this fix, and the fake applies it as the backend of that time did
 * (whatever load is active), so a store that stops naming its loads fails here.
 * All data is synthetic.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import { deferred, timerTurns, type Deferred } from '../../test/factories/async';
import { invokeIpcMock } from '../../test/factories/ipc';
import { makeProject } from '../../test/factories/survey';
import { useProjectStore } from '../projectStore';
import { ImportCancelButton } from '../../components/Dialogs/ImportCancelButton';

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

/** A load the fake backend started: the name it was sent with, answered when delivered. */
interface Load {
  path: string;
  name: unknown;
  reply: Deferred<ReturnType<typeof loadResult>>;
  cancelled: boolean;
  done: boolean;
}

/** A Cancel on its way to the backend: applied, then answered, when the test says so. */
interface HeldCancel {
  args: Record<string, unknown> | undefined;
  reply: Deferred<undefined>;
}

let backendOpen: string | null;
let active: Load | null;
let loads: Load[];
let holdNextCancel: boolean;
let held: HeldCancel[];

/** app_state.rs `cancel_loading`: the load in progress, and only by its name. */
function applyCancel(args: Record<string, unknown> | undefined): void {
  if (!active) return;
  if (args !== undefined && 'loadToken' in args) {
    if (typeof args.loadToken === 'string' && args.loadToken === active.name) active.cancelled = true;
    return;
  }
  // No name at all: the pre-#576 W44-NF3 call, which the backend applied to any load.
  active.cancelled = true;
}

/** The backend finishes the load (`clear_active_load`), and its answer arrives. */
function deliver(load: Load): void {
  load.done = true;
  if (active === load) active = null;
  if (load.cancelled) {
    load.reply.reject({ code: 'CANCELLED', message: 'Loading cancelled' });
    return;
  }
  backendOpen = load.path;
  load.reply.resolve(loadResult(load.path));
}

beforeEach(() => {
  vi.clearAllMocks();
  backendOpen = null;
  active = null;
  loads = [];
  holdNextCancel = false;
  held = [];
  loadBinaryMock.mockImplementation((path: string, loadToken: unknown) => {
    // `start_named_load`: a new load supersedes the one in progress.
    if (active) active.cancelled = true;
    const load: Load = { path, name: loadToken, reply: deferred(), cancelled: false, done: false };
    loads.push(load);
    active = load;
    return load.reply.promise;
  });
  invokeIpcMock.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    if (command === 'cancel_loading') {
      if (!holdNextCancel) return applyCancel(args);
      holdNextCancel = false;
      const cancel: HeldCancel = { args, reply: deferred() };
      held.push(cancel);
      return cancel.reply.promise;
    }
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

describe('a Cancel names the load it was made for (#576 W44-NF3)', () => {
  it("the loading modal's Cancel for load A, run after load B started, leaves B to open", async () => {
    const opA = useProjectStore.getState().loadProject(CAVE_A);
    await timerTurns(5);
    const loadA = loads[0]!;
    expect(loadA.path).toBe(CAVE_A);

    // The user presses the modal's Cancel (GlobalView's `handleCancelLoading` is
    // `cancelLoading`); its IPC is still on its way to the backend...
    holdNextCancel = true;
    const cancelA = useProjectStore.getState().cancelLoading();
    await timerTurns(2);
    expect(held).toHaveLength(1);

    // ...while A finishes on its own, and the user opens B.
    deliver(loadA);
    expect(await opA).toEqual({ success: true });
    const opB = useProjectStore.getState().loadProject(CAVE_B);
    await timerTurns(5);
    const loadB = loads[1]!;
    expect(loadB.path).toBe(CAVE_B);

    // A's Cancel reaches the backend now, with B the load in progress.
    applyCancel(held[0]!.args);
    held[0]!.reply.resolve(undefined);
    await cancelA;
    deliver(loadB);
    const outcomeB = await opB;

    const observed = {
      cancelNamed: held[0]!.args?.loadToken,
      loadBCancelled: loadB.cancelled,
      outcomeB,
      backendOpen,
      shown: useProjectStore.getState().project?.file_path ?? null,
    };
    console.log(
      `[W44-NF3] A named ${String(loadA.name)}, B named ${String(loadB.name)}; observed:`,
      JSON.stringify(observed),
    );
    expect(typeof loadA.name).toBe('string');
    expect(loadB.name).not.toBe(loadA.name);
    expect(observed).toEqual({
      cancelNamed: loadA.name,
      loadBCancelled: false,
      outcomeB: { success: true },
      backendOpen: CAVE_B,
      shown: CAVE_B,
    });
  });

  it('the cancel a superseded load sends names that load', async () => {
    const opA = useProjectStore.getState().loadProject(CAVE_A);
    await timerTurns(5);
    const loadA = loads[0]!;
    // B is requested while A runs: A is told (`onSuperseded`) and B waits for it.
    const opB = useProjectStore.getState().loadProject(CAVE_B);
    await timerTurns(5);
    const cancels = invokeIpcMock.mock.calls.filter(([command]) => command === 'cancel_loading');
    const superseded = cancels[cancels.length - 1]!;
    expect(superseded[1]).toEqual({ loadToken: loadA.name });
    expect(loadA.cancelled).toBe(true);

    deliver(loadA);
    await opA;
    await timerTurns(5);
    deliver(loads[1]!);
    expect(await opB).toEqual({ success: true });
    expect(loads[1]!.name).not.toBe(loadA.name);
  });

  it("a file import's Cancel names the import its dialog shows", async () => {
    const importAnswer = deferred<unknown>();
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'execute_tml_import') return importAnswer.promise;
      return undefined;
    });
    // The dialog starts its progress line, then the import (TmlImportDialog's handleImport).
    useProjectStore.getState().startLoading('CaveT', '/files/CaveT.tml');
    const run = useProjectStore.getState().executeTmlImport('/files/CaveT.tml', null);
    await timerTurns(3);
    const sent = invokeIpcMock.mock.calls.find(([command]) => command === 'execute_tml_import');
    const importName = (sent?.[1] as Record<string, unknown> | undefined)?.loadToken;
    expect(typeof importName).toBe('string');

    render(<ImportCancelButton data-testid="import-cancel">Cancel</ImportCancelButton>);
    await userEvent.click(screen.getByTestId('import-cancel'));
    const cancel = invokeIpcMock.mock.calls.find(([command]) => command === 'cancel_loading');
    expect(cancel?.[1]).toEqual({ loadToken: importName });

    // The import stops before anything is applied (import_runner.rs `cancelled`).
    importAnswer.reject({ code: 'IMPORT_CANCELLED', message: 'the TML import was cancelled; nothing was imported' });
    expect((await run).success).toBe(false);
  });
});
