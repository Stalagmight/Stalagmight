import { useTranslation } from 'react-i18next';
import {
  useSpeleoStore,
  PUSH_FAILURE_DIALOGS,
  type SpeleoBanner as SpeleoBannerState,
} from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { dispatchProjectSave } from '../../utils/dispatchProjectSave';
import { getSetAsideUploadMessage, getUploadContinuation, setUploadContinuation } from '../../utils/uploadContinuation';
import { useSpeleoExitPrompt } from '../../utils/speleoExit';
import { useSpeleoSignOut } from '../../utils/speleoSignOut';

export function SpeleoBanner() {
  const { t } = useTranslation();
  const banner = useSpeleoStore((s) => s.banner);
  const dismissBanner = useSpeleoStore((s) => s.dismissBanner);
  // #270: Retry re-issues push/done, and BOTH need an open edit gate. Without one
  // the backend uploads nothing and answers `not_editing` - so offering "Retry
  // upload" there is an invitation to a no-op that reads like a fix. A 401 is
  // exactly how a project lands here: `clearSession()` drops to read-only while
  // KEEPING the unpushed edits, which is also what raised the failure banner.
  const editing = useSpeleoStore((s) => s.link?.editState === 'editing');
  // #260: while the dialog that issued the push/unlock is open it OWNS the failure
  // and renders it inline; this banner would only render behind that dialog's modal
  // scrim - dimmed, unreadable, its Retry/Dismiss unclickable. Dismissing the dialog
  // with the failure unresolved brings the banner straight back.
  const dialogOwnsFailure = useUIStore((s) =>
    s.activeDialog !== null && PUSH_FAILURE_DIALOGS.includes(s.activeDialog),
  );
  // The exit prompt and the sign-out prompt are overlays of their own, not an
  // `activeDialog`, and both render the failure inline too (their upload step).
  const exitPromptOpen = useSpeleoExitPrompt((s) => s.request !== null);
  const signOutPromptOpen = useSpeleoSignOut((s) => s.request !== null);
  const busy = useSpeleoStore((s) => s.busy);
  // No banner, no bar. Read-only on its own is NOT a banner state: the map's
  // read-only pill and the SpeleoDB tab's lock badge already say it, and the pill
  // carries the Resume editing action.
  if (!banner) return null;
  // #260: the failure is on screen inside the dialog that caused it - never both.
  if (banner.kind === 'push_failed' && (dialogOwnsFailure || exitPromptOpen || signOutPromptOpen)) return null;

  // #260: keep the failed attempt's own context so Retry replays the same verb with
  // the same commit message (narrowed once here - the JSX callbacks close over it).
  const pushFailed = banner.kind === 'push_failed' ? banner : null;

  // Keyed on the banner union so every new kind requires its own message.
  const TEXT: Record<SpeleoBannerState['kind'], () => string> = {
    push_unconfirmed: () => t('speleodb.banner.pushUnconfirmed'),
    no_anchor: () => t('speleodb.banner.noAnchor'),
    push_failed: () =>
      t('speleodb.banner.pushFailed', { message: pushFailed?.message ?? '' }),
  };
  const text = TEXT[banner.kind]();

  // Retry returns to the shared Save confirmation, retaining the description
  // and any still-current Save-and-Close request through authentication recovery.
  const retry = () => {
    if (!pushFailed || useSpeleoStore.getState().busy || !editing) return;
    const path = useProjectStore.getState().project?.file_path;
    if (!path) return;
    const pending = getUploadContinuation(path);
    setUploadContinuation({
      path, message: pending?.message ?? getSetAsideUploadMessage(path) ?? pushFailed.pushMessage ?? '',
      closeAfterSave: pending?.closeAfterSave ?? false,
    });
    useUIStore.getState().openSpeleoWorkspace();
    if (pushFailed.origin === 'done' && !pending?.closeAfterSave) {
      // A failed explicit lock release retries that operation after confirmation.
      useUIStore.getState().openDialog('speleodb-done', { mode: 'release' });
    } else void dispatchProjectSave();
  };

  return (
    <div
      className="flex items-center justify-between gap-3 px-4 py-2 bg-warning-20 text-warning text-sm border-b border-border"
      role="status"
      data-testid="speleo-banner"
      data-kind={banner.kind}
    >
      <span>{text}</span>
      <div className="flex items-center gap-3 flex-shrink-0">
        {/* #270: the retry verb needs the edit gate open. Read-only (a token loss,
            a re-verify divergence) makes an upload impossible, so offer the step
            that makes one possible instead of a button that cannot upload. */}
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
              onClick={() => useSpeleoStore.getState().session?.loggedIn ? useUIStore.getState().openSpeleoWorkspace() : useUIStore.getState().openDialog('settings', { initialCategory: 'speleodb' })}
              disabled={busy}
              className="text-xs underline disabled:opacity-50"
              data-testid="speleo-banner-start-editing-to-retry"
            >
              {t('speleodb.banner.startEditingToRetry')}
            </button>
          ))}
        <button
          type="button"
          onClick={dismissBanner}
          className="text-xs underline"
          data-testid="speleo-banner-dismiss"
        >
          {t('common.actions.dismiss')}
        </button>
      </div>
    </div>
  );
}
