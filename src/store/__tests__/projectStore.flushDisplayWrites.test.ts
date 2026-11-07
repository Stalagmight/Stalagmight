/**
 * [#576 FZ-FZ-INT-6] A close or a load first flushes the queued display-setting
 * writes, so each lands in the session it was made in.
 *
 * A display write queued behind another of its key used to be sent after a close
 * and a reload of the SAME file: same `created_at`, same file, so the chain's
 * identity rule took it for the project it was made in, and the backend stored a
 * value the reopened session did not show (it showed what the reload read). Now the
 * close or load waits for the writes queued before it, bounded by
 * PROJECT_SETTING_FLUSH_TIMEOUT_MS; a write still queued (not yet sent) when the
 * bound runs out is dropped unsent.
 *
 * Store tier: the real stores and write chain; the IPC is mocked and the first write
 * of a burst is held until the test releases it. All data is synthetic.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { deferred, timerTurns, type Deferred } from '../../test/factories/async';
import { invokeIpcMock } from '../../test/factories/ipc';
import { makeProject } from '../../test/factories/survey';
import { PROJECT_SETTING_FLUSH_TIMEOUT_MS } from '../../utils/settingWriteChain';
import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';

const loadBinaryMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc');
vi.mock('../../utils/loadProjectBinary', () => ({ loadProjectFullBinary: loadBinaryMock }));

const CAVE_A = '/caves/CaveA.stalx';

/** Every IPC in send order: a display write as `key=value`. */
let sent: string[];
let releaseFirst: (() => void) | null;
/** The loads in flight, ended after each test (the switch queue is module state). */
let loads: Deferred<unknown>[];

beforeEach(() => {
  sent = [];
  releaseFirst = null;
  loads = [];
  vi.clearAllMocks();
  loadBinaryMock.mockImplementation((path: string) => {
    sent.push(`load ${path}`);
    const load = deferred<unknown>();
    loads.push(load);
    return load.promise;
  });
  invokeIpcMock.mockImplementation((command: string, args?: Record<string, unknown>) => {
    if (command === 'save_display_setting') {
      sent.push(`${String(args?.key)}=${String(args?.value)}`);
      if (releaseFirst === null && sent.filter((s) => s.includes('=')).length === 1) {
        return new Promise<null>((resolve) => {
          releaseFirst = () => resolve(null);
        });
      }
      return Promise.resolve(null);
    }
    sent.push(command);
    if (command === 'close_project') return Promise.resolve({ remote_link: null });
    return Promise.resolve(undefined);
  });
  useUIStore.setState({ appMode: 'survey', gridSpacing: 10 });
  useProjectStore.setState({
    project: makeProject({ created_at: '2026-09-01T10:00:00Z', file_path: CAVE_A }),
    isClosing: false,
  });
});

afterEach(async () => {
  vi.useRealTimers();
  for (const load of loads) load.reject({ code: 'CANCELLED', message: 'Loading cancelled' });
  await timerTurns(10);
});

describe('a close or a load first flushes the queued display writes (#576 FZ-FZ-INT-6)', () => {
  it('a close is sent after the writes queued before it', async () => {
    useUIStore.getState().setGridSpacing(20);
    await vi.waitFor(() => expect(releaseFirst).not.toBeNull());
    useUIStore.getState().setGridSpacing(50); // queued behind the held one

    const closing = useProjectStore.getState().closeProject();
    await timerTurns(5);
    expect(sent).not.toContain('close_project');

    releaseFirst?.();
    expect(await closing).toMatchObject({ success: true });
    expect(sent).toEqual(['gridSpacing=20', 'gridSpacing=50', 'close_project']);
  });

  it('a reload of the same file is sent after the writes queued before it', async () => {
    useUIStore.getState().setGridSpacing(20);
    await vi.waitFor(() => expect(releaseFirst).not.toBeNull());
    useUIStore.getState().setGridSpacing(50);

    void useProjectStore.getState().loadProject(CAVE_A);
    await timerTurns(5);
    expect(sent.some((s) => s.startsWith('load '))).toBe(false);

    releaseFirst?.();
    await vi.waitFor(() => expect(sent.some((s) => s.startsWith('load '))).toBe(true));
    expect(sent.indexOf('gridSpacing=50')).toBeLessThan(sent.indexOf(`load ${CAVE_A}`));
  });

  it(`gives up after ${PROJECT_SETTING_FLUSH_TIMEOUT_MS} ms and drops a write still queued, unsent`, async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    useUIStore.getState().setGridSpacing(20);
    await vi.waitFor(() => expect(releaseFirst).not.toBeNull());
    useUIStore.getState().setGridSpacing(50);

    const closing = useProjectStore.getState().closeProject();
    await vi.advanceTimersByTimeAsync(PROJECT_SETTING_FLUSH_TIMEOUT_MS - 1);
    expect(sent).not.toContain('close_project');

    await vi.advanceTimersByTimeAsync(1);
    expect(await closing).toMatchObject({ success: true });
    releaseFirst?.();
    await vi.advanceTimersByTimeAsync(10);
    expect(sent).toEqual(['gridSpacing=20', 'close_project']);
  });

  it('drops a write made while the flush waited, so it never lands after the reload (#576 RVF-c-A2-3)', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    useUIStore.getState().setGridSpacing(20); // wedged
    await vi.waitFor(() => expect(releaseFirst).not.toBeNull());
    const reload = useProjectStore.getState().loadProject(CAVE_A); // the same file
    await vi.advanceTimersByTimeAsync(5);
    useUIStore.getState().setGridSpacing(50); // made during the flush wait: still queued at the timeout

    await vi.advanceTimersByTimeAsync(PROJECT_SETTING_FLUSH_TIMEOUT_MS);
    await vi.waitFor(() => expect(loads).toHaveLength(1));
    loads[0]?.resolve(reloadResult());
    expect(await reload).toMatchObject({ success: true });
    releaseFirst?.(); // the wedged write answers only after the reload committed
    await vi.advanceTimersByTimeAsync(10);

    expect(sent.filter((s) => s.startsWith('gridSpacing=')), `sent: ${sent.join(', ')}`).toEqual(['gridSpacing=20']);
  });
});

/** The load answer for a reload of CaveA (the same project, a new session). */
function reloadResult() {
  return {
    loadingId: 7,
    isRemote: false,
    newerSchemaVersion: null,
    speleoLinkUnreadable: false,
    unreadableProjectSettings: [],
    loadRepairs: null,
    project: makeProject({ created_at: '2026-09-01T10:00:00Z', file_path: CAVE_A }),
    stations: [],
    surveyData: { shots: [], stations: [] },
    stats: null,
    warnings: [],
    frozen_shots: [],
    displaySettings: {},
  };
}
