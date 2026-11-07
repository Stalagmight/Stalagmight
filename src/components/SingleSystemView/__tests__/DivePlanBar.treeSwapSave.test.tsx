/**
 * [#576 W71-NF5] A dive-plan Save (or Save as) answered after an in-place SpeleoDB tree
 * swap reset the gas plan store. The swap (here History > View, `useRestoreRevision`,
 * reachable from the hamburger in Plan) reopens other bytes at the SAME path, so the
 * project session, `created_at` and `file_path` - the bar's `projectKey()` - do not move.
 * Nothing gates the bar while the swap's IPC is out, so a Save clicked then is answered
 * whenever the backend finishes it, and `markPlanSaved` is a store write that outlives
 * the bar the swap unmounted.
 *
 * Component tier: the real bar, hosted only while the workspace is Plan (as
 * `SingleSystemView` hosts it), the real swap hook and seam and the real stores; the IPC
 * is mocked and the save answer is released by hand. All data is synthetic.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, renderHook, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/ipc');

import { DivePlanBar } from '../DivePlanBar';
import { useProjectStore } from '../../../store/projectStore';
import { selectIsPlanDirty, useGasPlanStore } from '../../../store/gasPlanStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useRestoreRevision } from '../../../hooks/useRestoreRevision';
import { divePlanSnapshotOf } from '../../../utils/divePlan';
import { invokeIpcMock } from '../../../test/factories/ipc';
import { deferred, type Deferred } from '../../../test/factories/async';
import { projectStateEnvelope, type ProjectStateLike } from '../../../test/projectStateEnvelope';
import type { DivePlanMeta, DivePlanSnapshot } from '../../../types/ipc.generated';

const CREATED_AT = '2026-07-11T00:00:00Z';
const FILE_PATH = '/mock/speleodb/p1/project.stalx';
const CAVE_A = { name: 'Cave A', created_at: CREATED_AT, file_path: FILE_PATH };
const PLAN_META: DivePlanMeta = { id: 1, name: 'Plan of A', created_at: '2026-07-12T00:00:00Z', updated_at: '2026-07-12T00:00:00Z' };
const NEW_PLAN_META: DivePlanMeta = { id: 2, name: 'New plan', created_at: '2026-07-13T00:00:00Z', updated_at: '2026-07-13T00:00:00Z' };
const PLAN_ORIGIN = { section_id: 1, station_index: 2 };
const EDITED_ORIGIN = { section_id: 1, station_index: 0 };
const NO_PLAN_LOADED = 'Load a saved plan...';

/** The swapped tree: the same project file (same `created_at`, same path), other shots. */
function swappedTree(): ProjectStateLike {
  return {
    project: {
      name: 'Cave A',
      created_at: CREATED_AT,
      file_path: FILE_PATH,
      sections: [
        {
          id: 1,
          name: 'SectionA',
          color: '#336699',
          shots: [{ id: 100_000, azimuth_in: 10, azimuth_out: 190, length: 4, depth_in: 0, depth_out: 1, kind: 'Normal' }],
        },
      ],
    },
    stations: [0, 1].map((i) => ({
      station_ref: { section_id: 1, station_index: i },
      coordinates: { x: i * 4, y: 0, z: i },
    })),
    stats: {
      section_count: 1, shot_count: 1, total_length: 4, min_depth: 0, max_depth: 1,
      station_count: 2, loop_count: 0, penetration_distance: 4,
    },
    frozen_shots: [],
  };
}

/** The saved plan's snapshot: the store's defaults with its own origin. */
function savedSnapshot(): DivePlanSnapshot {
  useGasPlanStore.getState().reset();
  return { ...divePlanSnapshotOf(useGasPlanStore.getState()), origin: PLAN_ORIGIN };
}

/** The bar as `SingleSystemView` hosts it: only while the workspace is Plan. */
function PlanDock() {
  const appMode = useUIStore((s) => s.appMode);
  return appMode === 'plan' ? <DivePlanBar /> : null;
}

let saveAnswer: Deferred<DivePlanMeta>;
let viewAnswer: Deferred<unknown>;
let refreshAnswer: Deferred<ArrayBuffer> | null;

/** History > View of an older revision, as the History tab issues it. */
function viewOlderRevision(): Promise<boolean> {
  const restoreRevision = renderHook(() => useRestoreRevision()).result.current;
  return restoreRevision({ projectId: 'p1', projectName: 'Cave A', hexsha: 'abc12345def', shortSha: 'abc12345', restore: false });
}

/** Plan 1 loaded and then edited: the Save button is live. */
async function editTheLoadedPlan(): Promise<void> {
  useGasPlanStore.getState().applyLoadedPlan(1, savedSnapshot(), null);
  render(<PlanDock />);
  await screen.findByText('Plan of A');
  act(() => useGasPlanStore.getState().setOrigin(EDITED_ORIGIN));
  expect(screen.getByTestId('dive-plan-dirty')).toBeInTheDocument();
}

/** What the user is shown once back in Plan, after answering the entry question. */
async function backInPlan(): Promise<void> {
  act(() => {
    useUIStore.getState().setAppMode('plan');
    useGasPlanStore.getState().chooseEntrance();
  });
  await screen.findByTestId('dive-plan-bar');
}

function expectNoPlanLoaded(): void {
  const gas = useGasPlanStore.getState();
  expect(gas.loadedPlanId).toBeNull();
  expect(gas.loadedPlanBaseline).toBeNull();
  // The app-close guard (App.tsx `planDirty`) reads this.
  expect(selectIsPlanDirty(gas)).toBe(false);
}

