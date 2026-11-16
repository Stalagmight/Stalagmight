import { useUIStore } from '../store/uiStore';
import type { CommandResult } from '../types/commands';
import i18n from '../i18n';
import { TOAST_KEYS } from './toastKeys';

/**
 * The `CommandResult` warning code of a save whose file holds GPS fixes or
 * stored accuracies (#741; `OLDER_BUILDS_DROP_GPS_FIXES` in the Rust save
 * path). An older version of the app writes a fresh file without them on its
 * next save, and neither version can tell afterwards.
 */
export const OLDER_BUILDS_DROP_GPS_FIXES = 'OLDER_BUILDS_DROP_GPS_FIXES';

/** Projects already told this session, by `created_at`. */
const noticed = new Set<string>();

/**
 * Tell the user, once per project per session, that older versions of the app
 * drop this file's GPS fixes and accuracies when they save it. The backend
 * decides whether the saved file holds any; this only shows it. A warning
 * toast, not a dialog: the save did what was asked and nothing needs deciding
 * now (docs/notifications.md), and repeating it on every autosave would be
 * noise.
 *
 * `projectKey` is the project's `created_at`, stable across edits and saves.
 */
export function noticeOlderVersionsDropFixes(result: CommandResult, projectKey: string | undefined): void {
  if (!result.success || !projectKey || noticed.has(projectKey)) return;
  if (!(result.warnings ?? []).some((w) => w.code === OLDER_BUILDS_DROP_GPS_FIXES)) return;
  noticed.add(projectKey);
  useUIStore
    .getState()
    .showToast(i18n.t('saveNotices.olderVersionsDropGpsFixes'), TOAST_KEYS.OLDER_VERSIONS_DROP_GPS_FIXES, 'warning');
}

/** Forget which projects were told. Tests only. */
export function resetOlderVersionsNoticeForTests(): void {
  noticed.clear();
}
