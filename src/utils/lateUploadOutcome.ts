/**
 * frontend#2: an upload whose dialog the user walked away from (allowed once the
 * 120-second notice shows) still lands, or fails, later. Its outcome is applied by
 * `useSpeleoActions` whether or not the dialog is still open - the callers pass a
 * project/session check as `isCurrent`, never the dialog's identity - so the link,
 * the success toast and the failure banner all follow the real result.
 *
 * What the dialog would have shown in its place is the failure. A late success
 * already has its toast (`push` / `done`); a divergence opens its own resolution
 * dialog, and `not_editing` raises its own toast. Everything else that did not
 * land gets one toast here, beside the durable failure banner, because the user
 * is no longer looking at the dialog that would have said so.
 */
import i18n from '../i18n';
import { useUIStore } from '../store/uiStore';
import { TOAST_KEYS } from './toastKeys';

export function toastLateUploadOutcome(result: { kind: string } | null): void {
  const kind = result?.kind;
  if (kind === 'pushed' || kind === 'released' || kind === 'divergence_needed' || kind === 'not_editing') return;
  useUIStore.getState().showToast(
    i18n.t('speleodb.editBar.toasts.lateUploadFailed'),
    TOAST_KEYS.SPELEODB_LATE_UPLOAD,
    'error',
  );
}
