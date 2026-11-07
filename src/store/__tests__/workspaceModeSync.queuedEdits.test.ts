/**
 * #576 RV2-FE-G1 (C5): switching to Plan while a Survey edit is still queued must not
 * get that edit refused.
 *
 * Survey edits wait in the store's mutation queue behind the previous edit's aftermath
 * (its refresh and integrity re-check). The workspace mode was sent on a chain of its
 * own, so a click on Plan in that window reached the backend BEFORE the queued edit,
 * and the backend - which refuses survey edits while it is told Plan or Export - threw
 * away an edit the user made in Survey, with the "switch to Survey to edit" notice.
 * The mode now goes through the same queue, so every edit issued before the switch
 * lands first.
 *
 * Store tier: the real projectStore queue and the real mode sync, with the IPC mocked.
 * The mock refuses an update_shot while the last mode it was told is not Survey, as
 * the backend's workspace gate does (commands run in arrival order).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { startWorkspaceModeSync } from '../workspaceModeSync';
import { invokeIpcMock } from '../../test/factories/ipc';
import { nextTimer } from '../../test/factories/async';

vi.mock('../../utils/ipc');

let order: string[];
let backendMode: string;
let releaseRefresh: () => void;
/** Set by a test to make the first edit's refresh wait until `releaseRefresh`. */
let slowFirstRefresh: boolean;
let stop: () => void;

beforeEach(() => {
  vi.clearAllMocks();
  order = [];
  backendMode = 'survey';
  slowFirstRefresh = false;
  invokeIpcMock.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    if (command === 'set_workspace_mode') {
      backendMode = String(args?.mode);
      order.push(`set_workspace_mode:${backendMode}`);
      return null;
    }
    if (command === 'update_shot') {
      order.push(`update_shot:${String(args?.shotId)}@${backendMode}`);
      if (backendMode !== 'survey') throw { code: 'WORKSPACE_READ_ONLY', message: 'refused' };
      return { warnings: [], remote_link: null };
    }
    return undefined;
  });
  const refreshGate = new Promise<void>((resolve) => {
    releaseRefresh = resolve;
  });
  let refreshCalls = 0;
  useUIStore.setState({ appMode: 'survey', toasts: [], activeDialog: null });
  useProjectStore.setState({
    project: { file_path: '/CaveA.stalx', created_at: 'p1', sections: [] } as never,
    // The first edit's refresh is slow (a large project): the second edit waits behind it.
    refreshProjectState: vi.fn(async () => {
      refreshCalls += 1;
      if (refreshCalls === 1 && slowFirstRefresh) await refreshGate;
    }),
    refreshUndoState: vi.fn().mockResolvedValue(undefined),
    checkProjectIntegrity: vi.fn().mockResolvedValue([]),
    isDirty: false,
  });
  stop = startWorkspaceModeSync();
});

afterEach(() => {
  stop();
});

describe('entering Plan while a Survey edit is queued (RV2-FE-G1)', () => {
  it('sends the mode after the queued edit, which lands', async () => {
    slowFirstRefresh = true;
    const first = useProjectStore.getState().updateShot(1, 1, { length: 5 });
    await nextTimer();
    // Issued in Survey, queued behind the first edit's refresh.
    const second = useProjectStore.getState().updateShot(1, 2, { length: 6 });
    await nextTimer();
    useUIStore.getState().setAppMode('plan');
    await nextTimer();
    releaseRefresh();
    const secondResult = await second;
    await first;
    await nextTimer();

    expect(order).toEqual([
      'set_workspace_mode:survey',
      'update_shot:1@survey',
      'update_shot:2@survey',
      'set_workspace_mode:plan',
    ]);
    expect(secondResult.success, 'the edit made in Survey was refused').toBe(true);
    expect(useUIStore.getState().toasts).toEqual([]);
  });

  it('keeps an edit issued after the switch behind it, so the gate still refuses it', async () => {
    useUIStore.getState().setAppMode('plan');
    const late = await useProjectStore.getState().updateShot(1, 3, { length: 7 });
    await nextTimer();

    expect(order).toEqual(['set_workspace_mode:survey', 'set_workspace_mode:plan', 'update_shot:3@plan']);
    expect(late.success).toBe(false);
  });
});
