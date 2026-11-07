/**
 * [#576 Q-W44-7-D1] What a verb shows when it is refused because the project changed
 * under it: the backend's PROJECT_CHANGED (the project the request names is no
 * longer the open one, `expected_project.rs`), or the store's local twin of it
 * (`isProjectChangedRefusal`).
 *
 * - A BACKGROUND verb - one the user did not start by a click: the status and link
 *   polls, the SpeleoDB undo-mode write, the web-rendering switch, a display-setting
 *   write - drops it silently. It checks `isProjectChangedRefusal(e)` and returns
 *   before its ordinary error surface.
 * - A USER-STARTED verb - a dive-plan save, save as or delete, and the SpeleoDB verbs
 *   (start editing, force edit, update, take remote, discard, done, push, publish,
 *   release lock, delete project, roster edits, metadata edits, restore, unwind,
 *   unlink) - shows the short line below INSTEAD of its ordinary error, on the
 *   surface it already uses for its errors: `projectChangedMessage` for an inline
 *   error, a dialog or a banner, `toastProjectChanged` for a verb whose errors are
 *   toasts.
 */
import i18n from '../i18n';
import { isProjectChangedRefusal } from '../store/commandResult';
import { useUIStore } from '../store/uiStore';
import { TOAST_KEYS } from './toastKeys';

/** The short line a user-started verb shows for `e` when the project changed under it, or null for any other failure. */
export function projectChangedMessage(e: unknown): string | null {
  return isProjectChangedRefusal(e) ? i18n.t('projectChanged.notFinished') : null;
}

/**
 * For a user-started verb whose errors are toasts: toast the short line when `e` is
 * a refusal because the project changed, and say whether it did (the caller shows its
 * ordinary error toast only when it did not). A warning, not an error: the verb was
 * blocked by the user's own project switch, not broken (docs/notifications.md).
 */
export function toastProjectChanged(e: unknown): boolean {
  const message = projectChangedMessage(e);
  if (message === null) return false;
  useUIStore.getState().showToast(message, TOAST_KEYS.PROJECT_CHANGED, 'warning');
  return true;
}
