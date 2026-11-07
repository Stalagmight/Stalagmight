import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AlertDialog } from './AlertDialog';
import { Button } from '../common/Button';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useRestoreRevision } from '../../hooks/useRestoreRevision';
import { humanizeSpeleoError } from '../../utils/speleoError';
import { projectChangedMessage } from '../../utils/projectChangedNotice';
import { trackDialogComplete } from '../../utils/telemetry';

/**
 * [#303] Confirmation for "Unwind to previous SpeleoDB version" - the one action that
 * fetches a commit's undo history back so the user can keep undoing past a commit
 * boundary.
 *
 * It exists because the action is not a plain undo step: it downloads a revision and
 * REPLACES the working copy. Two shapes, worded differently because they promise
 * different things (`isCurrent`, from `speleodb_unwind_target`):
 *
 * - **the commit the copy already equals** (the first press after a push): nothing
 *   moves. What comes back is the history that produced this state, so the user can
 *   step back through the operations they just pushed.
 * - **an ancestor**: the working copy DOES move back, and a later push lands that older
 *   content as a new forward commit (SpeleoDB exposes no server-side revert).
 *
 * The backend re-checks every precondition (lock held, no force-edit session, an empty
 * undo stack, and any dirt being a previous Unwind's own doing), so this dialog is the
 * user-facing half of the gate, never the whole of it.
 *
 * `discardsUnpushed` is the third thing it has to say. The affordance is offered when the
 * UNDO stack is empty, which is also what undoing everything produces - and that leaves the
 * copy dirty with the undone steps sitting on the REDO stack. The backend used to refuse
 * that outright, so the entry existed only to fail once pressed. Now the same predicate
 * that drives the refusal rides on the unwind target, this dialog states the consequence in
 * red BEFORE the user commits, and confirming reports back that it did (`discardUnpushed`).
 * The warning names the survey changes as well as the history: this copy can also be dirty
 * for reasons no undo explains (a byte-budget trim, a legacy file), and there the swap
 * really does replace content.
 */
export default function SpeleoUnwindDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  // The same two sources the revision-history tab uses for its Restore.
  const projectId = useSpeleoStore((s) => s.remoteProjectId);
  const projectName = useProjectStore((s) => s.project?.name ?? '');
  const restoreRevision = useRestoreRevision();

  const hexsha = typeof dialogData.hexsha === 'string' ? dialogData.hexsha : '';
  const isCurrent = dialogData.isCurrent === true;
  const discardsUnpushed = dialogData.discardsUnpushed === true;
  // [#303] Which way this step walks. One component serves both because the mechanics are
  // identical - a single revision swap that keeps the downloaded history - and only the
  // wording differs; a second component would duplicate the busy/error handling for copy.
  const forward = dialogData.forward === true;
  const isLatest = dialogData.isLatest === true;
  const shortSha = hexsha.slice(0, 8);

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async () => {
    if (!projectId || !hexsha) return;
    setBusy(true);
    setError(null);
    try {
      const applied = await restoreRevision({
        projectId,
        projectName,
        hexsha,
        shortSha,
        restore: true,
        // From the second step onwards the copy is dirty because the PREVIOUS step made
        // it so, and the backend only honours the waiver when its chain pointer proves
        // that. Real un-pushed work with no pointer is refused there, not here.
        discardLocal: true,
        keepUndoHistory: true,
        // Only ever set when the warning above was actually rendered, so the backend's
        // refusal still stands for a copy the user was told nothing about.
        discardUnpushed: discardsUnpushed,
        // Which stack the backend requires empty: undo going back, redo going forward.
        chainForward: forward,
        // The toast would say "restored to revision X. Push to make it the latest",
        // which misreads the action: nothing is being restored FORWARD, the user is
        // stepping backwards through history.
        quiet: true,
      });
      if (!applied) {
        closeDialog();
        return;
      }
      trackDialogComplete('speleodb-unwind');
      closeDialog();
    } catch (e) {
      // [#576 Q-W44-7-D1] Refused because the project changed: the short line instead.
      setError(projectChangedMessage(e) ?? humanizeSpeleoError(e, t('speleodb.unwind.failed')));
      setBusy(false);
    }
  };

  return (
    <AlertDialog
      open
      title={t(forward ? 'speleodb.rewind.title' : 'speleodb.unwind.title')}
      onClose={() => {
        if (!busy) closeDialog();
      }}
      data-testid="speleodb-unwind-dialog"
      actions={
        <>
          <Button onClick={closeDialog} disabled={busy} data-testid="speleodb-unwind-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button
            variant="danger"
            onClick={() => void run()}
            disabled={busy}
            data-primary-action=""
            data-testid="speleodb-unwind-confirm-btn"
          >
            {busy
              ? t('speleodb.unwind.loading')
              : t(forward ? 'speleodb.rewind.confirmButton' : 'speleodb.unwind.confirmButton')}
          </Button>
        </>
      }
    >
      <div className="space-y-2 text-sm text-text-secondary">
        <p data-testid="speleodb-unwind-body">
          {forward
            ? t(isLatest ? 'speleodb.rewind.bodyLatest' : 'speleodb.rewind.bodyNext', {
                sha: shortSha,
              })
            : t(isCurrent ? 'speleodb.unwind.bodyCurrent' : 'speleodb.unwind.bodyAncestor', {
                sha: shortSha,
              })}
        </p>
        <p>
          {forward
            ? t('speleodb.rewind.consequence')
            : t(
                isCurrent
                  ? 'speleodb.unwind.consequenceCurrent'
                  : 'speleodb.unwind.consequenceAncestor',
              )}
        </p>
        {/* Stated on OPEN, not on press: this is the one shape of the action that destroys
            work, and the user has to be able to back out before committing to it. */}
        {discardsUnpushed && (
          <p
            className="text-error font-medium"
            data-testid="speleodb-unwind-discard-warning"
          >
            {t('speleodb.unwind.discardWarning', { sha: shortSha })}
          </p>
        )}
        {error && (
          <p className="text-error" data-testid="speleodb-unwind-error">
            {error}
          </p>
        )}
      </div>
    </AlertDialog>
  );
}
