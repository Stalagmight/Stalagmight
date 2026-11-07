/**
 * [#576 W71-NF6] A plan load answered after the bar closed. The bar lives in Plan only, so a
 * mode switch while `load_dive_plan` is out unmounts it; the answer still applies the plan
 * to the store (`applyLoadedPlan` outlives the bar), and the notices saying what the load
 * dropped - stations that no longer exist, span speeds out of range - must still reach the
 * user when they come back to Plan.
 *
 * Component tier: the real bar, hosted only while the workspace is Plan (as
 * `SingleSystemView` hosts it), and the real stores; the dive-plan IPC wrappers are mocked
 * and the load answer is released by hand. All data is synthetic.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/divePlanApi', () => ({
  listDivePlans: vi.fn(),
  loadDivePlan: vi.fn(),
  saveDivePlan: vi.fn(),
  deleteDivePlan: vi.fn(),
  planAnchorsOf: vi.fn(() => null),
  planAnchorsForRefs: vi.fn(async () => []),
  planRefsForAnchors: vi.fn(async () => []),
}));

// jsdom does not implement scrollIntoView; the portal Select calls it on open.
Element.prototype.scrollIntoView = vi.fn();

import { DivePlanBar } from '../DivePlanBar';
import { useProjectStore } from '../../../store/projectStore';
import { useGasPlanStore } from '../../../store/gasPlanStore';
import { useUIStore } from '../../../store/uiStore';
import { listDivePlans, loadDivePlan } from '../../../utils/divePlanApi';
import { divePlanSnapshotOf } from '../../../utils/divePlan';
import { deferred, type Deferred } from '../../../test/factories/async';
import type { DivePlan, DivePlanMeta } from '../../../types/ipc.generated';

const CAVE_A = { name: 'Cave A', created_at: '2026-07-11T00:00:00Z', file_path: '/mock/a.stalx' };
const PLAN_META: DivePlanMeta = { id: 1, name: 'Plan of A', created_at: '2026-07-12T00:00:00Z', updated_at: '2026-07-12T00:00:00Z' };
const STALE_NOTICE = '2 stations this plan referenced no longer exist';
const SPEED_NOTICE = '1 span speed in this plan was outside the range';

/** A saved plan whose load dropped two stations (backend) and one span speed (store). */
function shortenedPlan(): DivePlan {
  useGasPlanStore.getState().reset();
  const base = divePlanSnapshotOf(useGasPlanStore.getState());
  const snapshot = {
    ...base,
    origin: { section_id: 1, station_index: 2 },
    segments: base.segments.map((seg) => ({ ...seg, speed_m_per_min: 10_000 })),
  };
  return {
    meta: PLAN_META,
    snapshot,
    stale_ref_count: 2,
    anchors: { origin: null, endpoint: null, waypoints: [], marked_points: [], stage_drop_overrides: [] },
  };
}

/** The bar as `SingleSystemView` hosts it: only while the workspace is Plan. */
function PlanDock() {
  const appMode = useUIStore((s) => s.appMode);
  return appMode === 'plan' ? <DivePlanBar /> : null;
}

let loadAnswer: Deferred<DivePlan>;

async function pickTheSavedPlan(): Promise<void> {
  await userEvent.click(await screen.findByTestId('dive-plan-select'));
  await userEvent.click(await screen.findByTestId('dive-plan-select-option-1'));
  expect(vi.mocked(loadDivePlan)).toHaveBeenCalledWith(1);
}

async function answerTheLoad(): Promise<void> {
  await act(async () => {
    loadAnswer.resolve(shortenedPlan());
    await loadAnswer.promise;
  });
}

function setMode(mode: 'plan' | 'survey'): void {
  act(() => useUIStore.getState().setAppMode(mode));
}

describe('DivePlanBar: a load answered after the bar closed (#576 W71-NF6)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    loadAnswer = deferred<DivePlan>();
    vi.mocked(listDivePlans).mockImplementation(async () => [PLAN_META]);
    vi.mocked(loadDivePlan).mockImplementation(() => loadAnswer.promise);
    useProjectStore.setState({ project: CAVE_A as never, databaseOpen: true });
    useUIStore.setState({ view: 'single', appMode: 'survey' });
    useGasPlanStore.getState().reset();
    setMode('plan');
  });

  it('coming back to Plan shows what the load dropped', async () => {
    render(<PlanDock />);
    await pickTheSavedPlan();

    setMode('survey');
    expect(screen.queryByTestId('dive-plan-bar')).toBeNull();
    await answerTheLoad();
    // The shortened plan landed while the bar was closed.
    expect(useGasPlanStore.getState().loadedPlanId).toBe(1);

    setMode('plan');
    expect(await screen.findByTestId('dive-plan-stale-warning')).toHaveTextContent(STALE_NOTICE);
    expect(screen.getByTestId('dive-plan-speeds-dropped-warning')).toHaveTextContent(SPEED_NOTICE);
  });

  it('control: a load answered while the bar is open shows the same notices', async () => {
    render(<PlanDock />);
    await pickTheSavedPlan();
    await answerTheLoad();

    expect(screen.getByTestId('dive-plan-stale-warning')).toHaveTextContent(STALE_NOTICE);
    expect(screen.getByTestId('dive-plan-speeds-dropped-warning')).toHaveTextContent(SPEED_NOTICE);
  });

  it('the notices do not outlive the plan store: a project load clears them', async () => {
    render(<PlanDock />);
    await pickTheSavedPlan();
    setMode('survey');
    await answerTheLoad();

    // A project load resets the plan store (`applyDisplaySettings`).
    act(() => useGasPlanStore.getState().reset());
    setMode('plan');
    await screen.findByTestId('dive-plan-bar');

    expect(screen.queryByTestId('dive-plan-stale-warning')).toBeNull();
    expect(screen.queryByTestId('dive-plan-speeds-dropped-warning')).toBeNull();
  });
});
