import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '../common/Button';
import { AlertDialog } from './AlertDialog';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import {
  speleoProjectSessionIssuedFor,
  speleoProjectSessionStillCurrent,
  useSpeleoStore,
} from '../../store/speleoStore';
import { useRevertRevision } from '../../hooks/useRevertRevision';
import { SpeleoPushFailureNotice } from './SpeleoPushFailureNotice';

/**
 * Names this flow to `openDumpPicker`, so its own re-raise is read as a continuation
 * of the question it is still suspended on, and anyone else's raise is read as the
 * displacement it is.
 */
const DUMP_PICKER_OWNER = 'speleodb-revert';

/**
 * What a press asks `useRevertRevision` for.
 * - `revert`: the whole composite - take the lock, check the revision out, upload it.
 * - `discard`: the same, after the armed confirm, waiving the un-pushed-edits guard.
 * - `resume-upload`: ONLY the upload, against the copy this dialog already restored
 *   and the lock it is still holding (see `RevertRevisionArgs.resumeAtPush`).
 */
type RunMode = 'revert' | 'discard' | 'resume-upload';

/**
 * Revert-to-revision confirm, opened from the revision history of a READ-ONLY
 * project with `{ projectId, projectName, hexsha, shortSha, returnTo?, returnData? }`.
 *
 * Where Restore hands you an un-pushed working copy and expects you to push it,
 * Revert is the whole round trip: take the write lock, check the revision out,
 * upload it, release the lock again. The project is read-only when it finishes,
 * exactly as it was when it started - which is what makes it the right offer for
 * someone who never held the lock and does not want to.
 *
 * The destructive route (replacing un-pushed local work) reuses `SpeleoRestoreDialog`'s
 * armed two-press shape, including parking focus on Cancel the moment it appears.
 */
