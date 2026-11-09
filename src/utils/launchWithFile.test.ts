// @vitest-environment jsdom
/**
 * Launch-with-file (#424): the OS -> `routeOpenedFile` path.
 *
 * The cases that regress silently, and what makes each red:
 *
 * - Routing before the app is ready would run the router against a store that
 *   has not read its config.
 * - `routeOpenedFile` resolves at dialog HAND-OFF, not flow completion, so a
 *   plain `await` loop lets path 2 replace path 1's dialog in the single slot -
 *   path 1 is lost with no error anywhere. The mock below marks a dialog active
 *   on its first call, which is exactly what the real router does.
 * - Two nudges arriving together must not interleave two drains through that
 *   same single slot.
 *
 * The "must not swap a clean project out from under the user without a click"
 * guard itself now lives in `routeOpenedFile` (openOrImport.test.ts covers it,
 * for every `.stalx`-open entry point, not just this one) - this file only
 * pins that the launch channel tags its source and otherwise gets out of the
 * way, `routeOpenedFile` being mocked below.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, waitFor, act } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('./ipc', () => ({ invokeIpc: invokeIpcMock }));

const routeOpenedFileMock = vi.hoisted(() =>
  vi.fn(async (_path: string, _title?: string) => {}),
);
vi.mock('./openOrImport', () => ({ routeOpenedFile: routeOpenedFileMock }));

const setProjectActionSourceMock = vi.hoisted(() => vi.fn());
// uiStore imports the tracking side of this module; stub the whole surface so
// `openDialog` works without pulling telemetry's IPC in.
vi.mock('./telemetry', () => ({
  setProjectActionSource: setProjectActionSourceMock,
  clearProjectActionSource: vi.fn(),
  trackAction: vi.fn(),
  trackDialogComplete: vi.fn(),
  checkDialogCancel: vi.fn(),
  trackDisplayState: vi.fn(),
  setDwellState: vi.fn(),
  advanceProjectSeq: vi.fn(),
  trackNav: vi.fn(),
  trackPanelToggle: vi.fn(),
  trackPanelMode: vi.fn(),
  trackSelect: vi.fn(),
  trackSelectAdd: vi.fn(),
  isPersistSuppressed: () => false,
}));

const listenMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/event', () => ({ listen: listenMock }));

import {
  useLaunchWithFile,
  drainLaunchFiles,
  resetLaunchFileQueueForTest,
  LAUNCH_FILE_EVENT,
} from './launchWithFile';
import { useUIStore } from '../store/uiStore';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore } from '../store/speleoStore';

/** Captured backend-event handler, so a test can fire the nudge itself. */
let emitToApp: (() => void) | null = null;
const unlisten = vi.fn();

/** Queue the backend will hand over on the next `take_pending_launch_files`. */
let queued: string[] = [];

/** Routed paths, in the order the module actually committed to them. */
const routed = () => routeOpenedFileMock.mock.calls.map((c) => c[0]);

beforeEach(() => {
  vi.clearAllMocks();
  // `clearAllMocks` keeps implementations; a per-case router stub must not leak
  // into the next case (that is what made the confirm cases pass spuriously).
  routeOpenedFileMock.mockReset();
  routeOpenedFileMock.mockImplementation(async (_path: string, _title?: string) => {});
  resetLaunchFileQueueForTest();
  emitToApp = null;
  queued = [];
  useUIStore.setState({ activeDialog: null, dialogData: {}, placementMode: null, screenOccupants: 0 });
  useProjectStore.setState({ project: null, isDirty: false });
  useSpeleoStore.setState({ link: null });
  listenMock.mockImplementation(async (_event: string, handler: () => void) => {
    emitToApp = handler;
    return unlisten;
  });
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd !== 'take_pending_launch_files') throw new Error(`unexpected command ${cmd}`);
    const batch = queued;
    queued = [];
    return batch;
  });
});

afterEach(() => {
  useUIStore.setState({ activeDialog: null, dialogData: {}, placementMode: null, screenOccupants: 0 });
});

