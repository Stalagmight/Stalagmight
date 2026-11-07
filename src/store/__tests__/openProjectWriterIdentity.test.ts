/**
 * [#576 RVF-c-C1] The IPCs that write the open project beyond the edits and the save
 * name the project they were made for (`expectedProject`), so the backend can refuse
 * one that arrives after a switch with PROJECT_CHANGED. Pinned here at two call
 * sites: the dive-plan save and delete (the identity the caller captured rides the
 * IPC) and two SpeleoDB verbs of the store (the open project's identity at the call).
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { invokeIpcMock } from '../../test/factories/ipc';
import { useProjectStore } from '../projectStore';
import { useSpeleoStore } from '../speleoStore';
import { saveDivePlan, deleteDivePlan } from '../../utils/divePlanApi';
import type { DivePlanSnapshot } from '../../types/ipc.generated';

vi.mock('../../utils/ipc');

const CAVE_A = { createdAt: '2026-01-02T03:04:05Z', filePath: '/caves/CaveA.stalx' };

/** The args object `invokeIpc` was last called with for `command`. */
function argsOf(command: string): Record<string, unknown> | undefined {
  const calls = invokeIpcMock.mock.calls.filter(([cmd]) => cmd === command);
  const call = calls[calls.length - 1];
  return call?.[1] as Record<string, unknown> | undefined;
}

describe('IPCs that write the open project carry the project they were made for', () => {
  beforeEach(() => {
    invokeIpcMock.mockReset();
    invokeIpcMock.mockResolvedValue(null);
  });

  it('a dive-plan save and delete send the identity their caller captured', async () => {
    const snapshot = {} as DivePlanSnapshot;
    await saveDivePlan(7, 'Plan', snapshot, CAVE_A);
    await deleteDivePlan(7, CAVE_A);
    await saveDivePlan(null, 'Unsaved project', snapshot, null);

    expect(argsOf('delete_dive_plan')).toEqual({ id: 7, expectedProject: CAVE_A });
    const saves = invokeIpcMock.mock.calls.filter(([cmd]) => cmd === 'save_dive_plan');
    expect(saves.map(([, args]) => (args as { expectedProject?: unknown }).expectedProject)).toEqual([
      CAVE_A,
      null,
    ]);
  });

  it("the store's undo-mode and web-rendering verbs send the open project's identity", async () => {
    useProjectStore.setState({
      project: {
        ...(useProjectStore.getState().project ?? {}),
        created_at: CAVE_A.createdAt,
        file_path: CAVE_A.filePath,
      } as never,
    });
    useSpeleoStore.setState({ link: null, banner: null });

    await useSpeleoStore.getState().setUndoMode('disabled');
    await useSpeleoStore.getState().enableRendering();

    expect(argsOf('speleodb_set_undo_mode')).toEqual({ mode: 'disabled', expectedProject: CAVE_A });
    expect(argsOf('speleodb_set_exclude_geojson')).toEqual({
      exclude: false,
      expectedProject: CAVE_A,
    });
  });

  it('made with no project open, they say so (null), not nothing', async () => {
    useProjectStore.setState({ project: null });
    await useSpeleoStore.getState().setUndoMode('full');
    expect(argsOf('speleodb_set_undo_mode')).toEqual({ mode: 'full', expectedProject: null });
  });
});
