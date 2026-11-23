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
 *
 * The words name each station as a prose token (RS-PROSE, `utils/prose.ts`):
 * the station by its home line, as `identityLabelOf` words it, so the toast
 * (`ToastStack` renders tokens) and a delete preview (`<Prose text>`) draw a
 * pill; its live region and every plain sink read `proseToPlain`, the
 * sentence plain text wrote.
 */
import i18n from '../i18n';
import { TOAST_DEDUP_WINDOW_MS, useUIStore } from '../store/uiStore';
import type { CommandResult } from '../types/commands';
import type { IdentityNotice } from '../types/ipc/IdentityNotice';
import type { StationIdentity } from '../types/ipc/StationIdentity';
import { identityPartOf, proseToken } from './prose';
import { TOAST_KEYS } from './toastKeys';

/** An identity by its home line as a prose token (its plain words are `identityLabelOf`'s). */
function identityToken(
  id: number,
  name: string,
  homeLine: number | undefined,
  lineName: (lineId: number) => string,
  withId = true,
): string {
  const identity: StationIdentity = { id, name };
  if (homeLine !== undefined) identity.home_line = homeLine;
  const part = identityPartOf(identity, lineName);
  // A sentence that says the Station ID in words draws no superscript too.
  if (!withId && part.kind === 'station') delete part.station_id;
  return proseToken(part);
}

/**
 * The notice's words, or null when the action did nothing to names on its
 * own. Each station is a prose token: render with `<Prose text>`, or
 * `proseToPlain` for a plain-text sink.
 */
export function identityNoticeMessage(notice: IdentityNotice | undefined, lineName: (lineId: number) => string): string | null {
  if (!notice) return null;
  const parts: string[] = [];
  if (notice.lettered.length > 0) {
    const renamed = notice.lettered
      .map((l) =>
        i18n.t('notices.identities.letter', { station: identityToken(l.station_id, l.from, l.line_id, lineName), to: l.to }),
      )
      .join(', ');
    parts.push(i18n.t('notices.identities.lettered', { renamed }));
  }
  for (const r of notice.restored) {
    const station = identityToken(r.station.id, r.station.name, r.station.home_line, lineName, false);
    parts.push(i18n.t('notices.identities.restored', { station, id: r.station.id }));
    if (r.comment_hidden !== undefined) {
      // Named by line, name and text (RS-FX6, P2-7): "The comment of Annex
      // / 3, "belay on a flake", is hidden"; an older backend without the
      // old name gives the Station ID.
      const old =
        !r.old_name
          ? i18n.t('notices.identities.stationId', { id: r.old_id })
          : identityToken(r.old_id, r.old_name, r.old_home_line, lineName);
      parts.push(i18n.t('notices.identities.commentHidden', { station: old, comment: r.comment_hidden }));
    }
  }
  if (notice.unarchived.length > 0) {
    parts.push(i18n.t('notices.identities.unarchived', { count: notice.unarchived.length }));
  }
  for (const h of notice.hidden ?? []) {
    const station = identityToken(h.id, h.name, h.home_line, lineName);
    parts.push(i18n.t('notices.identities.hiddenComment', { station, comment: h.comment ?? '' }));
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