describe('drainLaunchFiles', () => {
  it('routes a single queued path through the unified router', async () => {
    queued = ['/dives/one.stalx'];

    await drainLaunchFiles();

    expect(routed()).toEqual(['/dives/one.stalx']);
    expect(setProjectActionSourceMock).toHaveBeenCalledWith('os-file-association');
  });

  it('does not route when the queue is empty', async () => {
    await drainLaunchFiles();
    expect(routeOpenedFileMock).not.toHaveBeenCalled();
  });

  it('routes the whole batch when no flow claims the dialog slot', async () => {
    queued = ['/dives/one.stalx', '/dives/two.tml'];

    await drainLaunchFiles();

    expect(routed()).toEqual(['/dives/one.stalx', '/dives/two.tml']);
  });

  /**
   * The blocker: the router resolves at HAND-OFF. Path 2 must wait for the
   * dialog path 1 opened, or it silently replaces it in the single slot.
   */
  it('routes one path per completed flow, holding the rest', async () => {
    routeOpenedFileMock.mockImplementation(async () => {
      useUIStore.setState({ activeDialog: 'unsaved-changes' });
    });
    queued = ['/dives/one.stalx', '/dives/two.tml', '/dives/three.tmlu'];

    await drainLaunchFiles();

    expect(routed()).toEqual(['/dives/one.stalx']);

    // A drain while the dialog is still up changes nothing.
    await drainLaunchFiles();
    expect(routed()).toEqual(['/dives/one.stalx']);
  });

  it('resumes the held paths once the dialog slot clears', async () => {
    routeOpenedFileMock.mockImplementationOnce(async () => {
      useUIStore.setState({ activeDialog: 'unsaved-changes' });
    });
    queued = ['/dives/one.stalx', '/dives/two.tml'];

    await drainLaunchFiles();
    expect(routed()).toEqual(['/dives/one.stalx']);

    useUIStore.setState({ activeDialog: null });
    await drainLaunchFiles();

    expect(routed()).toEqual(['/dives/one.stalx', '/dives/two.tml']);
  });

  it('holds everything when a dialog is already open before the file arrives', async () => {
    useUIStore.setState({ activeDialog: 'settings' });
    queued = ['/dives/one.stalx'];

    await drainLaunchFiles();

    expect(routeOpenedFileMock).not.toHaveBeenCalled();

    useUIStore.setState({ activeDialog: null });
    await drainLaunchFiles();
    expect(routed()).toEqual(['/dives/one.stalx']);
  });

  it('holds while a placement flow owns the screen', async () => {
    useUIStore.setState({
      placementMode: { active: true } as unknown as ReturnType<
        typeof useUIStore.getState
      >['placementMode'],
    });
    queued = ['/dives/one.stalx'];

    await drainLaunchFiles();

    expect(routeOpenedFileMock).not.toHaveBeenCalled();
  });

  // #445 review: `screenBusy()` used to read only `activeDialog`/`placementMode`,
  // blind to the independent top-layer overlays (TelemetryOptInDialog,
  // HeldLockResumeDialog, SpeleoDumpPickerDialog) that
  // render OUTSIDE that slot - exactly the startup/login surfaces live at the
  // moment a cold-launch drain runs. A cold launch with a held SpeleoDB lock plus
  // a `.stalx` argv used to raise a dialog over the held-lock prompt, which says
  // (from the inside) that it refuses exactly that. `screenOccupants` (uiStore,
  // fed by `useScreenOccupant`) is the single source those overlays register
  // into, so this covers all of them by construction.
  it('holds while an independent overlay (held-lock resume, telemetry opt-in, ...) owns the screen', async () => {
    useUIStore.setState({ screenOccupants: 1 });
    queued = ['/dives/one.stalx'];

    await drainLaunchFiles();

    expect(routeOpenedFileMock).not.toHaveBeenCalled();

    useUIStore.setState({ screenOccupants: 0 });
    await drainLaunchFiles();
    expect(routed()).toEqual(['/dives/one.stalx']);
  });

  /**
   * #445 review: a repeated drain while busy used to fetch (and concat onto
   * `deferred`) regardless, growing local memory without bound across a flood
   * of forwards and draining the backend's own `MAX_LAUNCH_PATHS` cap out from
   * under it. Busy is now checked BEFORE fetching, so the backend queue is
   * left untouched - `invokeIpc` is never even called - until the screen
   * actually clears.
   */
  it('does not fetch the backend queue while busy', async () => {
    useUIStore.setState({ activeDialog: 'settings' });
    queued = ['/dives/one.stalx'];

    await drainLaunchFiles();
    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(routeOpenedFileMock).not.toHaveBeenCalled();

    // Repeated drains while still busy: still no fetch.
    await drainLaunchFiles();
    await drainLaunchFiles();
    expect(invokeIpcMock).not.toHaveBeenCalled();

    useUIStore.setState({ activeDialog: null });
    await drainLaunchFiles();
    expect(routed()).toEqual(['/dives/one.stalx']);
  });

  /**
   * Two nudges landing together must not run two loops through one dialog slot.
   * Without the chain both drains fetch, both see an idle screen, and both
   * route - the second clobbering the first's dialog.
   */
  it('serializes concurrent drains', async () => {
    let inFlight = 0;
    let maxConcurrent = 0;
    routeOpenedFileMock.mockImplementation(async () => {
      inFlight += 1;
      maxConcurrent = Math.max(maxConcurrent, inFlight);
      await Promise.resolve();
      inFlight -= 1;
      useUIStore.setState({ activeDialog: 'unsaved-changes' });
    });
    queued = ['/dives/one.stalx', '/dives/two.tml'];

    await Promise.all([drainLaunchFiles(), drainLaunchFiles(), drainLaunchFiles()]);

    expect(maxConcurrent).toBe(1);
    expect(routed()).toEqual(['/dives/one.stalx']);
  });

  it('a failing drain does not poison later drains', async () => {
    invokeIpcMock.mockRejectedValueOnce(new Error('ipc down'));

    await expect(drainLaunchFiles()).rejects.toThrow('ipc down');

    queued = ['/dives/after.stalx'];
    await drainLaunchFiles();
    expect(routed()).toEqual(['/dives/after.stalx']);
  });
});

