import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import i18n from '../../i18n';
import { TOAST_KEYS } from '../../utils/toastKeys';
import type { CommandOutcome } from '../commandResult';
import type { EditOutcome } from '../../types/ipc/EditOutcome';
import type { BatchOutcome } from '../../types/ipc/BatchOutcome';
import type { Project } from '../../types/project';
import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import {
  beginMoveCarry,
  cancelMoveCarry,
  commitMoveCarry,
  isMoveCarryInFlight,
  isMoveCarryRefusal,
} from '../moveCarry';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

/**
 * GHOST-2 (#576): the mutation chokepoints refuse every survey mutation issued while
 * a move carry is in flight (R1) - nothing reaches IPC, the carry is cancelled once
 * and the caller gets a failed result - and a carry's own commit is re-checked
 * against the survey generation it was picked up on, inside the serialized section
 * (R3). Read-only queries are not mutations and pass untouched.
 */

/** What every mocked mutation answers: an outcome carrying the fields of all of them (#576 A12). */
const OK: EditOutcome & BatchOutcome = { warnings: [], remote_link: null, section_ids: [], reversed_sections: [] };

function sent(): string[] {
  return invokeIpcMock.mock.calls.map((call) => call[0] as string);
}

function handlers() {
  return { cancel: vi.fn(), restorePreview: vi.fn() };
}

function pickUp(h = handlers()): { token: number; h: ReturnType<typeof handlers> } {
  return { token: beginMoveCarry(useProjectStore.getState().projectTreeGeneration, h), h };
}

