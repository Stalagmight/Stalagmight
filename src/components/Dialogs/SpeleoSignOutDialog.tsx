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

  const count = prompt.locks.length;
  const projects = prompt.locks.map((lock) => lock.project_name).join(', ');
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
        <p className="text-sm text-text-primary" data-testid="speleodb-sign-out-message">
          {t('speleodb.signOutLocks.message', { count, projects })}
        </p>
        <p className="text-xs text-text-secondary">{t('speleodb.signOutLocks.keepHint', { count })}</p>
        <div className="flex flex-wrap justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={() => answer('cancel')} data-testid="speleodb-sign-out-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button variant="secondary" onClick={() => answer('keep')} data-testid="speleodb-sign-out-keep">
            {t('speleodb.signOutLocks.keep', { count })}
          </Button>
          {/* Primary + [data-primary-action]: a stray Enter hands the locks back, the
              sign-out's behaviour before it asked. */}
          <Button
            variant="primary"
            onClick={() => answer('release')}
            data-primary-action=""
            data-testid="speleodb-sign-out-release"
          >
            {t('speleodb.signOutLocks.release')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
