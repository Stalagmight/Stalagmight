import { useUIStore } from '../store/uiStore';
import type { CommandResult } from '../types/commands';
import { TOAST_KEYS } from './toastKeys';

/**
 * The `CommandResult` warning code a delete with shift reports for each loop
 * closure it removed because, after the delete, the closure spans a single
 * shot (#737; `CLOSURE_REMOVED_ACROSS_ONE_SHOT` in the Rust models).
 */
export const CLOSURE_REMOVED_ACROSS_ONE_SHOT = 'CLOSURE_REMOVED_ACROSS_ONE_SHOT';

/**
 * The `CommandResult` warning code an edit reports for each loop closure its
 * orphan-closure sweep removed because the closure's two stations are one point
 * (#738; `CLOSURE_REMOVED_MERGED_ENDS` in the Rust models): the delete, split,
 * unmerge and remove-datum commands, and a merge of two placed pieces (#741).
 */
export const CLOSURE_REMOVED_MERGED_ENDS = 'CLOSURE_REMOVED_MERGED_ENDS';

const CLOSURE_REMOVED_CODES: ReadonlySet<string> = new Set([
  CLOSURE_REMOVED_ACROSS_ONE_SHOT,
  CLOSURE_REMOVED_MERGED_ENDS,
]);

/**
 * Tell the user about every closure an edit removed (across one shot, #737, or
 * between merged stations, #738), then hand the result on unchanged, so a store
 * action can wrap its command in one expression.
 *
 * One toast for the whole edit (a range can remove more than one closure):
 * the messages are the backend's own words, joined, under one dedup key - one
 * event, one surface (docs/notifications.md). A warning toast, not an error:
 * the delete did what was asked, and one Undo brings the closure back with the
 * shots.
 */
export function notifyRemovedClosures(result: CommandResult): CommandResult {
  if (!result.success) return result;
  const messages = (result.warnings ?? [])
    .filter((w) => CLOSURE_REMOVED_CODES.has(w.code))
    .map((w) => w.message);
  if (messages.length > 0) {
    useUIStore.getState().showToast(messages.join(' '), TOAST_KEYS.CLOSURE_REMOVED, 'warning');
  }
  return result;
}