describe('DivePlanBar: a save answered after an in-place tree swap (#576 W71-NF5)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    saveAnswer = deferred<DivePlanMeta>();
    viewAnswer = deferred<unknown>();
    refreshAnswer = null;
    invokeIpcMock.mockImplementation((command: string) => {
      switch (command) {
        case 'list_dive_plans':
          return Promise.resolve([PLAN_META]);
        case 'save_dive_plan':
          return saveAnswer.promise;
        case 'speleodb_restore_revision':
          return viewAnswer.promise;
        case 'get_project_state_binary':
          return refreshAnswer?.promise ?? Promise.resolve(projectStateEnvelope(swappedTree()));
        case 'speleodb_open_file_flags':
          return Promise.resolve({ newerSchemaVersion: null, speleoLinkUnreadable: false, unreadableProjectSettings: [] });
        case 'check_project_integrity':
          return Promise.resolve({ messages: [], save_blocked: false });
        case 'history_discarded':
          return Promise.resolve(false);
        case 'speleodb_link_status':
          return Promise.reject(new Error('offline'));
        default:
          return Promise.resolve(undefined);
      }
    });
    useProjectStore.setState({ project: CAVE_A as never, databaseOpen: true });
    useSpeleoStore.setState({ session: null, sessionGeneration: 0, remoteProjectId: 'p1' });
    useUIStore.setState({ view: 'single', appMode: 'survey', showToast: vi.fn() });
    useUIStore.getState().setAppMode('plan');
    useGasPlanStore.getState().reset();
  });

  it('a Save answered after the swap does not mark the reset plan as loaded and unsaved', async () => {
    await editTheLoadedPlan();
    const viewing = viewOlderRevision();
    // The swap's IPC is out and the bar still offers Save.
    expect(screen.getByTestId('dive-plan-save')).toBeEnabled();
    await userEvent.click(screen.getByTestId('dive-plan-save'));
    expect(invokeIpcMock).toHaveBeenCalledWith('save_dive_plan', expect.objectContaining({ id: 1 }), expect.anything());

    await act(async () => {
      viewAnswer.resolve({ local_path: FILE_PATH, concurrent_session: false });
      expect(await viewing).toBe(true);
    });
    // The swap reset the plan and left Plan, the bar with it.
    expect(useUIStore.getState().appMode).toBe('survey');
    expect(screen.queryByTestId('dive-plan-bar')).toBeNull();

    await act(async () => {
      saveAnswer.resolve(PLAN_META);
      await saveAnswer.promise;
    });

    expectNoPlanLoaded();
    await backInPlan();
    expect(screen.queryByTestId('dive-plan-dirty')).toBeNull();
    expect(screen.getByTestId('dive-plan-select')).toHaveTextContent(NO_PLAN_LOADED);
  });

  it("a Save answered while the swap's tree refresh is still out is dropped too", async () => {
    refreshAnswer = deferred<ArrayBuffer>();
    await editTheLoadedPlan();
    const viewing = viewOlderRevision();
    await userEvent.click(screen.getByTestId('dive-plan-save'));

    await act(async () => {
      viewAnswer.resolve({ local_path: FILE_PATH, concurrent_session: false });
      await viewAnswer.promise;
    });
    // The swap has reset the plan and is waiting for the swapped tree.
    expect(useGasPlanStore.getState().originChosen).toBe(false);
    expect(invokeIpcMock).toHaveBeenCalledWith('get_project_state_binary', undefined, expect.anything());

    await act(async () => {
      saveAnswer.resolve(PLAN_META);
      await saveAnswer.promise;
    });
    await act(async () => {
      refreshAnswer?.resolve(projectStateEnvelope(swappedTree()));
      expect(await viewing).toBe(true);
    });

    expectNoPlanLoaded();
  });

  it('a Save as of an unsaved exploration answered after the swap is not marked loaded', async () => {
    useGasPlanStore.getState().setOrigin(EDITED_ORIGIN);
    render(<PlanDock />);
    await screen.findByText(NO_PLAN_LOADED);
    const viewing = viewOlderRevision();
    await userEvent.click(screen.getByTestId('dive-plan-save-as'));
    await userEvent.type(screen.getByTestId('dive-plan-name-input'), 'New plan');
    await userEvent.click(screen.getByTestId('dive-plan-save-as-confirm'));
    expect(invokeIpcMock).toHaveBeenCalledWith('save_dive_plan', expect.objectContaining({ id: null }), expect.anything());

    await act(async () => {
      viewAnswer.resolve({ local_path: FILE_PATH, concurrent_session: false });
      expect(await viewing).toBe(true);
    });
    await act(async () => {
      saveAnswer.resolve(NEW_PLAN_META);
      await saveAnswer.promise;
    });

    expectNoPlanLoaded();
  });

  it('control: the same Save with no swap marks the plan saved', async () => {
    await editTheLoadedPlan();
    await userEvent.click(screen.getByTestId('dive-plan-save'));

    await act(async () => {
      saveAnswer.resolve(PLAN_META);
      await saveAnswer.promise;
    });

    const gas = useGasPlanStore.getState();
    expect(gas.loadedPlanId).toBe(1);
    expect(gas.loadedPlanBaseline?.origin).toEqual(EDITED_ORIGIN);
    expect(selectIsPlanDirty(gas)).toBe(false);
    expect(screen.queryByTestId('dive-plan-dirty')).toBeNull();
  });
});
