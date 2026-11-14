import { setUploadContinuation } from '../../utils/uploadContinuation';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoActions } from '../../hooks/useSpeleoActions';
import { getErrorMessage } from '../../utils/ipc';
import { closeExitStillCurrent, handBackProject, performQuit, readPendingExit } from '../../utils/quit';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { SpeleoPushFailureNotice } from './SpeleoPushFailureNotice';
import { TOAST_KEYS } from '../../utils/toastKeys';

/**
 * [#303] Raised when a push is attempted on a copy that is dirty ONLY because a chain step
 * walked it back to an older version.
 *
 * The dirty bit answers "does this differ from the server", and every push surface used to
 * read it as "you have work to save". Those agree right up until an Unwind, and then they
 * are opposites: the difference is that the copy is OLDER. Pushing it does not save
 * anyone's work - it records the old survey over the newer one for every collaborator.
 *
 * That is not hypothetical. The close prompt said "You have unpushed changes" and offered
 * "Push, release & close" as its primary action, so looking at history and then closing the
 * project published a revert in two clicks, with a reassurance underneath saying the work
 * was safe.
 *
 * So the choice is put plainly, and the safe way out is the default: discard the older copy
 * and come back at the latest, still holding the lock so the user can carry on. Pushing the
 * old version stays available - it IS how you deliberately roll a project back - but it is
 * named for what it does and it is not the primary.
 *
 * Two of the routes here are a QUIT or a CLOSE mid-flight, and those prompts are gone from
 * uiStore's single dialog slot by the time this renders. So this dialog also owns the exit
 * it interrupted (`pendingExit`): it finishes the quit / close once the push has actually
 * released, and hands the prompt back on every other outcome. Without that, the push landed
 * and the app neither quit nor closed - and Cancel dropped the exit request silently.
 */
export default function SpeleoUnwoundPushDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const openDialog = useUIStore((s) => s.openDialog);
  const showToast = useUIStore((s) => s.showToast);
  const dialogData = useUIStore((s) => s.dialogData);
  const { push, done, discardChanges } = useSpeleoActions();

  const message = typeof dialogData.message === 'string' ? dialogData.message : null;
  const mode = dialogData.mode === 'push' ? 'push' : 'release';
  const [busy, setBusy] = useState(false);

  // [#303] The exit this confirmation interrupted, when it was raised out of one. `done`
  // short-circuits here from the quit and close prompts too, and those prompts are the
  // user's exit request: they are already evicted from uiStore's single dialog slot by
  // the time this renders, so unless this dialog finishes what they started - or hands
  // them back - the request is gone with no trace. `null` for the routes where this
  // dialog IS the whole interaction (the SpeleoDB tab).
  const pendingExit = readPendingExit(dialogData);
  const mounted = useRef(true);
  const project = useProjectStore.getState();
  const identity = { data: dialogData, sessionGeneration: useSpeleoStore.getState().sessionGeneration,
    projectPath: project.project?.file_path, clientOpId: project.loading.clientOpId };
  const owner = useRef(identity);
  if (owner.current.data !== dialogData) owner.current = identity;
  const issuedFor = owner.current;
  useEffect(() => { setBusy(false); }, [dialogData]);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const current = () => mounted.current &&
    useUIStore.getState().activeDialog === 'speleodb-unwound-push' &&
    useUIStore.getState().dialogData === dialogData &&
    useSpeleoStore.getState().sessionGeneration === issuedFor.sessionGeneration &&
    useProjectStore.getState().project?.file_path === issuedFor.projectPath &&
    useProjectStore.getState().loading.clientOpId === issuedFor.clientOpId &&
    (pendingExit?.kind !== 'close' || closeExitStillCurrent(pendingExit));
  const stopIfStale = () => {
    if (current()) return false;
    if (mounted.current && useUIStore.getState().dialogData === dialogData) setBusy(false);
    return true;
  };


  // The prompt that was displaced, recorded by `openExitPrompt` on the way in. Same pair
  // SpeleoQuitDialog reads for its own Cancel - see `openExitPrompt` in utils/quit.
  const cancelReturnsTo =
    typeof dialogData.cancelReturnsTo === 'string' ? dialogData.cancelReturnsTo : null;
  const cancelReturnsData =
    typeof dialogData.cancelReturnsData === 'object' && dialogData.cancelReturnsData !== null
      ? (dialogData.cancelReturnsData as Record<string, unknown>)
      : undefined;

  /**
   * Leave this confirmation with the app still running: put the exit prompt back if one
   * was displaced, otherwise just close. Used by Cancel / X / Escape AND by every outcome
   * that does not end in the exit - a user who asked to quit must never be dropped into a
   * bare view with nothing on screen saying what became of the request.
   */
  const returnToCaller = () => {
    if (cancelReturnsTo) openDialog(cancelReturnsTo, cancelReturnsData);
    else { setUploadContinuation(null); closeDialog(); }
  };

  const discardAndLock = async () => {
    if (stopIfStale()) return;
    setBusy(true);
    try {
      // Keeps the lock: this branch exists so the user can carry on from the latest.
      // [#303] See SpeleoDiscardChangesDialog: the backend refuses an unacknowledged
      // discard, and this dialog's whole body is the acknowledgement.
      const outcome = await discardChanges(true);
      if (stopIfStale()) return;
      if (outcome === 'editing') {
        showToast(t('speleodb.unwoundPush.discardedToast'), TOAST_KEYS.SPELEODB_DISCARDED, 'success');
      }
      trackDialogComplete('speleodb-unwound-push');
      // Not the exit: this branch keeps the lock so the user can carry on editing, and
      // quitting on a button that says so would be a lie. It still has to hand the exit
      // prompt back rather than close over it - the request was to leave, and the state
      // it was raised for has just changed under it.
      returnToCaller();
    } catch (e) {
      if (stopIfStale()) return;
      openDialog('error', {
        title: t('speleodb.unwoundPush.discardFailedTitle'),
        message: getErrorMessage(e, t('speleodb.unwoundPush.discardFailedMessage')),
      });
      setBusy(false);
    }
  };

  const pushAnyway = async () => {
    if (stopIfStale()) return;
    setBusy(true);
    if (mode === 'push') {
      const resolved = await push(message, { acceptOlderVersion: true });
      if (stopIfStale()) return;
      if (resolved?.kind === 'pushed') {
        trackDialogComplete('speleodb-unwound-push');
        setUploadContinuation(null);
        closeDialog();
      }
      else setBusy(false);
      return;
    }
    // The acknowledgement the guard in `useSpeleoActions.done` is waiting for - without it
    // this would raise this same dialog again.
    if (!pendingExit) {
      const resolved = await done(message, { acceptOlderVersion: true, isCurrent: current });
      if (stopIfStale()) return;
      if (resolved?.kind === 'released') {
        trackDialogComplete('speleodb-unwound-push');
        setUploadContinuation(null);
        closeDialog();
      } else {
        // The lock is still held and the push did not finish. Keep the confirmation,
        // its commit message, and its retry button alive just like SpeleoDoneDialog.
        setBusy(false);
      }
      return;
    }
    const resolved = await done(message, { acceptOlderVersion: true, isCurrent: current });
    if (stopIfStale()) return;
    if (resolved?.kind === 'released') {
      if (useProjectStore.getState().isDirty || useSpeleoStore.getState().link?.hasUnpushedEdits) { setBusy(false); return; }
      trackDialogComplete('speleodb-unwound-push');
      const treeGeneration = useProjectStore.getState().projectTreeGeneration;
      const closeCurrent = () =>
        useProjectStore.getState().project?.file_path === issuedFor.projectPath &&
        useProjectStore.getState().loading.clientOpId === issuedFor.clientOpId &&
        useProjectStore.getState().projectTreeGeneration === treeGeneration &&
        useSpeleoStore.getState().sessionGeneration === issuedFor.sessionGeneration &&
        !useProjectStore.getState().isDirty && !useSpeleoStore.getState().link?.hasUnpushedEdits;
      if (pendingExit.kind === 'quit') { closeDialog(); performQuit(); }
      else {
        await handBackProject(pendingExit, {
          isCurrent: closeCurrent,
          onClosed: () => { if (useUIStore.getState().dialogData === dialogData) closeDialog(); },
        });
        if (current()) setBusy(false);
        if (!useProjectStore.getState().project) setUploadContinuation(null);
      }
      return;
    }
    // Nothing was released, so nothing may leave - and the exit prompt is the surface
    // that says why: it renders the failure inline (SpeleoPushFailureNotice) and its own
    // push button is the retry. Closing here instead would lose the exit request for the
    // second time, over a failure the user never got to see.
    setBusy(false);
    returnToCaller();
  };

  return (
    <DialogWrapper
      title={t('speleodb.unwoundPush.title')}
      onClose={busy ? () => {} : returnToCaller}
      size="md"
      data-testid="speleodb-unwound-push-dialog"
    >
      <div className="space-y-4">
        <p className="text-sm text-text-secondary" data-testid="speleodb-unwound-push-body">
          {t('speleodb.unwoundPush.body')}
        </p>
        <p className="text-sm text-error font-medium" data-testid="speleodb-unwound-push-warning">
          {t('speleodb.unwoundPush.warning')}
        </p>
        <p className="text-xs text-text-secondary">{t('speleodb.unwoundPush.discardHint')}</p>

        <SpeleoPushFailureNotice />

        <div className="flex justify-end gap-2 pt-2 border-t border-border">
          <Button
            onClick={returnToCaller}
            disabled={busy}
            data-testid="speleodb-unwound-push-cancel"
          >
            {t('common.actions.cancel')}
          </Button>
          {/* Not primary, and named for what it does. This is the deliberate roll-back,
              which is a real thing to want - just never the thing you want by accident. */}
          <Button
            variant="danger"
            onClick={() => void pushAnyway()}
            disabled={busy}
            data-testid="speleodb-unwound-push-push-anyway"
          >
            {t('speleodb.unwoundPush.pushAnyway')}
          </Button>
          <Button
            variant="primary"
            onClick={() => void discardAndLock()}
            disabled={busy}
            data-primary-action=""
            data-testid="speleodb-unwound-push-discard"
          >
            {busy ? t('speleodb.unwoundPush.working') : t('speleodb.unwoundPush.discardAndLock')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
