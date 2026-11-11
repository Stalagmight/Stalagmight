/**
 * #345: the backend closes and reopens the project at a DIFFERENT revision tree in
 * place, bypassing `loadProject`'s full reset path. It already resets selection and
 * the tool transients; this pins that the gas-plan store gets the same treatment.
 *
 * #258 is what makes a leftover plan dangerous here specifically: two `.stalx` under
 * one SpeleoDB project are independent SQLite databases with independently assigned
 * section ids, so a restore can swap in a survey whose ids collide with the old one by
 * construction. A pin or anchor that survives the swap silently renames itself onto
 * whatever station holds that id in the new tree.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: invokeIpcMock }));

import { useRestoreRevision, type RestoreRevisionArgs } from '../useRestoreRevision';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';

const ARGS: RestoreRevisionArgs = {
  projectId: 'p1',
  projectName: 'Cave A',
  hexsha: 'abcdef0123',
  shortSha: 'abcdef01',
  restore: true,
};

const RESTORED = { local_path: '/cache/p1.stalx' };

const run = async (over: Partial<RestoreRevisionArgs> = {}): Promise<void> => {
  const { result } = renderHook(() => useRestoreRevision());
  await act(async () => {
    await result.current({ ...ARGS, ...over });
  });
};

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(RESTORED);
  useGasPlanStore.getState().reset();
  useProjectStore.setState({
    refreshProjectState: vi.fn(async () => {}),
    checkProjectIntegrity: vi.fn(async () => []),
    isDirty: true,
  });
  useSpeleoStore.setState({
    remoteProjectId: 'p1',
    refreshLinkStatus: vi.fn(async () => {}),
  });
  useUIStore.setState({ showToast: vi.fn(), setView: vi.fn(), clearSelection: vi.fn() });
});

describe('useRestoreRevision resets the gas-plan store (#345)', () => {
  it('leaves NO live gas-plan state behind after the swap', async () => {
    // A live plan with a positional pin and a captured anchor for it - the exact shape
    // #345 says survives today: `useGasPlanStore.reset()` has exactly one production
    // caller (`applyDisplaySettings`), reached only from `loadProject`/`createProject`,
    // never from this in-place swap.
    useGasPlanStore.getState().setEndpoint({ section_id: 1, station_index: 2 });
    useGasPlanStore.getState().setPushyAck(true);
    useGasPlanStore.setState({
      loadedPlanId: 7,
      loadedPlanBaseline: useGasPlanStore.getState().loadedPlanBaseline,
      anchors: {
        origin: null,
        endpoint: { id: 'anchor-1' },
        waypoints: [],
        markedPoints: [],
        stageDropOverrides: [],
      },
      anchoredPins: {
        origin: null,
        endpoint: { section_id: 1, station_index: 2 },
        waypoints: [],
        markedPoints: [],
        stageDropOverrides: [],
      },
    });
    const before = useGasPlanStore.getState();
    expect(before.endpoint).not.toBeNull();
    expect(before.anchors).not.toBeNull();
    expect(before.loadedPlanId).not.toBeNull();
    expect(before.pushyAck).toBe(true);

    await run();

    const after = useGasPlanStore.getState();
    expect(after.endpoint).toBeNull();
    expect(after.origin).toBeNull();
    expect(after.waypoints).toEqual([]);
    expect(after.anchors).toBeNull();
    expect(after.anchoredPins).toBeNull();
    expect(after.loadedPlanId).toBeNull();
    expect(after.loadedPlanBaseline).toBeNull();
    expect(after.pushyAck).toBe(false);
  });

  it('does not apply stale cleanup after the open remote project changes', async () => {
    invokeIpcMock.mockImplementation(async () => {
      useSpeleoStore.setState({ remoteProjectId: 'p2' });
      return RESTORED;
    });
    useGasPlanStore.getState().setEndpoint({ section_id: 1, station_index: 2 });

    const { result } = renderHook(() => useRestoreRevision());
    let applied = true;
    await act(async () => {
      applied = await result.current(ARGS);
    });

    expect(applied).toBe(false);
    expect(useGasPlanStore.getState().endpoint).toEqual({ section_id: 1, station_index: 2 });
    expect(useProjectStore.getState().refreshProjectState).not.toHaveBeenCalled();
  });
});

/**
 * A View / Restore download is unbounded and nothing blocks the workspace while it
 * runs, so its reply can land after the user opened another project. The backend
 * then refuses the swap (`SessionMoved`) and leaves that project alone; the reply is
 * about a project no longer on screen and must end as a quiet stale result.
 */
describe('useRestoreRevision late replies', () => {
  const SESSION_MOVED = {
    message: 'the project changed while this request was in flight; nothing was changed - try again',
    action: 'Retry',
    status: null,
    transport: null,
    unpushed_local: null,
    newer_schema: null,
  };

  const settle = async (over: Partial<RestoreRevisionArgs> = {}) => {
    const { result } = renderHook(() => useRestoreRevision());
    let outcome: { applied: boolean } | { thrown: unknown } = { applied: true };
    await act(async () => {
      try {
        outcome = { applied: await result.current({ ...ARGS, ...over }) };
      } catch (e) {
        outcome = { thrown: e };
      }
    });
    return outcome;
  };

  it.each([
    ['a local project', null],
    ['another SpeleoDB project', 'p2'],
  ])('a refusal that lands after the user opened %s resolves false quietly', async (_, now) => {
    invokeIpcMock.mockImplementation(async () => {
      useSpeleoStore.setState({ remoteProjectId: now });
      throw SESSION_MOVED;
    });

    for (const restore of [false, true]) {
      expect(await settle({ restore })).toEqual({ applied: false });
      useSpeleoStore.setState({ remoteProjectId: 'p1' });
    }
    expect(useUIStore.getState().showToast).not.toHaveBeenCalled();
    expect(useUIStore.getState().setView).not.toHaveBeenCalled();
    expect(useProjectStore.getState().refreshProjectState).not.toHaveBeenCalled();
  });

  it('any failure that lands after a sign-out or a new sign-in is stale too', async () => {
    invokeIpcMock.mockImplementation(async () => {
      useSpeleoStore.setState({
        sessionGeneration: useSpeleoStore.getState().sessionGeneration + 1,
      });
      throw new Error('network went away');
    });
    expect(await settle()).toEqual({ applied: false });
    expect(useUIStore.getState().showToast).not.toHaveBeenCalled();
  });

  it('still throws while the project it was issued for is the one open', async () => {
    // e.g. the user switched away and straight back: the frontend sees the same
    // project, the backend saw a new session and refused. The caller then shows the
    // backend's own neutral "nothing was changed - try again".
    invokeIpcMock.mockRejectedValue(SESSION_MOVED);
    expect(await settle()).toEqual({ thrown: SESSION_MOVED });
    expect(useProjectStore.getState().refreshProjectState).not.toHaveBeenCalled();
  });
});
