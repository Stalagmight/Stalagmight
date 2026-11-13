import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useSpeleoActions } from '../../hooks/useSpeleoActions';
import { convertToUnmanagedStalx } from '../../utils/projectActions';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';

/**
 * SpeleoDB conflict / divergence resolution (#198). Reached from the SpeleoDB
 * tab's "Resolve synchronization conflict" button (or any hand-off verb that
 * reported `divergence_needed`) when the local copy and the server have diverged.
 *
 * Three peer resolutions - the framing biases toward preserving local work, the
 * only non-recoverable artifact (the server side is always recoverable from SDB
 * git):
 * - **Keep my local version**: route to the push dialog so the user uploads their
 *   version to resolve it (the push re-fences and, if it still diverges, lands
 *   back here - never a silent clobber).
 * - **Take the SpeleoDB version**: DESTROYS local unpushed edits and resets to the
 *   server tip. This is the only irreversible choice, so it is gated behind a
 *   SECOND in-dialog confirmation (Back / Destroy & continue) - there is no
 *   single-click path to data loss.
 * - **Save my version as a local copy**: keep the locally-edited work by converting
 *   it to a plain unmanaged `.stalx` (Save-As), abandoning the managed copy (a
 *   later Open-from-SpeleoDB re-pulls the clean server tip). This is the
 *   non-destructive escape hatch the take-remote confirmation points at.
 */
export default function SpeleoDivergenceDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const openDialog = useUIStore((s) => s.openDialog);
  const clearDivergence = useSpeleoStore((s) => s.clearDivergence);
  const editState = useSpeleoStore((s) => s.link?.editState ?? 'read-only');
  const { startEditing, takeRemote } = useSpeleoActions();
  const [converting, setConverting] = useState(false);
  const [takingRemote, setTakingRemote] = useState(false);
  // Second-step gate for the destructive Take-remote choice (no single-click path).
  const [confirmingTakeRemote, setConfirmingTakeRemote] = useState(false);
  const busy = converting || takingRemote;

  // Closing without resolving leaves the divergence for later, but must CLEAR the
  // transient flag so the App-level effect doesn't immediately re-open this dialog
  // (fix 3). The button stays crimson while `remoteState === 'conflict'`, so the
  // user can reopen from there.
  const handleCancel = () => {
    clearDivergence();
    closeDialog();
  };

  // Keep local → push your version. While we hold the lock, route to the push
  // dialog. Without the lock, acquire it first (Start editing): if our BASE actually
  // matches the server the lock is granted and the user can then push their version;
  // if it still diverges, startEditing re-reports it and the App effect re-opens this
  // dialog - a real next step, not a dead-end toast.
  const keepLocal = () => {
    clearDivergence();
    if (editState === 'editing') {
      openDialog('speleodb-done', { mode: 'push' });
    } else {
      closeDialog();
      void startEditing();
    }
  };

  const saveCopy = async () => {
    setConverting(true);
    try {
      if (await convertToUnmanagedStalx()) {
        clearDivergence();
        closeDialog();
      }
    } finally {
      setConverting(false);
    }
  };

  // Second step confirmed: discard local edits and reset to the server tip.
  const destroyAndContinue = async () => {
    setTakingRemote(true);
    try {
      if (await takeRemote()) {
        clearDivergence();
        closeDialog();
      }
    } finally {
      setTakingRemote(false);
    }
  };

  if (confirmingTakeRemote) {
    return (
      <DialogWrapper
        title={t('speleodb.divergence.takeRemote.confirm.title')}
        onClose={handleCancel}
        size="md"
        data-testid="speleodb-divergence-dialog"
      >
        <div className="space-y-4">
          <p className="text-sm text-text-primary font-medium" data-testid="speleodb-take-remote-warning">
            {t('speleodb.divergence.takeRemote.confirm.warning')}
          </p>
          <p className="text-xs text-text-secondary">
            {t('speleodb.divergence.takeRemote.confirm.hint')}
          </p>

          <div className="flex justify-end gap-2 pt-2 border-t border-border">
            <Button
              onClick={() => setConfirmingTakeRemote(false)}
              disabled={busy}
              data-testid="speleodb-take-remote-back"
            >
              {t('speleodb.divergence.takeRemote.confirm.back')}
            </Button>
            <Button
              variant="danger"
              onClick={() => void destroyAndContinue()}
              disabled={busy}
              data-testid="speleodb-take-remote-destroy"
            >
              {takingRemote
                ? t('speleodb.divergence.takeRemote.confirm.destroying')
                : t('speleodb.divergence.takeRemote.confirm.destroy')}
            </Button>
          </div>
        </div>
      </DialogWrapper>
    );
  }

  return (
    <DialogWrapper
      title={t('speleodb.divergence.title')}
      onClose={handleCancel}
      size="md"
      data-testid="speleodb-divergence-dialog"
    >
      <div className="space-y-4">
        <p className="text-sm text-text-secondary">{t('speleodb.divergence.description')}</p>

        <div className="space-y-2">
          <button
            type="button"
            onClick={keepLocal}
            disabled={busy}
            className="w-full text-left p-3 rounded border border-border hover:bg-panel disabled:opacity-50"
            data-testid="speleodb-divergence-keep-local"
          >
            <div className="text-sm font-medium text-text-primary">
              {t('speleodb.divergence.keepLocal.label')}
            </div>
            <div className="text-xs text-text-secondary">
              {t('speleodb.divergence.keepLocal.help')}
            </div>
          </button>

          <button
            type="button"
            onClick={() => setConfirmingTakeRemote(true)}
            disabled={busy}
            className="w-full text-left p-3 rounded border border-border hover:bg-panel disabled:opacity-50"
            data-testid="speleodb-divergence-take-remote"
          >
            <div className="text-sm font-medium text-text-primary">
              {t('speleodb.divergence.takeRemote.label')}
            </div>
            <div className="text-xs text-text-secondary">
              {t('speleodb.divergence.takeRemote.help')}
            </div>
          </button>

          <button
            type="button"
            onClick={() => void saveCopy()}
            disabled={busy}
            className="w-full text-left p-3 rounded border border-border hover:bg-panel disabled:opacity-50"
            data-testid="speleodb-divergence-save-copy"
          >
            <div className="text-sm font-medium text-text-primary">
              {t('speleodb.divergence.saveCopy.label')}
            </div>
            <div className="text-xs text-text-secondary">
              {converting
                ? t('speleodb.divergence.saveCopy.converting')
                : t('speleodb.divergence.saveCopy.help')}
            </div>
          </button>
        </div>

        <div className="flex justify-end pt-2 border-t border-border">
          <Button onClick={handleCancel} disabled={busy} data-testid="speleodb-divergence-cancel">
            {t('common.actions.cancel')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
