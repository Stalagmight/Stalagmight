/**
 * #270 - the `ALREADY_OPEN` short circuit renders a project, so it owes the same
 * duty every other rendered-project path pays: RESOLVE the SpeleoDB edit gate.
 *
 * The success path does that synchronously (`syncProjectLink(result.isRemote, path)`),
 * and so does `createProject`. This branch did neither - it flipped the view, reset
 * loading, refreshed undo, and reported success with whatever link the caller had
 * seeded on the way in.
 *
 * That is reachable, not latent: "Open from SpeleoDB" sits in the IN-PROJECT
 * hamburger whenever signed in, so re-opening the currently-open project is an
 * ordinary mis-step. The backend refuses it over the unpushed local edits, the user
 * picks "Keep the local copy", and the old code then overwrote the live link with a
 * pessimistic read-only seed which this branch preserved verbatim - a read-only
 * survey UI for a project we hold the lock on, plus a disarmed quit guard, healing
 * only on a subsequent status refresh.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { useSpeleoStore, type SpeleoLinkState } from '../speleoStore';

const invokeIpcMock = vi.hoisted(() => vi.fn());
const loadBinaryMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({ invokeIpc: invokeIpcMock }));
vi.mock('../../utils/loadProjectBinary', () => ({ loadProjectFullBinary: loadBinaryMock }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));

/** An editing link for a project we hold the lock on, with work not yet pushed. */
const EDITING_LINK: SpeleoLinkState = {
  permission: 'READ_AND_WRITE',
  editState: 'editing',
  lockHolder: 'me@example.com',
  lockedByOther: false,
  hasUnpushedEdits: true,
  unwound: false,

  undoMode: 'full',
  remoteState: 'up_to_date',
  noAccess: false,
  serverError: null,
};

let refreshLinkStatusMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.clearAllMocks();
  // The backend's "this project is already open" refusal, in the shape
  // `parseLoadError` reads (`code`).
  loadBinaryMock.mockRejectedValue({ code: 'ALREADY_OPEN', message: 'project already open' });
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'list_recent_projects') return [];
    return undefined;
  });

  refreshLinkStatusMock = vi.fn(async () => {});
  useSpeleoStore.setState({
    link: { ...EDITING_LINK },
    linkPath: '/cache/p1.stalx',
    projectLinkKind: 'remote',
    refreshLinkStatus: refreshLinkStatusMock,
  });
  useUIStore.setState({ appMode: 'survey' });
  useProjectStore.setState({
    project: null,
    isDirty: false,
    loading: { loadingId: 0n, clientOpId: 0, isLoading: false, overallProgress: 0 },
    refreshUndoState: vi.fn(async () => {}),
  });
});

describe('loadProject ALREADY_OPEN re-resolves the SpeleoDB gate (#270)', () => {
  it('reports success, shows the project, and asks the backend to re-resolve the link', async () => {
    const result = await useProjectStore.getState().loadProject('/cache/p1.stalx');

    expect(result.success).toBe(true);
    expect(useUIStore.getState().view).toBe('single');
    // The whole point: this branch no longer renders a project with an unexamined
    // edit gate. Unawaited (the status call does blocking HTTP), so only the call
    // itself is pinned here.
    expect(refreshLinkStatusMock).toHaveBeenCalled();
  });

  /**
   * The branch had no staleness guard, while BOTH its neighbours (the success
   * path and the generic error path) check the same `clientOpId`. So a load that
   * had already lost its slot still flipped the view and called `resetLoading()`,
   * tearing down the loading modal of the NEWER load still running - and then
   * reported success, which `keepLocalCopy` / `finishOpen` now act on.
   */
  it('reports SUPERSEDED, and touches nothing, when a newer load took over', async () => {
    // The newer load: bump the id while this one is between its IPC and the
    // branch. `startLoading` is what the real one calls; here the reject fires
    // first, so move the id from inside the failing IPC.
    loadBinaryMock.mockImplementation(async () => {
      useProjectStore.setState((s) => ({ loading: { ...s.loading, clientOpId: s.loading.clientOpId + 99 } }));
      throw { code: 'ALREADY_OPEN', message: 'project already open' };
    });
    useUIStore.setState({ view: 'global' });

    const result = await useProjectStore.getState().loadProject('/cache/p1.stalx');

    expect(result).toEqual({ success: false, superseded: true });
    // The newer load owns the view and the loading modal now.
    expect(useUIStore.getState().view).toBe('global');
    expect(refreshLinkStatusMock).not.toHaveBeenCalled();
  });

  /**
   * Cheap hardening, not a proven production route: this branch resolved the gate
   * ONLY through the unawaited, error-swallowing `refreshLinkStatus`, unlike every
   * other rendered-project path. Left at `unknown` - read-only under the
   * fail-closed gate - a failed status call renders the project permanently
   * read-only with no status button to act from (it requires
   * `link !== null`). Seeding pessimistically gives the user a way out on screen;
   * the status reply then resolves it either way.
   */
  it('fails CLOSED, with a status button to recover through, when nothing resolved the gate', async () => {
    useSpeleoStore.setState({ link: null, linkPath: null, projectLinkKind: 'unknown' });
    refreshLinkStatusMock.mockImplementation(() => new Promise<void>(() => {}));

    await useProjectStore.getState().loadProject('/cache/p1.stalx');

    expect(useSpeleoStore.getState().projectLinkKind).toBe('remote');
    expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    // The SpeleoDB workspace requires `link !== null` to offer manual recovery.
    expect(useSpeleoStore.getState().link).not.toBeNull();
  });

  it('does not silently downgrade a lock we hold while the status call is pending', async () => {
    // The refinement must not itself be a regression: nothing in this branch may
    // touch the live link, so the survey UI stays editable and the quit guard stays
    // armed even if the status call never answers (offline).
    refreshLinkStatusMock.mockImplementation(() => new Promise<void>(() => {}));

    await useProjectStore.getState().loadProject('/cache/p1.stalx');

    const link = useSpeleoStore.getState().link;
    expect(link?.editState).toBe('editing');
    expect(link?.hasUnpushedEdits).toBe(true);
  });
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => { resolve = yes; });
  return { promise, resolve };
}

