import { useTranslation } from 'react-i18next';
import { useSpeleoStore, selectPushFailure } from '../../store/speleoStore';

/**
 * [#260] The failed-upload notice, rendered INSIDE the dialog that issued the
 * push/unlock.
 *
 * A failed "Upload & release" leaves its dialog open (correct - the work is not
 * done), but the durable failure surface was the shell banner, which sits behind
 * the dialog's modal scrim: dimmed, unreadable, and its buttons unclickable. So
 * at the moment the user most needs the error they could neither read nor act on
 * it. This renders the SAME humanized line, in the SAME warning treatment, right
 * above the buttons - as the result of the button they just pressed.
 *
 * It deliberately carries NO retry control: the retry is the dialog's own primary
 * button (pressing it again re-issues the verb, which clears this notice at the
 * start of the attempt and re-raises it only if the new attempt also fails).
 * While a dialog owning the failure is open the shell banner hides itself
 * ({@link PUSH_FAILURE_DIALOGS}, plus the exit and sign-out prompts, which are
 * overlays of their own); leaving the dialog with the failure unresolved
 * hands the banner back, so the user is still reminded their changes are only on
 * this computer.
 */
export function SpeleoPushFailureNotice() {
  const { t } = useTranslation();
  const failure = useSpeleoStore(selectPushFailure);
  if (!failure) return null;

  return (
    <div
      className="rounded border border-border bg-warning-20 text-warning text-sm px-3 py-2 space-y-1"
      role="alert"
      data-testid="speleodb-push-failure-inline"
    >
      <p className="font-medium">{t('speleodb.done.failedHeading')}</p>
      <p>{failure.message}</p>
      <p className="text-xs">{t('speleodb.done.retryHint')}</p>
    </div>
  );
}
