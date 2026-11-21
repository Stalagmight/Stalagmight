import { useUIStore } from '../store/uiStore';
import { TOAST_KEYS } from './toastKeys';
import { READ_ONLY_FILE_NOTICE } from './editErrors';

/**
 * Say why something the user tried is turned off (a file made with a newer
 * Stalagmight, a SpeleoDB project without the write lock, the Closures
 * Computed View): the worded warning toast every blocked edit shares
 * (`TOAST_KEYS.EDIT_DISABLED`), once while one is on screen, so a key held
 * down, every further keystroke or a disabled menu row clicked again adds
 * nothing (demo findings I-10, N-1, N-12).
 */
export function showBlockedNotice(message: string): void {
  const ui = useUIStore.getState();
  const showing = ui.toasts.some((toast) => toast.key === TOAST_KEYS.EDIT_DISABLED && !toast.leaving);
  if (!showing) ui.showToast(message, TOAST_KEYS.EDIT_DISABLED, 'warning');
}

/** {@link showBlockedNotice} in the read-only FILE's words, the same notice
 *  the backend's `READ_ONLY_FILE` refusal of an edit, an undo or a save gets. */
export function showReadOnlyFileNotice(): void {
  showBlockedNotice(READ_ONLY_FILE_NOTICE());
}
