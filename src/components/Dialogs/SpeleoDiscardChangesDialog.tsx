import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { isUnwoundNotEdited, useSpeleoStore } from '../../store/speleoStore';
import { useSpeleoActions } from '../../hooks/useSpeleoActions';
import { getErrorMessage } from '../../utils/ipc';
import { holdsWriteLock } from '../../utils/quit';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { TOAST_KEYS } from '../../utils/toastKeys';

/**
 * [#303] Confirmation for the explicit "Discard changes" action.
 *
 * Throwing the local copy away is one decision; what happens to the write lock afterwards
 * is a separate one, and neither answer is the odd one out. Someone resetting to start
 * again wants to KEEP editing; someone abandoning a session wants to hand the project back
 * so a colleague can pick it up. Folding the two together - as an action called "discard
 * and lock" did - forces the second user to discard and then go hunting for Release.
 *
 * The wording branches on whether there is anything of the user's to lose. Telling someone
 * "your changes will be lost" when all they did was step back through history is how the
 * close prompt got this wrong in the first place, just pointed the other way.
 */
export default function SpeleoDiscardChangesDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const openDialog = useUIStore((s) => s.openDialog);
  const showToast = useUIStore((s) => s.showToast);
  const link = useSpeleoStore((s) => s.link);
  const { discardChanges } = useSpeleoActions();

  const unwound = isUnwoundNotEdited(link);
  // [#303] Whether the lock question below is even ours to answer. `holdsWriteLock` is
  // false for a #213 force-editor, who reaches this dialog dirty by design.
  const holdsLock = holdsWriteLock(link);
  const [busy, setBusy] = useState(false);

  const run = async (keepLock: boolean) => {
    setBusy(true);
    try {
      // [#303] `acknowledged` is this dialog reporting that it put the loss in front of
      // the user; the backend refuses without it. Not a second guess at the decision - it
      // is what stops any OTHER route reaching a verb that destroys un-pushed work.
      const outcome = await discardChanges(keepLock);
      if (outcome === 'editing' || outcome === 'released') {
        showToast(
          outcome === 'editing'
            ? t('speleodb.discardChanges.doneKeptToast')
            : t('speleodb.discardChanges.doneReleasedToast'),
          TOAST_KEYS.SPELEODB_DISCARDED,
          'success',
        );
      }
      // [#576 RV6b-F1] Refused in the read-only Plan/Export workspace (the notice is
      // up): close as on Cancel - nothing was discarded, so the dialog did not complete.
      if (outcome !== 'workspace_read_only') trackDialogComplete('speleodb-discard-changes');
      closeDialog();
    } catch (e) {
      openDialog('error', {
        title: t('speleodb.discardChanges.failedTitle'),
        message: getErrorMessage(e, t('speleodb.discardChanges.failedMessage')),
      });
      setBusy(false);
    }
  };

  return (
    <DialogWrapper
      title={t('speleodb.discardChanges.title')}
      onClose={busy ? () => {} : closeDialog}
      size="md"
      data-testid="speleodb-discard-changes-dialog"
    >
      <div className="space-y-4">
        <p className="text-sm text-text-secondary" data-testid="speleodb-discard-changes-body">
          {unwound
            ? t('speleodb.discardChanges.bodyUnwound')
            : t('speleodb.discardChanges.bodyEdited')}
        </p>
        {/* Only ask the lock question when there are two answers to pick between. */}
        {holdsLock && (
          <p className="text-xs text-text-secondary">{t('speleodb.discardChanges.lockQuestion')}</p>
        )}

        <div className="flex justify-end gap-2 pt-2 border-t border-border">
          <Button
            onClick={closeDialog}
            disabled={busy}
            data-testid="speleodb-discard-changes-cancel"
          >
            {t('common.actions.cancel')}
          </Button>
          {/* [#303] Only offer to hand back a lock this session actually holds. A #213
              force-editor is dirty by design and holds none, so "release" would reach
              `speleodb_release_current_lock` - the ADMIN-capable stuck-lock recovery verb -
              and either fail outright or force-release the colleague's lock as a side
              effect of the user discarding their own work. Discarding itself stays
              available without the lock: that is the whole point of the escape hatch. */}
          {holdsLock && (
            <Button
              variant="secondary"
              onClick={() => void run(false)}
              disabled={busy}
              data-testid="speleodb-discard-changes-release"
            >
              {t('speleodb.discardChanges.discardAndRelease')}
            </Button>
          )}
          {/* Primary because it is the reversible one: still holding the lock, the user can
              release at any time, whereas a released lock is a colleague's to take. */}
          <Button
            variant="primary"
            onClick={() => void run(true)}
            disabled={busy}
            data-primary-action=""
            data-testid="speleodb-discard-changes-keep"
          >
            {busy
              ? t('speleodb.discardChanges.working')
              : t('speleodb.discardChanges.discardAndKeep')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
