// @vitest-environment jsdom
/**
 * #576 W62-Q1-D1: a file the OS launches while an import's file is still parsing
 * waits in the launch queue and opens once that import is done - when the parse
 * settles with nothing to show, when the user closes the dialog its result
 * opened (an error, or the import's first dialog cancelled), or when the user
 * cancelled the import while it parsed.
 *
 * The parse has nothing on screen (no project open, or the import-as-new confirm
 * cancelled under it), so the launch queue used to read the screen as idle and
 * route the file at once - into the router's one-import-at-a-time gate, which
 * refused it with the in-progress toast and dropped it. A file the user picks or
 * drops during the parse still gets that toast (decided behaviour, pinned in
 * openOrImport.parseInFlight.test.ts); only the launch channel has a queue to
 * hold it in.
 *
 * Real `launchWithFile.ts` + `openOrImport.ts` + `uiStore.ts`. Mocked: the
 * backend IPC (the launch queue answers `take_pending_launch_files`), the Tauri
 * event listener (the test fires the nudge), and the parsers, each held on a
 * deferred promise so the test decides when it lands.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn().mockResolvedValue(null) }));

vi.mock('../ipc');

const listenMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/event', () => ({ listen: listenMock }));

import { useLaunchWithFile, resetLaunchFileQueueForTest } from '../launchWithFile';
import { routeOpenedFile } from '../openOrImport';
import { TOAST_KEYS } from '../toastKeys';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { invokeIpcMock } from '../../test/factories/ipc';
import { deferred, timerTurns, type Deferred } from '../../test/factories/async';

const PICKED_DMP = '/cave/CaveA.dmp';
const PICKED_TML = '/cave/CaveA.tml';
const LAUNCHED = '/dives/Launched.tml';

/** The backend's launch queue, handed over (and emptied) on the next fetch. */
let queued: string[] = [];
/** The backend nudge, captured from the hook's listener. */
let nudge: (() => void) | null = null;

/** One held parse per parser call, by path. */
let held: Map<string, Deferred<unknown>>;
const holdParse = (path: string): Promise<unknown> => {
  const d = deferred<unknown>();
  held.set(path, d);
  return d.promise;
};
const parsers = {
  importDmp: vi.fn(holdParse),
  parseTml: vi.fn(holdParse),
};

const realShowToast = useUIStore.getState().showToast;
const showToast = vi.fn();

const parsed = (path: string): Deferred<unknown> => {
  const d = held.get(path);
  if (!d) throw new Error(`no parse of ${path} was started`);
  return d;
};

const inProgressToasts = (): number =>
  showToast.mock.calls.filter((call) => call[1] === TOAST_KEYS.IMPORT_IN_PROGRESS).length;

/** The OS hands the running app a file: queue it, then nudge. */
function launch(path: string): void {
  queued.push(path);
  if (!nudge) throw new Error('the launch listener is not registered');
  nudge();
}

/** Mount the launch channel, ready, and wait until its listener is live. */
async function mountLaunchChannel(): Promise<void> {
  renderHook(() => useLaunchWithFile(true));
  await waitFor(() => expect(nudge).not.toBeNull());
}

beforeEach(() => {
  vi.clearAllMocks();
  resetLaunchFileQueueForTest();
  queued = [];
  nudge = null;
  held = new Map();
  parsers.importDmp.mockImplementation(holdParse);
  parsers.parseTml.mockImplementation(holdParse);
  useUIStore.setState({
    activeDialog: null,
    dialogData: {},
    importContext: null,
    placementMode: null,
    screenOccupants: 0,
    importParse: null,
    showToast,
  });
  useProjectStore.setState({ project: null, isDirty: false, ...parsers } as never);
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  listenMock.mockImplementation(async (_event: string, handler: () => void) => {
    nudge = handler;
    return () => {};
  });
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd !== 'take_pending_launch_files') return undefined;
    const batch = queued;
    queued = [];
    return batch;
  });
});

afterEach(() => {
  useProjectStore.setState({ project: null, isDirty: false });
  useUIStore.setState({ activeDialog: null, dialogData: {}, importParse: null, showToast: realShowToast });
});

