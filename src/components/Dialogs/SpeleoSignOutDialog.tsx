import { useTranslation } from 'react-i18next';
import { useSpeleoStore } from '../../store/speleoStore';
import { useScreenOccupant } from '../../hooks/useScreenOccupant';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';

/**
 * [#576 SIGNOUT-PROMPT-D1] "You hold the lock on CaveA. Release it before signing
 * out?" - asked before a sign-out while this session holds SpeleoDB write locks:
 * a logout, or a sign-in as another account (or on another server), which signs
 * the live session out first. Never shown when no lock is held.
 *
 * - Release and sign out: the sign-out hands the locks back, as it always did; a
 *   release SpeleoDB refuses still ends in the "still locked" warning.
 * - Keep locks and sign out: the sign-out sends no release, and the locks stay
 *   held on SpeleoDB.
 * - Cancel (and Escape, X, the backdrop): nothing happens, and what Settings
 *   staged stays staged.
 *
 * [#576 SIGNOUT-PROMPT-D2] A lock over edits not pushed to SpeleoDB yet is kept by
 * every sign-out, so it is named on a line of its own and offered no release. When
 * such locks are all there is, the prompt only informs: Sign out or Cancel.
 *
 * An INDEPENDENT top-layer overlay (`signOutPrompt`), not the shared `activeDialog`
 * slot, for the storage prompt's reason: the sign-out is applied from the Settings
 * account panel, which must survive a Cancel with everything it staged.
 */
export default function SpeleoSignOutDialog() {
  const { t } = useTranslation();
  // Mounted only while the prompt is open (App.tsx), so always occupied.
  useScreenOccupant(true);
  const prompt = useSpeleoStore((s) => s.signOutPrompt);
  const answer = useSpeleoStore((s) => s.answerSignOutPrompt);
  if (!prompt) return null;

  const names = (locks: { project_name: string }[]) => locks.map((lock) => lock.project_name).join(', ');
  const count = prompt.releasable.length;
  const offersRelease = count > 0;
  const switching = prompt.kind === 'switch';

  return (
    <DialogWrapper
      title={switching ? t('speleodb.signOutLocks.titleSwitch') : t('speleodb.signOutLocks.title')}
      onClose={() => answer('cancel')}
      size="lg"
      data-testid="speleodb-sign-out-dialog"
    >
      <div className="space-y-4">
        {switching && (
          <p className="text-sm text-text-secondary">{t('speleodb.signOutLocks.switchNote')}</p>
        )}
        {offersRelease && (
          <p className="text-sm text-text-primary" data-testid="speleodb-sign-out-message">
            {t('speleodb.signOutLocks.message', { count, projects: names(prompt.releasable) })}
          </p>
        )}
        {prompt.unpushed.length > 0 && (
          <p className="text-sm text-text-primary" data-testid="speleodb-sign-out-unpushed">
            {t('speleodb.signOutLocks.unpushed', {
              count: prompt.unpushed.length,
              projects: names(prompt.unpushed),
            })}
          </p>
        )}
        {offersRelease && (
          <p className="text-xs text-text-secondary">{t('speleodb.signOutLocks.keepHint', { count })}</p>
        )}
        <div className="flex flex-wrap justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={() => answer('cancel')} data-testid="speleodb-sign-out-cancel">
            {t('common.actions.cancel')}
          </Button>
          {offersRelease ? (
            <>
              <Button variant="secondary" onClick={() => answer('keep')} data-testid="speleodb-sign-out-keep">
                {t('speleodb.signOutLocks.keep', { count })}
              </Button>
              {/* Primary + [data-primary-action]: a stray Enter hands the locks back,
                  the sign-out's behaviour before it asked. */}
              <Button
                variant="primary"
                onClick={() => answer('release')}
                data-primary-action=""
                data-testid="speleodb-sign-out-release"
              >
                {t('speleodb.signOutLocks.release')}
              </Button>
            </>
          ) : (
            // Nothing to release: the sign-out runs as it always did, and the
            // un-pushed-edits veto keeps those locks (see SignOutChoice).
            <Button
              variant="primary"
              onClick={() => answer('release')}
              data-primary-action=""
              data-testid="speleodb-sign-out-ok"
            >
              {t('speleodb.signOutLocks.signOut')}
            </Button>
          )}
        </div>
      </div>
    </DialogWrapper>
  );
}
