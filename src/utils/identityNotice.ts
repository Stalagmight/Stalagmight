/**
 * The notice of what an action did to station names on its own (plan
 * 2026-11-23, RS 7 "Notices"): a merge without a dialog, a delete that brings
 * an archived station back (RS 4.3), a closure removal (RS 5.3) or a promotion
 * may letter a station so no line shows a name twice (rule L), restore an
 * archived station, make a loop closure's hidden end show its own name again,
 * or hide a comment (a promotion archived its station, RS 12). The backend
 * reports these on `CommandResult.identities`; nothing the
 * user saw changes name silently (docs/notifications.md), so one toast per
 * action names each. A dialog's own answers are the user's and are not
 * reported; imports report theirs in the receipt only.
 */
import i18n from '../i18n';
import { TOAST_DEDUP_WINDOW_MS, useUIStore } from '../store/uiStore';
import type { CommandResult } from '../types/commands';
import type { IdentityNotice } from '../types/ipc/IdentityNotice';
import { formatStationLabel } from './formatters';
import { TOAST_KEYS } from './toastKeys';

/** The notice's words, or null when the action did nothing to names on its own. */
export function identityNoticeMessage(notice: IdentityNotice | undefined, lineName: (lineId: number) => string): string | null {
  if (!notice) return null;
  const parts: string[] = [];
  if (notice.lettered.length > 0) {
    const renamed = notice.lettered
      .map((l) => i18n.t('notices.identities.letter', { station: formatStationLabel(l.from, lineName(l.line_id)), to: l.to }))
      .join(', ');
    parts.push(i18n.t('notices.identities.lettered', { renamed }));
  }
  for (const r of notice.restored) {
    const station = formatStationLabel(r.station.name, r.station.home_line === undefined ? null : lineName(r.station.home_line));
    parts.push(i18n.t('notices.identities.restored', { station, id: r.station.id }));
    if (r.comment_hidden !== undefined) {
      parts.push(i18n.t('notices.identities.commentHidden', { id: r.old_id }));
    }
  }
  if (notice.unarchived.length > 0) {
    parts.push(i18n.t('notices.identities.unarchived', { count: notice.unarchived.length }));
  }
  for (const h of notice.hidden ?? []) {
    const station = formatStationLabel(h.name, h.home_line === undefined ? null : lineName(h.home_line));
    parts.push(i18n.t('notices.identities.hiddenComment', { station }));
  }
  return parts.length > 0 ? parts.join(' ') : null;
}

/**
 * Toast what a successful action did to names on its own, then hand the
 * result on unchanged (so a store action can wrap its command in one
 * expression). A `warning`: the action did what was asked, with names the
 * user did not choose; one Undo brings them back with the rest of it.
 *
 * Every action's notice rides one dedup key, and the toast stack refreshes a
 * toast of the same key in place inside its burst window. So a notice that
 * lands while an earlier one is still fresh joins it rather than replacing
 * it (review finding ui-9): for an action without a dialog the notice is the
 * only report of its letters. The same words twice in a burst are said once.
 */
export function notifyIdentityChanges(result: CommandResult, lineName: (lineId: number) => string): CommandResult {
  if (!result.success) return result;
  const message = identityNoticeMessage(result.identities, lineName);
  if (message === null) return result;
  const ui = useUIStore.getState();
  const now = performance.now();
  const fresh = ui.toasts.find(
    (t) => t.key === TOAST_KEYS.STATION_IDENTITIES && !t.leaving && now - t.refreshedAt < TOAST_DEDUP_WINDOW_MS,
  );
  const joined = fresh && !fresh.message.includes(message) ? `${fresh.message} ${message}` : (fresh?.message ?? message);
  ui.showToast(joined, TOAST_KEYS.STATION_IDENTITIES, 'warning');
  return result;
}
