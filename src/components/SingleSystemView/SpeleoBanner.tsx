import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  useSpeleoStore,
  PUSH_FAILURE_DIALOGS,
  type SpeleoBanner as SpeleoBannerState,
} from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoActions } from '../../hooks/useSpeleoActions';
import { humanizeSpeleoError } from '../../utils/speleoError';
import { isProjectChangedRefusal } from '../../store/commandResult';
import { TOAST_KEYS } from '../../utils/toastKeys';

/**
 * Thin banner slot below the toolbar for SpeleoDB status (#200/#204). It surfaces
 * one non-blocking notice at a time:
 * - `push_unconfirmed` / `concurrent_session` - transient action feedback (#204);
 * - `rendering_disabled` - the server's `exclude_geojson` is set, so the web map
 *   won't refresh; offers Enable-rendering (a PATCH that takes effect on the NEXT
 *   push, gated behind an inline confirmation stating exactly that). This one is
 *   server-derived state, so it has NO Dismiss button - dismissing would just be
 *   re-derived by the next status poll; the resolution is Enable-rendering (or the
 *   flag being cleared server-side). It also preempts `no_anchor` in the store so it
 *   is always reachable.
 * - `no_anchor` - a push/unlock of a survey with no georeferenced zero datum, so the
 *   server generated no new map geometry (the map may be stale).
 * - `push_failed` (#260) - the last push/unlock threw. The action toast auto-clears
 *   after 3 s, which left a failed upload with no durable surface at all; this banner
 *   keeps the humanized error on screen and offers Retry, which replays the SAME verb
 *   (push keeps the lock, done releases it) with the same commit message. It clears on
 *   the next attempt, on a landed push/release, and when the project closes. It is
 *   HIDDEN while the dialog that issued the verb is open (`PUSH_FAILURE_DIALOGS`):
 *   that dialog stays open on failure and renders the same error inline, and a
 *   banner behind its modal scrim is dimmed, unreadable and unclickable.
 * - `lock_lost` (#576 B35(e)) - the backend switched editing off because SpeleoDB
 *   says the write lock this session was editing under is gone or someone else's.
 *   Names the new holder; with nobody on the mutex it offers Start editing.
 * None of these block editing or pushing.
 *
 * Above all of them sits the #213 force-edit banner (big-red, derived from
 * `link.forceEditing`, NOT the transient slot): while editing without the write lock
 * it persistently warns the `.stalx` is unmergeable and offers Reconnect. It preempts
 * the yellow notices so the data-safety warning is never masked.
 */
