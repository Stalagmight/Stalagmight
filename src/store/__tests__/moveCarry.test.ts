import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import i18n from '../../i18n';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { accepted, type CommandOutcome } from '../commandResult';
import type { EditOutcome } from '../../types/ipc/EditOutcome';
import { useUIStore } from '../uiStore';
import {
  beginMoveCarry,
  cancelMoveCarry,
  commitMoveCarry,
  endMoveCarry,
  isMoveCarryInFlight,
  isMoveCarryLive,
  isMoveCarryRefusal,
  moveCarryGeneration,
  refuseMutationDuringCarry,
  refuseStaleCarryCommit,
  takeMoveCarryCommit,
  type MoveCarryCancelReason,
} from '../moveCarry';

/**
 * GHOST-2 (#576): the move-carry registry on its own - what "in flight" means, the
 * single cancel routine, the R1 refusal, the commit marker and the R3 stale check.
 * The chokepoint wiring in projectStore is pinned by projectStore.moveCarry.test.ts.
 */

const OK: CommandOutcome<EditOutcome> = accepted<EditOutcome>({ warnings: [], remote_link: null });

function handlers() {
  return { cancel: vi.fn(), restorePreview: vi.fn() };
}

describe('moveCarry registry (GHOST-2)', () => {
  let showToast: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    showToast = vi.fn();
    useUIStore.setState({ showToast } as Partial<ReturnType<typeof useUIStore.getState>> as never);
  });

  afterEach(() => {
    // Module state outlives a test: never leak a carry into the next one.
    cancelMoveCarry('toolSwitch');
  });

  it('tracks one carry from pickup until it is ended, and ignores a stale token', () => {
    expect(isMoveCarryInFlight()).toBe(false);
    expect(moveCarryGeneration()).toBeNull();

    const first = beginMoveCarry(7, handlers());
    expect(isMoveCarryInFlight()).toBe(true);
    expect(isMoveCarryLive(first)).toBe(true);
    expect(moveCarryGeneration()).toBe(7);

    // A second pickup replaces the first; the first token is dead from then on.
    const second = beginMoveCarry(8, handlers());
    expect(isMoveCarryLive(first)).toBe(false);
    endMoveCarry(first);
    expect(isMoveCarryInFlight()).toBe(true);
    expect(moveCarryGeneration()).toBe(8);

    endMoveCarry(second);
    expect(isMoveCarryInFlight()).toBe(false);
    expect(isMoveCarryLive(second)).toBe(false);
    endMoveCarry(null);
  });

  it('cancels through the registered routine exactly once and toasts only the reasons the user did not choose', () => {
    const toasted: Record<MoveCarryCancelReason, string | null> = {
      toolSwitch: null,
      placementMode: null,
      projectSwitched: null,
      otherMutation: i18n.t('map.move.cancelledOtherChange'),
      surveyChanged: i18n.t('map.move.cancelledSurveyChanged'),
      editingBlocked: i18n.t('map.move.cancelledReadOnly'),
    };
    for (const [reason, message] of Object.entries(toasted) as Array<[MoveCarryCancelReason, string | null]>) {
      showToast.mockClear();
      const h = handlers();
      beginMoveCarry(1, h);

      expect(cancelMoveCarry(reason)).toBe(true);
      expect(h.cancel, reason).toHaveBeenCalledTimes(1);
      expect(isMoveCarryInFlight()).toBe(false);
      // A second cancel finds nothing to cancel.
      expect(cancelMoveCarry(reason)).toBe(false);
      expect(h.cancel, reason).toHaveBeenCalledTimes(1);

      if (message === null) {
        expect(showToast, reason).not.toHaveBeenCalled();
      } else {
        expect(showToast, reason).toHaveBeenCalledTimes(1);
        expect(showToast).toHaveBeenCalledWith(message, TOAST_KEYS.MOVE_CANCELLED, 'warning');
      }
    }
  });

  it('runs the cancel routine with the carry already cleared, so a re-entrant check sees none', () => {
    let seenInFlight: boolean | null = null;
    beginMoveCarry(1, { cancel: () => { seenInFlight = isMoveCarryInFlight(); }, restorePreview: vi.fn() });
    cancelMoveCarry('surveyChanged');
    expect(seenInFlight).toBe(false);
  });

  it('lets a mutation through when no carry is in flight', () => {
    expect(refuseMutationDuringCarry()).toBeNull();
    expect(showToast).not.toHaveBeenCalled();
  });

  it('refuses a mutation issued during a carry: cancels once, toasts once, returns a recognisable failure', () => {
    const h = handlers();
    beginMoveCarry(3, h);

    const refused = refuseMutationDuringCarry();

    expect(refused).not.toBeNull();
    expect(refused!.success).toBe(false);
    expect(refused!.error).toBe(i18n.t('map.move.cancelledOtherChange'));
    expect(isMoveCarryRefusal(refused)).toBe(true);
    expect(h.cancel).toHaveBeenCalledTimes(1);
    expect(h.restorePreview).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(
      i18n.t('map.move.cancelledOtherChange'),
      TOAST_KEYS.MOVE_CANCELLED,
      'warning'
    );
    // The carry is gone, so the retry goes through.
    expect(refuseMutationDuringCarry()).toBeNull();
  });

  it('does not mistake an ordinary failure (or anything else) for a carry refusal', () => {
    expect(isMoveCarryRefusal({ success: false, error: 'x', code: 'FAILED', failure: { code: 'FAILED', message: 'x' } })).toBe(false);
    expect(isMoveCarryRefusal(new Error('x'))).toBe(false);
    expect(isMoveCarryRefusal(null)).toBe(false);
    expect(isMoveCarryRefusal(undefined)).toBe(false);
  });

  it("marks exactly the one command its commit issues, and ends the carry as that commit", async () => {
    const h = handlers();
    const token = beginMoveCarry(5, h);
    const claimed: Array<ReturnType<typeof takeMoveCarryCommit>> = [];

    const result = await commitMoveCarry(token, () => {
      claimed.push(takeMoveCarryCommit());
      // A second command issued by the same send gets no marker.
      claimed.push(takeMoveCarryCommit());
      return Promise.resolve(OK);
    });

    expect(result).toBe(OK);
    expect(claimed[0]).toEqual({ generation: 5, restorePreview: h.restorePreview });
    expect(claimed[1]).toBeNull();
    // Outside the send there is no marker, and the carry is no longer in flight -
    // the commit's own refresh can therefore never cancel it (R2).
    expect(takeMoveCarryCommit()).toBeNull();
    expect(isMoveCarryInFlight()).toBe(false);
    expect(h.cancel).not.toHaveBeenCalled();
    expect(showToast).not.toHaveBeenCalled();
  });

  it('sends nothing for a commit whose carry was already cancelled (or never registered)', async () => {
    const token = beginMoveCarry(5, handlers());
    cancelMoveCarry('surveyChanged');
    showToast.mockClear();
    const send = vi.fn(() => Promise.resolve(OK));

    const late = await commitMoveCarry(token, send);
    const unregistered = await commitMoveCarry(null, send);

    expect(send).not.toHaveBeenCalled();
    expect(late.success).toBe(false);
    expect(isMoveCarryRefusal(late)).toBe(true);
    expect(isMoveCarryRefusal(unregistered)).toBe(true);
  });

  it('refuses a commit whose generation moved, restoring the preview; passes a current one', () => {
    const restorePreview = vi.fn();

    expect(refuseStaleCarryCommit(null, 9)).toBeNull();
    expect(refuseStaleCarryCommit({ generation: 9, restorePreview }, 9)).toBeNull();
    expect(restorePreview).not.toHaveBeenCalled();

    const stale = refuseStaleCarryCommit({ generation: 9, restorePreview }, 10);

    expect(stale).not.toBeNull();
    expect(stale!.success).toBe(false);
    expect(isMoveCarryRefusal(stale)).toBe(true);
    expect(restorePreview).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(
      i18n.t('map.move.cancelledSurveyChanged'),
      TOAST_KEYS.MOVE_CANCELLED,
      'warning'
    );
  });
});