describe('an OS-launched file waits for an import whose file is still parsing (#576 W62-Q1-D1)', () => {
  it('opens once the parse settles with nothing to show', async () => {
    await mountLaunchChannel();
    const first = routeOpenedFile(PICKED_DMP);
    await waitFor(() => expect(useUIStore.getState().importParse?.path).toBe(PICKED_DMP));

    launch(LAUNCHED);
    await timerTurns(5);
    // Held, not refused: no toast, no parse of it, and nothing on screen.
    expect(inProgressToasts()).toBe(0);
    expect(parsers.parseTml).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBeNull();

    parsed(PICKED_DMP).resolve(null); // the quiet abandon
    await first;

    await waitFor(() => expect(parsers.parseTml).toHaveBeenCalledWith(LAUNCHED));
    parsed(LAUNCHED).resolve({ cave_name: 'Launched', sections: [] });
    await waitFor(() => expect(useUIStore.getState().activeDialog).toBe('new-project-for-import'));
    expect(useUIStore.getState().dialogData).toMatchObject({ filePath: LAUNCHED });
    expect(inProgressToasts()).toBe(0);
  });

  it('opens once the error the failed parse raised is dismissed', async () => {
    await mountLaunchChannel();
    const first = routeOpenedFile(PICKED_DMP);
    await waitFor(() => expect(useUIStore.getState().importParse?.path).toBe(PICKED_DMP));

    launch(LAUNCHED);
    await timerTurns(5);
    expect(inProgressToasts()).toBe(0);

    parsed(PICKED_DMP).reject(new Error('No valid sections found'));
    await first;
    // The first import's error owns the screen; the launched file still waits.
    expect(useUIStore.getState().activeDialog).toBe('error');
    await timerTurns(5);
    expect(parsers.parseTml).not.toHaveBeenCalled();

    useUIStore.getState().closeDialog();
    await waitFor(() => expect(parsers.parseTml).toHaveBeenCalledWith(LAUNCHED));
    parsed(LAUNCHED).resolve({ cave_name: 'Launched', sections: [] });
    await waitFor(() => expect(useUIStore.getState().dialogData).toMatchObject({ filePath: LAUNCHED }));
    expect(useUIStore.getState().activeDialog).toBe('new-project-for-import');
    expect(inProgressToasts()).toBe(0);
  });

  it('opens once the import the good parse opened is cancelled', async () => {
    await mountLaunchChannel();
    const first = routeOpenedFile(PICKED_DMP);
    await waitFor(() => expect(useUIStore.getState().importParse?.path).toBe(PICKED_DMP));

    launch(LAUNCHED);
    await timerTurns(5);
    expect(inProgressToasts()).toBe(0);

    parsed(PICKED_DMP).resolve({ sections: [] });
    await first;
    // The first file's import, not the launched one, is on screen.
    expect(useUIStore.getState().activeDialog).toBe('new-project-for-import');
    expect(useUIStore.getState().dialogData).toMatchObject({ filePath: PICKED_DMP });
    await timerTurns(5);
    expect(parsers.parseTml).not.toHaveBeenCalled();

    useUIStore.getState().closeDialog(); // the user cancels that import
    await waitFor(() => expect(parsers.parseTml).toHaveBeenCalledWith(LAUNCHED));
    parsed(LAUNCHED).resolve({ cave_name: 'Launched', sections: [] });
    await waitFor(() => expect(useUIStore.getState().dialogData).toMatchObject({ filePath: LAUNCHED }));
    expect(useUIStore.getState().activeDialog).toBe('new-project-for-import');
    expect(inProgressToasts()).toBe(0);
  });

  it('opens once an import cancelled while its file parsed has let go of the parse', async () => {
    // A project is open, so the picked TML went through the import-as-new
    // confirm, which stays up while the file parses.
    useProjectStore.setState({ project: { name: 'ProjectA' } as never });
    useUIStore.setState({ activeDialog: 'import-as-new-project', dialogData: { path: PICKED_TML } });
    await mountLaunchChannel();
    const closeCurrent = vi.fn(async () => ({ success: true }));
    const first = routeOpenedFile(PICKED_TML, 'Open Failed', { importAsNew: { closeCurrent } });
    await waitFor(() => expect(useUIStore.getState().importParse?.path).toBe(PICKED_TML));

    launch(LAUNCHED);
    await timerTurns(5);
    expect(inProgressToasts()).toBe(0);

    // Cancel on the confirm: the dialog goes, the parse is still running.
    useUIStore.getState().closeDialog();
    await timerTurns(5);
    expect(inProgressToasts()).toBe(0);
    expect(useUIStore.getState().activeDialog).toBeNull();

    // The cancelled parse lands and is dropped; the launched file goes next.
    parsed(PICKED_TML).resolve({ cave_name: 'CaveA', sections: [] });
    await first;
    expect(closeCurrent).not.toHaveBeenCalled();
    await waitFor(() => expect(useUIStore.getState().activeDialog).toBe('import-as-new-project'));
    expect(useUIStore.getState().dialogData).toMatchObject({ path: LAUNCHED });
    expect(inProgressToasts()).toBe(0);
  });
});