export function SpeleoBanner() {
  const { t } = useTranslation();
  const banner = useSpeleoStore((s) => s.banner);
  const dismissBanner = useSpeleoStore((s) => s.dismissBanner);
  const enableRendering = useSpeleoStore((s) => s.enableRendering);
  const forceEditing = useSpeleoStore((s) => s.link?.forceEditing ?? false);
  const lockedByOther = useSpeleoStore((s) => s.link?.lockedByOther ?? false);
  const lockHolder = useSpeleoStore((s) => s.link?.lockHolder ?? null);
  // #270: Retry re-issues push/done, and BOTH need an open edit gate. Without one
  // the backend uploads nothing and answers `not_editing` - so offering "Retry
  // upload" there is an invitation to a no-op that reads like a fix. A 401 is
  // exactly how a project lands here: `clearSession()` drops to read-only while
  // KEEPING the unpushed edits, which is also what raised the failure banner.
  const editing = useSpeleoStore((s) => s.link?.editState === 'editing');
  const showToast = useUIStore((s) => s.showToast);
  // #260: while the dialog that issued the push/unlock is open it OWNS the failure
  // and renders it inline; this banner would only render behind that dialog's modal
  // scrim - dimmed, unreadable, its Retry/Dismiss unclickable. Dismissing the dialog
  // with the failure unresolved brings the banner straight back.
  const dialogOwnsFailure = useUIStore((s) =>
    s.activeDialog !== null && PUSH_FAILURE_DIALOGS.includes(s.activeDialog),
  );
  // #213: Reconnect is the existing acquire+reconcile verb (startEditing).
  // #260: Retry re-issues the push/unlock verb that failed.
  const { startEditing, push, done, busy } = useSpeleoActions();
  // Inline second step for the Enable-rendering action (no single-click side effect
  // on server metadata; the confirm states rendering refreshes on the next push).
  const [confirmingEnable, setConfirmingEnable] = useState(false);
  const [enabling, setEnabling] = useState(false);

  // If the banner kind changes out from under the confirm sub-step (e.g. a push
  // lands and swaps in `push_unconfirmed` while the user is confirming), reset it -
  // otherwise the confirm prompt text would linger with no actionable buttons (the
  // confirm/back buttons are gated on `rendering_disabled`).
  useEffect(() => {
    if (banner?.kind !== 'rendering_disabled') setConfirmingEnable(false);
  }, [banner?.kind]);

  // #213: a force-edit session (editing WITHOUT the write lock) shows a PERSISTENT
  // big-red data-safety warning that outranks the yellow transient notices - the
  // "this .stalx cannot be merged" message must never be masked by a lesser banner.
  // Its only action is Reconnect (acquire the lock + reconcile, via startEditing);
  // it is not dismissable (backend state, re-derived every poll). When someone else
  // holds the lock we name them, since their work is what a reconnect reconciles against.
  if (forceEditing) {
    return (
      <div
        className="flex items-center justify-between gap-3 px-4 py-2 bg-error-20 text-error text-sm font-medium border-b border-border"
        role="alert"
        data-testid="speleo-banner"
        data-kind="force_edit_active"
      >
        <span>
          {lockedByOther
            ? t('speleodb.forceEdit.banner.lockedBy', {
                email: lockHolder ?? t('speleodb.editBar.someone'),
              })
            : t('speleodb.forceEdit.banner.text')}
        </span>
        <button
          type="button"
          onClick={() => void startEditing()}
          disabled={busy}
          className="text-xs underline flex-shrink-0 disabled:opacity-50"
          data-testid="speleo-banner-reconnect"
        >
          {t('speleodb.forceEdit.reconnect')}
        </button>
      </div>
    );
  }

  if (!banner) return null;
  // #260: the failure is on screen inside the dialog that caused it - never both.
  if (banner.kind === 'push_failed' && dialogOwnsFailure) return null;

  // #260: keep the failed attempt's own context so Retry replays the same verb with
  // the same commit message (narrowed once here - the JSX callbacks close over it).
  const pushFailed = banner.kind === 'push_failed' ? banner : null;
  // #576 B35(e): who holds the lock now, when the server named somebody.
  const lockLost = banner.kind === 'lock_lost' ? banner : null;

  // A total lookup rather than a ternary chain: the chain's final `else` was
  // `concurrent_session`, so a new banner kind would have silently rendered THAT
  // kind's message. Keyed on the union, so a new kind is a compile error here.
  const TEXT: Record<SpeleoBannerState['kind'], () => string> = {
    push_unconfirmed: () => t('speleodb.banner.pushUnconfirmed'),
    rendering_disabled: () => t('speleodb.banner.renderingDisabled'),
    no_anchor: () => t('speleodb.banner.noAnchor'),
    concurrent_session: () => t('speleodb.banner.concurrentSession'),
    push_failed: () =>
      t('speleodb.banner.pushFailed', { message: pushFailed?.message ?? '' }),
    lock_lost: () =>
      lockLost?.holder
        ? t('speleodb.banner.lockLostTo', { email: lockLost.holder })
        : t('speleodb.banner.lockLost'),
  };
  const text = TEXT[banner.kind]();

  // The verbs share the speleoStore `busy` lock, so a Retry while one is in flight
  // can't double-issue; the guard just stops the click from queueing another.
  const retry = () => {
    // `editing` is also what gates the button below - checked again here so the
    // no-op re-issue is impossible even if the two ever drift.
    if (!pushFailed || busy || !editing) return;
    void (pushFailed.origin === 'done'
      ? done(pushFailed.pushMessage)
      : push(pushFailed.pushMessage));
  };

  const confirmEnable = async () => {
    setEnabling(true);
    try {
      await enableRendering();
      // enableRendering clears the banner on success.
    } catch (e) {
      // [#576 Q-W44-7-D1] The web-rendering switch is a background verb: a refusal
      // because the project changed is dropped, nothing shown.
      if (isProjectChangedRefusal(e)) return;
      showToast(
        humanizeSpeleoError(e, t('speleodb.banner.enableRenderingFailed')),
        TOAST_KEYS.SPELEODB_ENABLE_RENDERING_ERROR,
        'error',
      );
    } finally {
      setEnabling(false);
      setConfirmingEnable(false);
    }
  };

  return (
    <div
      className="flex items-center justify-between gap-3 px-4 py-2 bg-warning-20 text-warning text-sm border-b border-border"
      role="status"
      data-testid="speleo-banner"
      data-kind={banner.kind}
    >
      <span>{confirmingEnable ? t('speleodb.banner.enableRenderingConfirm') : text}</span>
      <div className="flex items-center gap-3 flex-shrink-0">
        {/* #270: the retry verb needs the edit gate open. Read-only (a token loss,
            a re-verify divergence) makes an upload impossible, so offer the step
            that makes one possible instead of a button that cannot upload. */}
        {/* #576 B35(e): with nobody on the mutex, Start editing takes the lock back
            and fences the copy. With someone else on it there is nothing to start -
            the toolbar already says who - so only Dismiss is offered. */}
        {lockLost && !lockLost.holder && (
          <button
            type="button"
            onClick={() => void startEditing()}
            disabled={busy}
            className="text-xs underline disabled:opacity-50"
            data-testid="speleo-banner-lock-lost-start-editing"
          >
            {t('speleodb.banner.startEditingToRetry')}
          </button>
        )}
        {pushFailed &&
          (editing ? (
            <button
              type="button"
              onClick={retry}
              disabled={busy}
              className="text-xs underline disabled:opacity-50"
              data-testid="speleo-banner-retry-push"
            >
              {t('speleodb.banner.retryPush')}
            </button>
          ) : (
            <button
              type="button"
              onClick={() => void startEditing()}
              disabled={busy}
              className="text-xs underline disabled:opacity-50"
              data-testid="speleo-banner-start-editing-to-retry"
            >
              {t('speleodb.banner.startEditingToRetry')}
            </button>
          ))}
        {banner.kind === 'rendering_disabled' &&
          (confirmingEnable ? (
            <>
              <button
                type="button"
                onClick={() => void confirmEnable()}
                disabled={enabling}
                className="text-xs underline disabled:opacity-50"
                data-testid="speleo-banner-enable-rendering-confirm"
              >
                {enabling
                  ? t('speleodb.banner.enableRenderingBusy')
                  : t('speleodb.banner.enableRenderingConfirmAction')}
              </button>
              <button
                type="button"
                onClick={() => setConfirmingEnable(false)}
                disabled={enabling}
                className="text-xs underline disabled:opacity-50"
                data-testid="speleo-banner-enable-rendering-back"
              >
                {t('common.actions.cancel')}
              </button>
            </>
          ) : (
            <button
              type="button"
              onClick={() => setConfirmingEnable(true)}
              className="text-xs underline"
              data-testid="speleo-banner-enable-rendering"
            >
              {t('speleodb.banner.enableRendering')}
            </button>
          ))}
        {/* `rendering_disabled` is server-derived, so it has no Dismiss (a poll would
            just re-raise it): its resolution is Enable-rendering / the flag clearing.
            Every other kind is dismissable transient/action feedback. */}
        {banner.kind !== 'rendering_disabled' && (
          <button
            type="button"
            onClick={dismissBanner}
            className="text-xs underline"
            data-testid="speleo-banner-dismiss"
          >
            {t('common.actions.dismiss')}
          </button>
        )}
      </div>
    </div>
  );
}
