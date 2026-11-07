import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '../common/Button';
import { AlertDialog } from './AlertDialog';
import { useUIStore } from '../../store/uiStore';
import {
  speleoProjectSessionIssuedFor,
  speleoProjectSessionStillCurrent,
  useSpeleoStore,
} from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useRestoreRevision } from '../../hooks/useRestoreRevision';
import { humanizeSpeleoError, unpushedLocalEdits, dumpCandidates } from '../../utils/speleoError';

/** Names this flow to `openDumpPicker` - see the revert dialog for why. */
const DUMP_PICKER_OWNER = 'speleodb-restore';

/**
 * [#218] Restore-revision confirm. Opened from the Project Settings revision
 * history with `{ projectId, projectName, hexsha, shortSha }`. Carries the
 * disclaimer - a git-FORWARD restore: it replaces the working copy with the chosen
 * revision as UNPUSHED edits on top of the current server state, so the project's
 * history is never rewritten and the restore only becomes the server's latest once
 * you push - plus an unsaved-changes safeguard (a restore discards local edits that
 * have not been saved/pushed). On confirm it runs `speleodb_restore_revision` then
 * reloads the project so the restored tree is what the user sees, dirty and ready to
 * push. Gated upstream on holding the lock (the history Restore button).
 */
export default function SpeleoRestoreDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  const isDirty = useProjectStore((s) => s.isDirty);
  const restoreRevision = useRestoreRevision();
  const openDumpPicker = useSpeleoStore((s) => s.openDumpPicker);

  const projectId = typeof dialogData.projectId === 'string' ? dialogData.projectId : '';
  const projectName = typeof dialogData.projectName === 'string' ? dialogData.projectName : '';
  const hexsha = typeof dialogData.hexsha === 'string' ? dialogData.hexsha : '';
  const shortSha =
    typeof dialogData.shortSha === 'string' ? dialogData.shortSha : hexsha.slice(0, 8);

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Set when the backend refused because the working copy holds un-pushed edits.
  // A restore STAMPS the copy un-pushed itself, so this is the ordinary state of
  // "restore a second revision" or "revert a restore I did not like" - without a
  // way through it, Restore was single-use.
  //
  // It only OFFERS the discard route; it never arms it. The refusal is a purely
  // local read, so the failed first attempt can resolve inside a double-click
  // interval - and this dialog's primary button is the one DialogWrapper/AlertDialog
  // focuses. Letting the refusal re-label that same focused control into "replace my
  // changes" made a second Enter (or the tail of a double click) permanently destroy
  // un-pushed survey work that the user never chose to destroy.
  const [needsDiscard, setNeedsDiscard] = useState(false);
  // [#576 W55 NF-W53-1] The same refusal arrives when the copy is there but could not
  // be read (`state_unknown`: a hot journal after a crash, a lock another program
  // holds). The discard route is the same; only the warning says what is known.
  const [copyUnreadable, setCopyUnreadable] = useState(false);
  // The separate arming step for that route, on its own control - the same
  // arm-then-confirm shape SpeleoUnpushedLocalDialog and SpeleoQuitDialog use for
  // their destructive verbs.
  const [discardArmed, setDiscardArmed] = useState(false);

  // React reconciles the swapped-in discard control into the same slot as the
  // primary Restore button, so it inherits that button's FOCUS - which would let two
  // more Enters (arm, then confirm) run the discard without the user ever looking at
  // it. Park focus on Cancel the moment the destructive route appears, so keyboard
  // repetition is inert and reaching the discard needs a deliberate move.
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (needsDiscard) cancelRef.current?.focus();
  }, [needsDiscard]);

  const run = async (discardLocal: boolean, entryPath?: string) => {
    if (!projectId || !hexsha) return;
    setBusy(true);
    setError(null);
    // Captured BEFORE the request, so the ambiguity catch can tell whether the session
    // that issued it is still the one signed in - and whether the project still is.
    const issuedFor = speleoProjectSessionIssuedFor(projectId);
    try {
      // Restore mode: the backend reopens the swapped tree with editing re-enabled
      // and the hook pulls it into the view. Close the confirm only on success so a
      // failure keeps the inline error visible.
      const applied = await restoreRevision({
        projectId,
        projectName,
        hexsha,
        shortSha,
        restore: true,
        discardLocal,
        ...(entryPath ? { entryPath } : {}),
      });
      if (!applied) {
        closeDialog();
        return;
      }
      closeDialog();
    } catch (e) {
      // [#258] The write lock is held for this whole call and is NOT touched here, so
      // unlike the revert composite the retry is just this attempt again. Cancelling
      // returns the dialog to its opening state with the lock still held - the user
      // is still in an edit session, and the edit bar still says so.
      const candidates = dumpCandidates(e);
      // [#258 review] Same issuing-session guard the open verbs use: this download is
      // unbounded, and a concurrent 401 can tear the surface down while it is out. Plus
      // the project one, which the open verbs must NOT use - a restore binds to whatever
      // is open, so a picker raised for a project that is no longer it offers a choice
      // the backend will refuse.
      if (candidates && speleoProjectSessionStillCurrent(issuedFor)) {
        openDumpPicker(
          candidates,
          'stalx',
          (chosen) => run(discardLocal, chosen),
          () => setBusy(false),
          // Suspends on the answer like the revert does, so it names itself for the
          // same reason: its own re-raise must not fire the cancel that releases it,
          // and anything else's raise must.
          DUMP_PICKER_OWNER,
        );
        return;
      }
      // A refusal over un-pushed edits is not a dead end: arm the explicit
      // discard step instead of showing the raw "local edits never pushed" line
      // about changes this dialog is already about to replace.
      const refusal = unpushedLocalEdits(e);
      if (refusal) {
        setNeedsDiscard(true);
        setCopyUnreadable(refusal.state_unknown === true);
        setError(null);
      } else {
        setError(humanizeSpeleoError(e, t('speleodb.restore.failed')));
      }
      setBusy(false);
    }
  };

  return (
    <AlertDialog
      open
      title={t('speleodb.restore.title', { sha: shortSha })}
      onClose={() => {
        if (!busy) closeDialog();
      }}
      data-testid="speleodb-restore-dialog"
      actions={
        <>
          {/* Once the destructive route is on offer, Cancel is the safe landing spot
              for the focus heuristic and for a stray Enter. */}
          <Button
            ref={cancelRef}
            onClick={closeDialog}
            disabled={busy}
            {...(needsDiscard ? { 'data-primary-action': '' } : {})}
            data-testid="speleodb-restore-cancel"
          >
            {t('common.actions.cancel')}
          </Button>
          {needsDiscard ? (
            // A DISTINCT control, rendered only after the refusal, that still needs
            // its own arming press before it can discard anything. The plain retry
            // is deliberately gone: `run(false)` would just be refused again.
            <Button
              variant="danger"
              onClick={() => {
                if (!discardArmed) {
                  setDiscardArmed(true);
                  return;
                }
                void run(true);
              }}
              disabled={busy}
              data-testid={
                discardArmed
                  ? 'speleodb-restore-discard-confirm-btn'
                  : 'speleodb-restore-discard-offer-btn'
              }
            >
              {busy
                ? t('speleodb.restore.restoring')
                : discardArmed
                  ? t('speleodb.restore.discardConfirm')
                  : t('speleodb.restore.discardOffer')}
            </Button>
          ) : (
            <Button
              variant="danger"
              onClick={() => void run(false)}
              disabled={busy}
              data-primary-action=""
              data-testid="speleodb-restore-confirm-btn"
            >
              {busy ? t('speleodb.restore.restoring') : t('speleodb.restore.confirmButton')}
            </Button>
          )}
        </>
      }
    >
      <div className="space-y-2 text-sm text-text-secondary">
        <p>{t('speleodb.restore.body', { sha: shortSha, name: projectName })}</p>
        <p>{t('speleodb.restore.consequence')}</p>
        {isDirty && (
          <p className="text-warning" data-testid="speleodb-restore-unsaved-warning">
            {t('speleodb.restore.unsavedWarning')}
          </p>
        )}
        {needsDiscard && (
          <p
            className="rounded border border-error bg-error-20 text-error px-3 py-2"
            role="alert"
            data-testid="speleodb-restore-discard-warning"
          >
            {copyUnreadable
              ? t('speleodb.restore.unreadableWarning')
              : t('speleodb.restore.discardWarning')}
          </p>
        )}
        {error && (
          <p className="text-error" data-testid="speleodb-restore-error">
            {error}
          </p>
        )}
      </div>
    </AlertDialog>
  );
}
