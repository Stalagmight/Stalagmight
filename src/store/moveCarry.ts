/**
 * The move-tool carry registry (GHOST-2, #576).
 *
 * Move Station / Shot / Segment are click-to-pick-up, click-to-drop: between the
 * two clicks a ghost follows the cursor, and the commit it eventually sends was
 * computed against the survey as it stood at the PICKUP. StationRefs are
 * positional (`section_id:station_index`), so any other edit landing in between -
 * an undo, a table edit, a delete - can make that commit move the wrong station or
 * apply a stale offset. The user's rule: while a carry is in flight no other
 * survey mutation runs; one that is attempted cancels the move instead.
 *
 * This module is the single place that knows whether a carry is in flight. It is
 * plain module state, not a store, because it is read at the mutation chokepoints
 * (`projectStore.executeCommand` / `executeUndoRedo` and the few direct
 * `invokeIpc` mutation routes) and MapCanvas owns the carry itself:
 *
 * - `beginMoveCarry` at the pickup click registers MapCanvas's one cancel routine
 *   and the `projectTreeGeneration` the ghost was computed against.
 * - `refuseMutationDuringCarry` (R1) is the check every survey-mutation route runs
 *   at ISSUE time: with a carry in flight it cancels the carry, toasts once, and
 *   hands back a refused outcome (`CommandRefused`, no backend code) instead of
 *   letting the command reach IPC.
 * - `cancelMoveCarry` (R2/R4) cancels through the same routine Escape runs.
 * - `commitMoveCarry` ends the carry as ITS OWN commit and marks the one command
 *   that commit issues, so the chokepoint lets it through (R1) but re-checks the
 *   pickup generation inside the serialized section (R3,
 *   `refuseStaleCarryCommit`).
 *
 * It deliberately does NOT feed `useEditingBlocked` / `isEditingBlockedNow`: a
 * carry must not turn the UI read-only, only stop other mutations at the
 * chokepoint (docs/move-tools.md, "Integrity while carrying").
 */
import i18n from '../i18n';
import { useUIStore } from './uiStore';
import { TOAST_KEYS } from '../utils/toastKeys';
import { refusedLocally, type CommandOutcome, type CommandRefused } from './commandResult';

/** Why a carry was cancelled. Only some reasons tell the user (see `CANCEL_TOAST`). */
export type MoveCarryCancelReason =
  | 'toolSwitch'
  | 'placementMode'
  | 'projectSwitched'
  | 'otherMutation'
  | 'surveyChanged'
  | 'editingBlocked';

export interface MoveCarryHandlers {
  /** MapCanvas's single cancel routine - exactly what Escape does to the ghost. */
  cancel: () => void;
  /** Put back features a commit already hid (`SurveyLayer.endDragPreview`). */
  restorePreview: () => void;
}

/** What a carry's own commit carries into the serialized section (R3). */
export interface MoveCarryCommit {
  generation: number;
  restorePreview: () => void;
}

interface LiveCarry extends MoveCarryCommit {
  token: number;
  cancel: () => void;
}

let lastToken = 0;
let live: LiveCarry | null = null;
// Set only for the synchronous call of a commit's `send`, so exactly the one
// command that `send` issues can claim it (`takeMoveCarryCommit`).
let issuing: MoveCarryCommit | null = null;

// A tool switch or entering placement mode, like Escape (which MapCanvas runs
// straight through its cancel routine), is the user's own gesture and visibly drops
// the ghost on the spot, so it stays silent. So does closing or switching the
// project: the ghost goes with the survey it belonged to, and "the survey changed"
// would only be noise over the project that replaced it. The others cancel a move
// the user did not ask to cancel (docs/notifications.md: no silent state changes).
const CANCEL_TOAST: Partial<Record<MoveCarryCancelReason, string>> = {
  otherMutation: 'map.move.cancelledOtherChange',
  surveyChanged: 'map.move.cancelledSurveyChanged',
  editingBlocked: 'map.move.cancelledReadOnly',
};

// Every refusal this module hands out, so a caller that would otherwise surface a
// failure of its own (an error toast or dialog) can tell "refused because of a
// carry" apart and leave the notice to this module: one event, one notice.
const refusals = new WeakSet<object>();