it('deduplicates current paths but selects the last A in A-B-A', async () => {
  const probe = deferred<null>();
  let probes = 0;
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'speleodb_project_access_at' && ++probes === 1) return probe.promise;
    if (command === 'list_recent_projects') return [];
    return undefined;
  });
  const store = useProjectStore.getState();
  const first = store.loadProject('/A.stalx');
  expect(store.loadProject('/A.stalx')).toBe(first);
  await vi.waitFor(() => expect(probes).toBe(1));
  const second = store.loadProject('/B.stalx');
  const final = store.loadProject('/A.stalx');
  expect(final).not.toBe(first);
  expect(store.loadProject('/A.stalx')).toBe(final);
  expect(await final).toEqual({ success: true });
  probe.resolve(null);
  expect(await first).toEqual({ success: false, superseded: true });
  expect(await second).toEqual({ success: false, superseded: true });
  expect(loadBinaryMock).toHaveBeenCalledExactlyOnceWith('/A.stalx');
});

it('does not allocate an operation ID or reset B after A cancellation completes late', async () => {
  const cancellation = deferred<void>();
  const entered = deferred<void>();
  let cancellations = 0;
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'cancel_loading' && ++cancellations === 1) {
      entered.resolve();
      await cancellation.promise;
    }
    if (command === 'list_recent_projects') return [];
    return undefined;
  });
  const store = useProjectStore.getState();
  const a = store.loadProject('/A.stalx', { accessConfirmed: true });
  await entered.promise;
  await store.loadProject('/B.stalx', { accessConfirmed: true });
  const loading = useProjectStore.getState().loading;
  cancellation.resolve();
  expect(await a).toEqual({ success: false, superseded: true });
  expect(useProjectStore.getState().loading).toBe(loading);
  expect(loadBinaryMock).toHaveBeenCalledExactlyOnceWith('/B.stalx');
});

it.each(['close', 'create'])('a newer %s invalidates an open waiting for access', async (kind) => {
  const probe = deferred<null>();
  const entered = deferred<void>();
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'speleodb_project_access_at') { entered.resolve(); return probe.promise; }
    if (command === 'close_project') return { success: true };
    if (command === 'create_project_full') throw new Error('synthetic create failure');
    return undefined;
  });
  const store = useProjectStore.getState();
  const open = store.loadProject('/A.stalx');
  await entered.promise;
  if (kind === 'close') await store.closeProject();
  else await store.createProject('Synthetic');
  probe.resolve(null);
  expect(await open).toEqual({ success: false, superseded: true });
  expect(loadBinaryMock).not.toHaveBeenCalled();
});

it('reports superseded if a newer request arrives during already-open reconciliation', async () => {
  const undo = deferred<void>();
  const entered = deferred<void>();
  useProjectStore.setState({ refreshUndoState: vi.fn().mockImplementationOnce(async () => {
    entered.resolve();
    await undo.promise;
  }).mockResolvedValue(undefined) });
  const store = useProjectStore.getState();
  const a = store.loadProject('/A.stalx');
  await entered.promise;
  expect(await store.loadProject('/B.stalx')).toEqual({ success: true });
  undo.resolve();
  expect(await a).toEqual({ success: false, superseded: true });
  expect(refreshLinkStatusMock).toHaveBeenCalledOnce();
});
