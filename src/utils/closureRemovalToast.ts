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
 * Tell the user about every closure a delete removed, then hand the result on
 * unchanged, so a store action can wrap its command in one expression.
 *
 * One toast for the whole delete (a range can remove more than one closure):
 * the messages are the backend's own words, joined, under one dedup key - one
 * event, one surface (docs/notifications.md). A warning toast, not an error:
 * the delete did what was asked, and one Undo brings the closure back with the
 * shots.
 */
export function notifyRemovedClosures(result: CommandResult): CommandResult {
  if (!result.success) return result;
  const messages = (result.warnings ?? [])
    .filter((w) => w.code === CLOSURE_REMOVED_ACROSS_ONE_SHOT)
    .map((w) => w.message);
  if (messages.length > 0) {
    useUIStore.getState().showToast(messages.join(' '), TOAST_KEYS.CLOSURE_REMOVED, 'warning');
  }
  return result;
}