describe('projectStore mutation chokepoints during a move carry (GHOST-2)', () => {
  let showToast: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    invokeIpcMock.mockReset();
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'repair_project_integrity') return { fixed: 0, remaining: 0, changed: false };
      if (command === 'can_undo' || command === 'can_redo') return false;
      if (command.startsWith('get_') || command === 'check_merge_connectivity') return [];
      return OK;
    });
    useProjectStore.setState({
      project: { created_at: 'project-a', file_path: '/caves/CaveA.stalx', sections: [] } as unknown as Project,
      // A real refresh mints a new tree generation; so does this one.
      refreshProjectState: vi.fn(async () => {
        useProjectStore.setState((s) => ({ projectTreeGeneration: s.projectTreeGeneration + 1 }));
      }),
      refreshWarnings: vi.fn().mockResolvedValue(undefined),
      refreshUndoState: vi.fn().mockResolvedValue(undefined),
      checkProjectIntegrity: vi.fn().mockResolvedValue([]),
    } as Partial<ReturnType<typeof useProjectStore.getState>> as never);
    showToast = vi.fn();
    useUIStore.setState({ showToast, activeDialog: null } as Partial<ReturnType<typeof useUIStore.getState>> as never);
  });

  afterEach(() => {
    cancelMoveCarry('toolSwitch');
  });

  it('refuses a survey mutation issued during a carry: no IPC, one cancel, one toast, a failed result', async () => {
    const { h } = pickUp();

    const result = await useProjectStore.getState().updateShot(1, 2, { length: 42 } as never);

    expect(sent()).toEqual([]);
    expect(h.cancel).toHaveBeenCalledTimes(1);
    expect(isMoveCarryInFlight()).toBe(false);
    expect(result.success).toBe(false);
    expect(result.error).toBe(i18n.t('map.move.cancelledOtherChange'));
    expect(isMoveCarryRefusal(result)).toBe(true);
    // The refusal is the one notice: not the catch-all error toast as well.
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(
      i18n.t('map.move.cancelledOtherChange'),
      TOAST_KEYS.MOVE_CANCELLED,
      'warning'
    );
    // No refresh either: nothing changed, and a refresh would mint a new generation.
    expect(useProjectStore.getState().refreshProjectState).not.toHaveBeenCalled();

    // "Try it again": with the carry gone the same edit goes through.
    await useProjectStore.getState().updateShot(1, 2, { length: 42 } as never);
    expect(sent()).toEqual(['update_shot']);
  });

  it('refuses every routed command shape the same way (batch, delete, import)', async () => {
    const store = useProjectStore.getState();
    const attempts: Array<() => Promise<CommandOutcome<unknown>>> = [
      () => store.executeBatch([]),
      () => store.deleteSection(1),
      () => store.deleteShotRangeShift(1, 2, 3),
      () => store.importSections([]),
      () => store.updateProjectSettings('CaveA', {} as never, null),
    ];
    for (const attempt of attempts) {
      const { h } = pickUp();
      const result = await attempt();
      expect(isMoveCarryRefusal(result)).toBe(true);
      expect(h.cancel).toHaveBeenCalledTimes(1);
    }
    expect(sent()).toEqual([]);
  });

  it.each([
    ['undo', () => useProjectStore.getState().undo()],
    ['redo', () => useProjectStore.getState().redo()],
    ['undo_to', () => useProjectStore.getState().undoTo(2)],
    ['redo_to', () => useProjectStore.getState().redoTo(2)],
  ] as const)('refuses %s during a carry without sending it', async (_command, run) => {
    const { h } = pickUp();

    const result = await run();

    expect(sent()).toEqual([]);
    expect(h.cancel).toHaveBeenCalledTimes(1);
    expect(isMoveCarryRefusal(result)).toBe(true);
    // Not the undo/redo failure toast on top of the refusal notice.
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(expect.any(String), TOAST_KEYS.MOVE_CANCELLED, 'warning');
  });

  it('refuses the direct (non-executeCommand) mutation routes too', async () => {
    const store = useProjectStore.getState();

    let carry = pickUp();
    expect(await store.resolveWarning(1, 2, 'W1')).toBe(false);
    expect(carry.h.cancel).toHaveBeenCalledTimes(1);

    carry = pickUp();
    await store.unresolveWarning(1, 2, 'W1');
    expect(carry.h.cancel).toHaveBeenCalledTimes(1);

    carry = pickUp();
    // Repair has no failure shape: the refusal is thrown, recognisably.
    const thrown = await store.repairProjectIntegrity().then(
      () => null,
      (e: unknown) => e
    );
    expect(isMoveCarryRefusal(thrown)).toBe(true);
    expect(carry.h.cancel).toHaveBeenCalledTimes(1);

    expect(sent()).toEqual([]);
  });

  it('renaming a project by path counts only when the path is the open project', async () => {
    const store = useProjectStore.getState();

    const { h } = pickUp();
    await store.setProjectNameForPath('/caves/CaveB.stalx', 'CaveB');
    expect(sent()).toEqual(['set_project_name_for_path']);
    expect(h.cancel).not.toHaveBeenCalled();
    expect(isMoveCarryInFlight()).toBe(true);

    const refused = await store.setProjectNameForPath('/caves/CaveA.stalx', 'CaveA2');
    expect(isMoveCarryRefusal(refused)).toBe(true);
    expect(h.cancel).toHaveBeenCalledTimes(1);
    expect(sent()).toEqual(['set_project_name_for_path']);
  });

  it('leaves read-only queries alone during a carry', async () => {
    const store = useProjectStore.getState();
    const { h } = pickUp();
    const ref = { section_id: 1, station_index: 0 };

    await store.checkMergeConnectivity(ref, { section_id: 2, station_index: 0 });
    await store.getConnectedSectionIds(1);
    await store.getAnchoredSectionIds();
    await store.getUndoStack();

    expect(sent()).toEqual([
      'check_merge_connectivity',
      'get_rigid_connected_section_ids',
      'get_anchored_section_ids',
      'get_undo_stack',
    ]);
    expect(h.cancel).not.toHaveBeenCalled();
    expect(isMoveCarryInFlight()).toBe(true);
  });

  it("sends a carry's own commit picked up on the current survey", async () => {
    const { token, h } = pickUp();

    const result = await commitMoveCarry(token, () =>
      useProjectStore.getState().moveStation({ section_id: 1, station_index: 1 }, { x: 1, y: 2, z: 0 })
    );

    expect(result.success).toBe(true);
    expect(sent()).toEqual(['move_station']);
    expect(h.cancel).not.toHaveBeenCalled();
    expect(h.restorePreview).not.toHaveBeenCalled();
    expect(showToast).not.toHaveBeenCalled();
  });

  it("refuses a carry's commit when the survey changed since the pickup, sending nothing", async () => {
    const { token, h } = pickUp();
    // Something replaced the survey under the ghost (R2 would also have cancelled
    // the carry; the commit's own check is the backstop that does not rely on it).
    useProjectStore.setState((s) => ({ projectTreeGeneration: s.projectTreeGeneration + 1 }));

    const result = await commitMoveCarry(token, () =>
      useProjectStore.getState().moveSegment(1, { x: 1, y: 2, z: 0 })
    );

    expect(sent()).toEqual([]);
    expect(result.success).toBe(false);
    expect(isMoveCarryRefusal(result)).toBe(true);
    expect(h.restorePreview).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(
      i18n.t('map.move.cancelledSurveyChanged'),
      TOAST_KEYS.MOVE_CANCELLED,
      'warning'
    );
  });

  it('refuses a commit that queued behind a mutation whose refresh changed the survey', async () => {
    // A table edit is already in flight when the station is picked up...
    let release: (value: typeof OK) => void = () => {};
    invokeIpcMock.mockImplementationOnce(
      () =>
        new Promise<typeof OK>((resolve) => {
          release = resolve;
        })
    );
    const edit = useProjectStore.getState().updateShot(1, 2, { length: 42 } as never);
    await vi.waitFor(() => expect(sent()).toEqual(['update_shot']));
    const { token, h } = pickUp();

    // ...and the drop happens before it lands: the commit passes at issue time and
    // queues behind the edit, whose refresh then moves the survey generation.
    const commit = commitMoveCarry(token, () =>
      useProjectStore.getState().executeBatch([
        { type: 'move_station', station_ref: { section_id: 1, station_index: 1 }, offset: { x: 1, y: 2, z: 0 } },
      ])
    );
    release(OK);
    await edit;
    const result = await commit;

    expect(sent()).toEqual(['update_shot']);
    expect(isMoveCarryRefusal(result)).toBe(true);
    expect(h.restorePreview).toHaveBeenCalledTimes(1);
  });
});