function refusal(messageKey: string): CommandRefused {
  const result = refusedLocally(i18n.t(messageKey));
  refusals.add(result);
  return result;
}

/** Whether `value` (a command result, or what a refusing call threw) is a carry refusal. */
export function isMoveCarryRefusal(value: unknown): boolean {
  return typeof value === 'object' && value !== null && refusals.has(value);
}

function toast(messageKey: string): void {
  useUIStore.getState().showToast(i18n.t(messageKey), TOAST_KEYS.MOVE_CANCELLED, 'warning');
}

/**
 * A carry is in flight from its pickup click (for Move Segment that includes the
 * pending connected-section fetch, and the snap-merge dialog phase) until its own
 * commit starts or it is cancelled. Returns the carry's token.
 */
export function beginMoveCarry(generation: number, handlers: MoveCarryHandlers): number {
  lastToken += 1;
  live = { token: lastToken, generation, cancel: handlers.cancel, restorePreview: handlers.restorePreview };
  return lastToken;
}

/** Forget the carry without cancelling anything (its owner already cleaned up). No-op for a stale token. */
export function endMoveCarry(token: number | null): void {
  if (token !== null && live?.token === token) live = null;
}

export function isMoveCarryInFlight(): boolean {
  return live !== null;
}

/** Whether `token` is still the carry in flight (a cancelled or committed carry is not). */
export function isMoveCarryLive(token: number | null): boolean {
  return token !== null && live?.token === token;
}

/** The `projectTreeGeneration` the carry in flight was picked up against, or null. */
export function moveCarryGeneration(): number | null {
  return live?.generation ?? null;
}

/** Cancel the carry in flight through its owner's cancel routine. Returns whether there was one. */
export function cancelMoveCarry(reason: MoveCarryCancelReason): boolean {
  const carry = live;
  if (!carry) return false;
  // Cleared first, so anything the cancel routine triggers sees no carry.
  live = null;
  carry.cancel();
  const messageKey = CANCEL_TOAST[reason];
  if (messageKey) toast(messageKey);
  return true;
}

/**
 * R1, the one check every survey-mutation route runs when the mutation is ISSUED:
 * null to proceed, or the failed result to return without sending anything. With
 * a carry in flight the carry is cancelled (as Escape would) and the user is told
 * neither change happened, so they can retry the one they meant.
 */
export function refuseMutationDuringCarry(): CommandRefused | null {
  if (!cancelMoveCarry('otherMutation')) return null;
  return refusal('map.move.cancelledOtherChange');
}

/**
 * The carry's own commit. `send` must issue exactly one store command
 * synchronously (`moveStation` / `moveSegment` / `executeBatch`); that command
 * claims the commit marker at issue time and so passes R1, and its generation is
 * re-checked right before IPC (R3). A carry that is no longer in flight - cancelled
 * while a snap-merge dialog was being confirmed - sends nothing.
 */
export function commitMoveCarry<T>(
  token: number | null,
  send: () => Promise<CommandOutcome<T>>,
): Promise<CommandOutcome<T>> {
  const carry = live;
  if (!carry || carry.token !== token) return Promise.resolve(refusal('map.move.cancelled'));
  live = null;
  issuing = { generation: carry.generation, restorePreview: carry.restorePreview };
  try {
    return send();
  } finally {
    issuing = null;
  }
}

/** Claimed by the chokepoint at issue time: non-null only for a carry's own commit. */
export function takeMoveCarryCommit(): MoveCarryCommit | null {
  const commit = issuing;
  issuing = null;
  return commit;
}

/**
 * R3, run inside the serialized section right before `invokeIpc`: a commit that
 * queued behind another mutation, or whose survey was replaced while it waited,
 * no longer matches the geometry the ghost was computed against. It is refused
 * with nothing sent and the hidden features restored.
 */
export function refuseStaleCarryCommit(commit: MoveCarryCommit | null, generation: number): CommandRefused | null {
  if (!commit || commit.generation === generation) return null;
  commit.restorePreview();
  toast('map.move.cancelledSurveyChanged');
  return refusal('map.move.cancelledSurveyChanged');
}