// The "swap out a clean project?" confirm itself is `routeOpenedFile`'s job
// now (openOrImport.test.ts), for every `.stalx`-open entry point at once -
// `routeOpenedFile` is mocked in this file, so it can't be observed asking
// here. What this file still owns: the launch channel must delegate to
// `routeOpenedFile` unconditionally, tagged as its own source, whether or not
// a project is open - it no longer pre-empts that call with a local check.
describe('routeLaunchedFile delegates unconditionally', () => {
  it('routes a .stalx over an already-open project through routeOpenedFile, source-tagged', async () => {
    useProjectStore.setState({ project: { name: 'Open' } as never, isDirty: false });
    queued = ['/dives/one.stalx'];

    await drainLaunchFiles();

    expect(routed()).toEqual(['/dives/one.stalx']);
    expect(setProjectActionSourceMock).toHaveBeenCalledWith('os-file-association');
  });

  it('routes a .stalx with no project open through routeOpenedFile the same way', async () => {
    queued = ['/dives/one.stalx'];

    await drainLaunchFiles();

    expect(routed()).toEqual(['/dives/one.stalx']);
    expect(setProjectActionSourceMock).toHaveBeenCalledWith('os-file-association');
  });
});

describe('useLaunchWithFile', () => {
  it('drains the cold-launch queue as soon as the app is ready', async () => {
    queued = ['/dives/cold.stalx'];

    renderHook(() => useLaunchWithFile(true));

    await waitFor(() => expect(routed()).toEqual(['/dives/cold.stalx']));
  });

  it('subscribes to the backend nudge', async () => {
    renderHook(() => useLaunchWithFile(false));
    await waitFor(() =>
      expect(listenMock).toHaveBeenCalledWith(LAUNCH_FILE_EVENT, expect.any(Function)),
    );
  });

  it('holds a path queued until the app is ready, then routes it', async () => {
    queued = ['/dives/early.stalx'];

    const { rerender } = renderHook(({ ready }) => useLaunchWithFile(ready), {
      initialProps: { ready: false },
    });
    await waitFor(() => expect(emitToApp).not.toBeNull());

    // The OS hands us a file while the app is still starting.
    emitToApp?.();
    await Promise.resolve();
    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(routeOpenedFileMock).not.toHaveBeenCalled();

    rerender({ ready: true });

    await waitFor(() => expect(routed()).toEqual(['/dives/early.stalx']));
  });

  it('routes a nudge that arrives while the app is already running', async () => {
    renderHook(() => useLaunchWithFile(true));
    await waitFor(() => expect(emitToApp).not.toBeNull());
    routeOpenedFileMock.mockClear();

    queued = ['/dives/second-instance.stalx'];
    emitToApp?.();

    await waitFor(() => expect(routed()).toEqual(['/dives/second-instance.stalx']));
  });

  /** Nothing else asks again once a held path's blocking flow ends. */
  it('re-drains by itself when the dialog slot clears', async () => {
    routeOpenedFileMock.mockImplementationOnce(async () => {
      useUIStore.setState({ activeDialog: 'unsaved-changes' });
    });
    queued = ['/dives/one.stalx', '/dives/two.tml'];

    renderHook(() => useLaunchWithFile(true));
    await waitFor(() => expect(routed()).toEqual(['/dives/one.stalx']));

    act(() => {
      useUIStore.setState({ activeDialog: null });
    });

    await waitFor(() => expect(routed()).toEqual(['/dives/one.stalx', '/dives/two.tml']));
  });

  /**
   * #445 review: a nudge arriving while the screen is busy is never fetched at
   * all (`runDrain` checks `screenBusy()` before touching the backend), so
   * `deferred` cannot grow while a dialog stays open through a flood of
   * repeated second-instance launches. But the file that nudge represents must
   * not be lost either - `deferred` stays EMPTY the whole time here; only
   * `pendingBackendCheck` remembers there is more to look at once the screen
   * clears, so this still surfaces it rather than stranding it in the backend
   * queue with no local trace.
   */
  it('does not lose a nudge that arrived while busy - deferred stays empty until the screen clears', async () => {
    useUIStore.setState({ activeDialog: 'settings' });
    renderHook(() => useLaunchWithFile(true));
    await waitFor(() => expect(emitToApp).not.toBeNull());

    queued = ['/dives/busy-arrival.stalx'];
    emitToApp?.();
    await Promise.resolve();

    // Busy: the backend queue was never even fetched.
    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(routeOpenedFileMock).not.toHaveBeenCalled();

    act(() => {
      useUIStore.setState({ activeDialog: null });
    });

    await waitFor(() => expect(routed()).toEqual(['/dives/busy-arrival.stalx']));
  });

  it('unsubscribes on unmount', async () => {
    const { unmount } = renderHook(() => useLaunchWithFile(true));
    await waitFor(() => expect(emitToApp).not.toBeNull());

    unmount();

    expect(unlisten).toHaveBeenCalled();
  });
});
