import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { DialogWrapper } from '../Dialogs/DialogWrapper';
import { Button } from '../common/Button';
import { invokeIpc } from '../../utils/ipc';
import { useSpeleoCacheStore } from '../../store/speleoCacheStore';
import { useSpeleoOperationBusy } from '../../hooks/useSpeleoOperationBusy';
import { cachedCopyAtRisk } from '../../utils/speleoCachedCopies';
import type { CommandResult, SpeleoCacheOwnership, SpeleoCachedProject } from '../../types/ipc.generated';

/**
 * What happens to the project's editing lock. The backend hands back this
 * account's lock (holder-checked) after removing a copy of its own; signed out, or
 * for another account's copy, nothing is asked and a held lock stays until the next
 * sign-in's held-lock prompt or the website.
 */
const LOCK_NOTE: Record<SpeleoCacheOwnership, string> = {
  mine: 'speleodb.cachedCopies.remove.lockHandedBack',
  unknown: 'speleodb.cachedCopies.remove.lockHandedBack',
  signed_out: 'speleodb.cachedCopies.remove.lockStaysSignedOut',
  other_account: 'speleodb.cachedCopies.remove.lockStaysOtherAccount',
};

/**
 * In-app confirmation for removing the copy of a SpeleoDB project this computer
 * downloaded. The project stays on SpeleoDB.
 *
 * A copy with changes that were never uploaded (or whose state could not be read)
 * gets a warning and a confirm button that names the loss; only that confirmation
 * sends `discardUnpushed`, and the backend refuses at-risk copies without it. The
 * copy's state is re-read when the dialog opens, and confirming waits for that
 * read, so the choice is always made against the copy as it is now. It also says
 * what becomes of the project's editing lock (see `LOCK_NOTE`).
 */
export function RemoveCachedCopyDialog({
  copy,
  onClose,
  onRemoved,
}: {
  copy: SpeleoCachedProject;
  onClose: () => void;
  /** Called after a successful removal, with any backend warnings. */
  onRemoved: (copy: SpeleoCachedProject, warnings: string[]) => void;
}) {
  const { t } = useTranslation();
  const refresh = useSpeleoCacheStore((s) => s.refresh);
  const latest = useSpeleoCacheStore((s) => s.copies.find((c) => c.project_id === copy.project_id));
  const current = latest ?? copy;
  const [checked, setChecked] = useState(false);
  const speleoBusy = useSpeleoOperationBusy();
  const [removing, setRemoving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    void refresh().finally(() => {
      if (live) setChecked(true);
    });
    return () => {
      live = false;
    };
  }, [refresh]);

  const atRisk = cachedCopyAtRisk(current);
  const handleConfirm = async () => {
    setRemoving(true);
    setError(null);
    try {
      const result = await invokeIpc<CommandResult>(
        'speleodb_remove_cached_copy',
        { projectId: current.project_id, discardUnpushed: atRisk },
        { purpose: 'remove a downloaded SpeleoDB copy (user confirmed)' },
      );
      if (result?.success) {
        onRemoved(current, (result.warnings ?? []).map((w) => w.message));
        return;
      }
      setError(result?.error ?? t('speleodb.cachedCopies.remove.failed'));
      void refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : t('speleodb.cachedCopies.remove.failed'));
    } finally {
      setRemoving(false);
    }
  };

  return (
    <DialogWrapper
      title={t('speleodb.cachedCopies.remove.title')}
      onClose={() => { if (!removing) onClose(); }}
      size="md"
      data-testid="speleodb-remove-copy-dialog"
    >
      <div className="space-y-4">
        <p className="text-text-secondary">
          {t('speleodb.cachedCopies.remove.body', { name: current.project_name })}
        </p>
        {atRisk && (
          <div
            className="p-3 bg-warning-10 border border-warning-20 rounded text-warning-text text-sm"
            data-testid="speleodb-remove-copy-warning"
          >
            {t(current.state_unknown
              ? 'speleodb.cachedCopies.remove.unknown'
              : 'speleodb.cachedCopies.remove.unpushed')}
          </div>
        )}
        <p className="text-text-secondary text-sm" data-testid="speleodb-remove-copy-lock">
          {t(LOCK_NOTE[current.ownership])}
        </p>
        {error && (
          <div className="text-error text-sm" data-testid="speleodb-remove-copy-error">
            {error}
          </div>
        )}
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={onClose} disabled={removing} data-testid="speleodb-remove-copy-cancel">
            {t('speleodb.cachedCopies.remove.cancel')}
          </Button>
          <Button
            variant="danger"
            onClick={() => void handleConfirm()}
            // Another lock, upload or account call must not overlap the lock hand-back.
            disabled={removing || !checked || speleoBusy}
            data-testid="speleodb-remove-copy-confirm"
          >
            {removing
              ? t('speleodb.cachedCopies.remove.removing')
              : t(atRisk
                ? 'speleodb.cachedCopies.remove.confirmLosingChanges'
                : 'speleodb.cachedCopies.remove.confirm')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