export default function SpeleoRevertDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const openDialog = useUIStore((s) => s.openDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  const isDirty = useProjectStore((s) => s.isDirty);
  const revertRevision = useRevertRevision();
  const openDumpPicker = useSpeleoStore((s) => s.openDumpPicker);

  const projectId = typeof dialogData.projectId === 'string' ? dialogData.projectId : '';
  const projectName = typeof dialogData.projectName === 'string' ? dialogData.projectName : '';
  const hexsha = typeof dialogData.hexsha === 'string' ? dialogData.hexsha : '';
  const shortSha =
    typeof dialogData.shortSha === 'string' ? dialogData.shortSha : hexsha.slice(0, 8);
  // The revision history lives INSIDE another dialog, and the single `activeDialog`
  // slot means opening this one evicted it. Put the user back where they pressed the
  // button rather than on a bare map - and back on the same TAB, which needs the
  // data to reopen with: `openDialog` defaults its second argument to `{}`, so
  // reopening by id alone lands on the host dialog's default tab.
  const returnTo = typeof dialogData.returnTo === 'string' ? dialogData.returnTo : null;
  const returnData =
    typeof dialogData.returnData === 'object' && dialogData.returnData !== null
      ? (dialogData.returnData as Record<string, unknown>)
      : undefined;

  const [busy, setBusy] = useState(false);
  const [step, setStep] = useState<'locking' | 'restoring' | 'pushing' | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** [#258] The composite is parked on the file picker - suspended, not failed. */
  const [awaitingChoice, setAwaitingChoice] = useState(false);
  // Set when the revert cannot proceed without replacing un-pushed local work -
  // either refused up front (before any lock was taken) or by the backend mid-flight.
  const [needsDiscard, setNeedsDiscard] = useState(false);
  // [#576 W55 NF-W53-1] That refusal was over a copy that could not be read
  // (`state_unknown`), not one read dirty: same route, honest wording.
  const [copyUnreadable, setCopyUnreadable] = useState(false);
  const [discardArmed, setDiscardArmed] = useState(false);
  // True when the failure left us holding a lock we did not have before, so the
  // dialog can say so instead of leaving the user to discover it from the toolbar.
  const [lockHeld, setLockHeld] = useState(false);
  // The upload - and ONLY the upload - is outstanding: the revision is already in the
  // working copy and the write lock is still ours, so everything except the last round
  // trip is done. That earns its own primary control, which re-issues the upload ALONE
  // (`resume-upload`). Re-running the whole revert from the top would instead hit the
  // pre-flight un-pushed-edits refusal and offer to "replace my changes" - where "my
  // changes" would be the user's own revert - and, on the attempt whose upload really
  // landed, would re-enter the acquire's strict reconcile and raise a divergence
  // against that very commit.
  const [retryUpload, setRetryUpload] = useState(false);
  const [message, setMessage] = useState(() =>
    t('speleodb.revert.commitMessage', { sha: hexsha.slice(0, 8) }),
  );

  // React reconciles the swapped-in discard control into the same slot as the primary
  // Revert button, so it inherits that button's FOCUS - which would let two more
  // Enters (arm, then confirm) replace un-pushed survey work the user never looked at.
  // Park focus on Cancel the moment the destructive route appears.
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (needsDiscard) cancelRef.current?.focus();
  }, [needsDiscard]);

  const dismiss = () => {
    if (returnTo) openDialog(returnTo, returnData);
    else closeDialog();
  };

  const run = async (mode: RunMode, entryPath?: string) => {
    if (!projectId || !hexsha) return;
    setBusy(true);
    setError(null);
    setAwaitingChoice(false);
    // Cleared for the duration of the attempt: this run either fails at the upload
    // again (and re-arms it below) or gets past that step, in which case the retry
    // control must not linger over an outcome it no longer describes.
    setRetryUpload(false);
    // Captured BEFORE the composite runs; the ambiguity arm checks it.
    const issuedFor = speleoProjectSessionIssuedFor(projectId);
    const outcome = await revertRevision({
      projectId,
      projectName,
      hexsha,
      shortSha,
      message: message.trim() || null,
      discardLocal: mode === 'discard',
      resumeAtPush: mode === 'resume-upload',
      onStep: setStep,
      // Conditional spread: `exactOptionalPropertyTypes` rejects an explicit
      // `undefined` here, the same reason `PublishArgs` spells `onStep` out.
      ...(entryPath ? { entryPath } : {}),
    });
    setStep(null);
    // The restore LANDED, so the destructive question has been asked AND answered:
    // disarm the discard route before reporting where we got to. `needsDiscard` used
    // to stay true for the rest of the dialog's life, and the render below tests it
    // FIRST - so every outcome past the restore was shown the discard control, and
    // the upload-only retry the push arms below could never appear. Worse, the
    // control on offer was the still-ARMED confirm: one press re-ran the whole
    // composite under the discard policy, re-restoring over anything edited since
    // and, on the attempt whose upload actually landed, re-entering the acquire's
    // reconcile - which cannot recognise our own commit, so it releases the lock and
    // prompts about a conflict with ourselves. Consent to replace un-pushed work is
    // spent once it has been acted on; a second destructive press has to be asked
    // for again from the top.
    const restoreLanded = () => {
      setNeedsDiscard(false);
      setDiscardArmed(false);
    };
    switch (outcome.kind) {
      case 'reverted':
        dismiss();
        return;
      case 'awaiting_dump_choice':
        // NOT a failure: the write lock is already back, nothing was written, and the
        // composite is suspended on a question. So no `error` is set - the red line
        // below would be a lie - and `busy` deliberately STAYS true, which keeps the
        // plain Revert control inert so a second composite cannot be armed behind the
        // picker. Answering re-runs the whole thing, which is what re-acquires the
        // lock; cancelling hands the dialog back exactly as it was.
        // [#258 review] Only for a session that can still answer it - the probe behind
        // this outcome is an unbounded download, and a concurrent 401 can tear the
        // surface down while it is out. Without the picker there is nothing to suspend
        // on, so `busy` is released rather than deliberately held.
        if (!speleoProjectSessionStillCurrent(issuedFor)) {
          setBusy(false);
          return;
        }
        setAwaitingChoice(true);
        openDumpPicker(
          outcome.candidates,
          'stalx',
          (chosen) => run(mode, chosen),
          () => {
            setAwaitingChoice(false);
            setBusy(false);
          },
          // This flow suspends on the answer - `busy` stays true across it and the
          // cancel above is the only thing that releases it - so it names itself. A
          // re-raise from its own retry is a continuation and must NOT fire that
          // cancel; a picker raised by anything else displaces it and must.
          DUMP_PICKER_OWNER,
        );
        return;
      case 'diverged':
        // The remote moved on: the divergence dialog has already taken the dialog
        // slot from us. Touching it here would tear down the prompt that replaced us.
        return;
      case 'project_changed':
        // A different project is open than the one this dialog is about, so every
        // control on it now points somewhere it should not - including the armed
        // discard, which would be spent against the wrong survey. Nothing was taken
        // and nothing was written, so there is nothing to unwind and nothing to
        // report; just leave, and by the same route Cancel would take.
        dismiss();
        return;
      case 'needs_discard':
        setNeedsDiscard(true);
        setCopyUnreadable(outcome.stateUnknown);
        setLockHeld(outcome.lockHeld);
        setBusy(false);
        return;
      case 'unsaved_local':
        // Refused, NOT offered as a discard: un-saved is not un-pushed. The armed
        // discard waives the SpeleoDB guard and saves nothing, so routing here would
        // destroy work the user still has a one-keystroke way to keep, under copy
        // ("changes that are not on SpeleoDB") that does not describe it. Nothing was
        // taken and nothing was changed, so the plain Revert stays the primary: save,
        // come back, press it again.
        setError(t('speleodb.revert.errors.unsavedLocal'));
        setBusy(false);
        return;
      case 'locked_by':
        setError(
          t('speleodb.revert.errors.locked', {
            email: outcome.email ?? t('speleodb.editBar.someone'),
          }),
        );
        setBusy(false);
        return;
      case 'lock_failed':
        setError(t('speleodb.revert.errors.lockFailed'));
        setBusy(false);
        return;
      case 'workspace_read_only':
        // Refused before anything was taken (#576 RV4-FE-SP4): Plan and Export are
        // read-only, and the remedy is the workspace switch, not a retry here.
        setError(t('editGate.workspaceReadOnlyNotice'));
        setBusy(false);
        return;
      case 'restore_failed':
        setError(
          `${outcome.message}. ${
            outcome.lockHeld
              ? t('speleodb.revert.errors.restoreFailedLockHeld')
              : t('speleodb.revert.errors.restoreFailed')
          }`,
        );
        setBusy(false);
        return;
      case 'push_failed':
        // The revision IS in the working copy and the lock IS still held, so the
        // only thing left to do is upload again. The #260 shell banner's Retry does
        // exactly that, but it is behind this dialog's own full-viewport scrim -
        // dimmed and unclickable - so this dialog has to carry the retry itself.
        // `SpeleoPushFailureNotice` below reports WHY the upload failed.
        restoreLanded();
        setRetryUpload(true);
        setError(t('speleodb.revert.errors.pushFailed', { sha: shortSha }));
        setBusy(false);
        return;
      case 'not_editing':
        // Same dead end, different footing: the session was dropped mid-flight, so
        // there is no held lock and the banner's Retry cannot work. Saying "you are
        // holding the write lock" here - as pushFailed does - would send the user
        // at a button that fails. The restore still landed, so the discard route is
        // spent here too - and leaving it armed would offer a re-run of the whole
        // composite as the way out of a dropped session, which cannot even acquire.
        restoreLanded();
        setError(t('speleodb.revert.errors.notEditing', { sha: shortSha }));
        setBusy(false);
        return;
      case 'push_unconfirmed':
        // Same footing as `push_failed`, one step further along: the upload went out
        // but came back unconfirmed, and the backend KEEPS both the lock and the dirty
        // bit on that arm expressly so a retry can recover by ancestry - a landed
        // commit is then adopted, a lost one re-uploaded. Re-issuing the upload is
        // therefore the whole recovery, and it is what every other surface offers for
        // this state ("click Done again to retry"). The composite is never the right
        // answer here: it would replace a tree that is already correct, and take the
        // one route - the acquire's reconcile - with no way to recognise our own
        // landed commit.
        restoreLanded();
        setRetryUpload(true);
        setError(t('speleodb.revert.errors.pushUnconfirmed'));
        setBusy(false);
        return;
    }
  };

  return (
    <AlertDialog
      open
      title={t('speleodb.revert.title', { sha: shortSha })}
      onClose={() => {
        if (!busy) dismiss();
      }}
      data-testid="speleodb-revert-dialog"
      actions={
        <>
          {/* Once the destructive route is on offer, Cancel is the safe landing spot
              for the focus heuristic and for a stray Enter. */}
          <Button
            ref={cancelRef}
            onClick={dismiss}
            disabled={busy}
            {...(needsDiscard ? { 'data-primary-action': '' } : {})}
            data-testid="speleodb-revert-cancel"
          >
            {t('common.actions.cancel')}
          </Button>
          {needsDiscard ? (
            // A DISTINCT control, rendered only after the refusal, that still needs its
            // own arming press. The plain retry is gone: it would be refused again.
            <Button
              variant="danger"
              onClick={() => {
                if (!discardArmed) {
                  setDiscardArmed(true);
                  return;
                }
                void run('discard');
              }}
              disabled={busy}
              data-testid={
                discardArmed
                  ? 'speleodb-revert-discard-confirm-btn'
                  : 'speleodb-revert-discard-offer-btn'
              }
            >
              {busy
                ? t('speleodb.revert.reverting')
                : discardArmed
                  ? t('speleodb.revert.discardConfirm')
                  : t('speleodb.revert.discardOffer')}
            </Button>
          ) : retryUpload ? (
            // Re-issue the upload and NOTHING else: `resume-upload` skips the acquire
            // and the restore, because both already succeeded and their results are
            // still standing - the lock is ours and the revision is in the working
            // copy. Re-running them would re-download and re-swap a tree that is
            // already correct, and would put the one attempt whose upload actually
            // landed through the acquire's reconcile, which cannot tell that commit
            // is ours and so releases the lock to prompt about a conflict with
            // ourselves. Not `danger` - the destructive part already happened and was
            // consented to.
            <Button
              variant="primary"
              onClick={() => void run('resume-upload')}
              disabled={busy}
              data-primary-action=""
              data-testid="speleodb-revert-retry-upload-btn"
            >
              {busy ? t('speleodb.revert.reverting') : t('speleodb.revert.retryUpload')}
            </Button>
          ) : (
            <Button
              variant="danger"
              onClick={() => void run('revert')}
              disabled={busy}
              data-primary-action=""
              data-testid="speleodb-revert-confirm-btn"
            >
              {busy ? t('speleodb.revert.reverting') : t('speleodb.revert.confirmButton')}
            </Button>
          )}
        </>
      }
    >
      <div className="space-y-2 text-sm text-text-secondary">
        <p>{t('speleodb.revert.body', { sha: shortSha, name: projectName })}</p>
        <p>{t('speleodb.revert.consequence')}</p>
        <div>
          <label className="block mb-1" htmlFor="speleodb-revert-message">
            {t('speleodb.revert.messageLabel')}
          </label>
          <textarea
            id="speleodb-revert-message"
            value={message}
            onChange={(e) => setMessage(e.target.value)}
            rows={2}
            disabled={busy}
            className="w-full px-2 py-1 bg-background border border-border rounded text-sm"
            data-testid="speleodb-revert-message"
          />
        </div>
        {isDirty && (
          <p className="text-warning" data-testid="speleodb-revert-unsaved-warning">
            {t('speleodb.revert.unsavedWarning')}
          </p>
        )}
        {needsDiscard && (
          <p
            className="rounded border border-error bg-error-20 text-error px-3 py-2"
            role="alert"
            data-testid="speleodb-revert-discard-warning"
          >
            {copyUnreadable
              ? t('speleodb.revert.unreadableWarning')
              : t('speleodb.revert.unpushedWarning')}
          </p>
        )}
        {needsDiscard && lockHeld && (
          <p className="text-warning" data-testid="speleodb-revert-lock-held">
            {copyUnreadable
              ? t('speleodb.revert.errors.lockStillHeldUnreadable')
              : t('speleodb.revert.errors.lockStillHeld')}
          </p>
        )}
        {/* A revert is three server round trips, so name the one in flight - a single
            "Reverting…" button label leaves a stalled acquire indistinguishable from a
            stalled upload, and only one of those has left a lock behind. */}
        {step && (
          <p className="text-text-secondary" data-testid="speleodb-revert-step">
            {t(`speleodb.revert.steps.${step}`, { sha: shortSha })}
          </p>
        )}
        {/* Neutral, not `text-error`: the lock is back, nothing was written, and the
            revert is waiting on the user rather than on the server. Rendering this as
            a failure is what the first cut of #258 did, and it told the user the
            revert had died when it had merely asked them something. */}
        {awaitingChoice && (
          <p className="text-text-secondary" data-testid="speleodb-revert-awaiting-choice">
            {t('speleodb.revert.awaitingChoice', { sha: shortSha })}
          </p>
        )}
        {error && (
          <p className="text-error" data-testid="speleodb-revert-error">
            {error}
          </p>
        )}
        {/* #260: a failed upload leaves this dialog open, and the shell banner that
            carries the server's reason sits behind `AlertDialog`'s full-viewport
            scrim - dimmed and unclickable. Render the same line here, above the
            button that retries it, exactly as the done / close / quit dialogs do.
            It renders nothing unless there IS a push failure, so the other failure
            kinds are unaffected. */}
        <SpeleoPushFailureNotice />
      </div>
    </AlertDialog>
  );
}
