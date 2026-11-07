/**
 * [#576 W60-NF2] A dive-plan reply made for the previous project never lands on the
 * bar once another project is open. The bar's success paths already dropped such a
 * reply; its failures (`setError`) and its plan list (`refresh()`) did not, so the
 * previous project's error, or its list of plans, showed on the new project's bar.
 *
 * The bar stays mounted across the change in two real flows: a load whose backend
 * answers ALREADY_OPEN with another project than the store shows (projectStore
 * `loadProject`: a new session, then the re-read project, then the view switch), and
 * a project Save As (same project, new file). The tests change the store the way
 * those flows do. All data is synthetic.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/divePlanApi', () => ({
  listDivePlans: vi.fn(async () => []),
  loadDivePlan: vi.fn(),
  saveDivePlan: vi.fn(),
  deleteDivePlan: vi.fn(),
  planAnchorsOf: vi.fn(() => null),
}));

import { DivePlanBar } from '../DivePlanBar';
import { useProjectStore } from '../../../store/projectStore';
import { noteProjectSessionChanged } from '../../../store/projectSession';
import { listDivePlans, saveDivePlan } from '../../../utils/divePlanApi';
import { IpcCommandError } from '../../../utils/commandError';
import { deferred } from '../../../test/factories/async';
import type { DivePlanMeta } from '../../../types/ipc.generated';

const CAVE_A = { name: 'Cave A', created_at: '2026-07-11T00:00:00Z', file_path: '/mock/a.stalx' };
const CAVE_B = { name: 'Cave B', created_at: '2026-08-02T00:00:00Z', file_path: '/mock/b.stalx' };
const PLAN_OF_A: DivePlanMeta = { id: 1, name: 'Plan of A', created_at: '2026-07-12T00:00:00Z', updated_at: '2026-07-12T00:00:00Z' };
const NO_PLANS = 'No saved plans yet';
const SOME_PLANS = 'Load a saved plan...';

const saveAs = async (name: string) => {
  await userEvent.click(screen.getByTestId('dive-plan-save-as'));
  await userEvent.type(screen.getByTestId('dive-plan-name-input'), name);
  await userEvent.click(screen.getByTestId('dive-plan-save-as-confirm'));
};

/** Project B replaces A while the bar stays up: the ALREADY_OPEN re-read in `loadProject`. */
function switchToB(): void {
  act(() => {
    noteProjectSessionChanged();
    useProjectStore.setState({ project: CAVE_B as never });
  });
}

/** A Save As of A: the same project, at another file (`saveProjectAs`). */
function saveProjectAs(): void {
  act(() => {
    useProjectStore.setState({ project: { ...CAVE_A, file_path: '/mock/a-copy.stalx' } as never });
  });
}

describe('DivePlanBar: a reply made for the previous project (#576 W60-NF2)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(listDivePlans).mockImplementation(async () => []);
    useProjectStore.setState({ project: CAVE_A as never, databaseOpen: true });
  });

  it('a Save as refused after the switch shows nothing on the new project', async () => {
    const answer = deferred<DivePlanMeta>();
    vi.mocked(saveDivePlan).mockReturnValueOnce(answer.promise);
    render(<DivePlanBar />);
    await saveAs('Plan A');

    switchToB();
    await act(async () => {
      answer.reject(new IpcCommandError({ code: 'PROJECT_CHANGED', message: 'not applied: another project is open now.' }));
      await answer.promise.catch(() => undefined);
    });

    expect(screen.queryByTestId('dive-plan-error')).toBeNull();
  });

  it("the previous project's plan list is not shown on the new project", async () => {
    const listOfA = deferred<DivePlanMeta[]>();
    vi.mocked(listDivePlans).mockReturnValueOnce(listOfA.promise);
    render(<DivePlanBar />);

    switchToB(); // the bar asks again, for B: no plans
    await act(async () => {
      listOfA.resolve([PLAN_OF_A]);
      await listOfA.promise;
    });

    expect(vi.mocked(listDivePlans)).toHaveBeenCalledTimes(2);
    expect(screen.getByTestId('dive-plan-select')).toHaveTextContent(NO_PLANS);
    expect(screen.getByTestId('dive-plan-select')).not.toHaveTextContent(SOME_PLANS);
  });

  it("the previous project's failed plan list shows nothing on the new project", async () => {
    const listOfA = deferred<DivePlanMeta[]>();
    vi.mocked(listDivePlans).mockReturnValueOnce(listOfA.promise);
    render(<DivePlanBar />);

    switchToB();
    await act(async () => {
      listOfA.reject(new IpcCommandError({ code: 'IO', message: 'could not read the plans' }));
      await listOfA.promise.catch(() => undefined);
    });

    expect(screen.queryByTestId('dive-plan-error')).toBeNull();
  });

  it('a Save as that fails after a project Save As shows nothing (its success is dropped too)', async () => {
    const answer = deferred<DivePlanMeta>();
    vi.mocked(saveDivePlan).mockReturnValueOnce(answer.promise);
    render(<DivePlanBar />);
    await saveAs('Plan A');

    saveProjectAs();
    await act(async () => {
      answer.reject(new IpcCommandError({ code: 'IO', message: 'disk full' }));
      await answer.promise.catch(() => undefined);
    });

    expect(screen.queryByTestId('dive-plan-error')).toBeNull();
  });

  it('control: a Save as refused for the project still open shows its error', async () => {
    const answer = deferred<DivePlanMeta>();
    vi.mocked(saveDivePlan).mockReturnValueOnce(answer.promise);
    render(<DivePlanBar />);
    await saveAs('Plan A');

    await act(async () => {
      answer.reject(new IpcCommandError({ code: 'IO', message: 'disk full' }));
      await answer.promise.catch(() => undefined);
    });

    expect(screen.getByTestId('dive-plan-error')).toHaveTextContent('disk full');
  });
});
