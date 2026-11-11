import { canResumeSpeleoEditing } from '../../utils/speleoPermission';
import { saveOpenProjectLocally } from '../../utils/saveOpenProjectLocally';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { useProject } from '../../hooks/useProject';
import { captureUploadOwner, getSetAsideUploadMessage, getUploadContinuation, setAsideUploadMessage, setUploadContinuation, uploadOwnerStillCurrent } from '../../utils/uploadContinuation';
import { useSpeleoStore } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoActions } from '../../hooks/useSpeleoActions';
import { getErrorMessage } from '../../utils/ipc';
import { surfaceSaveFailure } from '../../utils/saveFeedback';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { SpeleoPushFailureNotice } from './SpeleoPushFailureNotice';
import { SpeleoProgressOverlay } from './SpeleoProgressOverlay';
import { SpeleoUploadErrorsNotice } from './SpeleoUploadErrorsNotice';
import { countUnresolvedErrors, unresolvedErrorsKey } from '../../utils/validation';
import { CommitMessageLimit } from './CommitMessageLimit';
import { commitMessageTooLong } from '../../utils/commitMessage';
import { toastLateUploadOutcome } from '../../utils/lateUploadOutcome';
// lock#6: a quit meanwhile waits for the whole save-upload-close, not just its calls.
import { holdQuitDuring } from '../../utils/quitAfterWork';

/**
 * Commit-message confirm for the action shell (#204), in two modes read off
 * `dialogData.mode`:
 * - **release** (default): `speleodb_done` - render + VACUUM snapshot + push +
 *   release the lock, returning to read-only.
 * - **push** (T2d, fix 8): `speleodb_push` - the same push WITHOUT releasing the
 *   lock, so editing continues.
 *
 * Both collect a required message when uploading changes then route through `useSpeleoActions`. Closes
 * only when the verb actually LANDED (released / pushed). A `divergence_needed`
 * keeps the dialog's typed message alive and lets the App-level effect open the
 * resolution dialog over it (fix 3); a `push_unconfirmed` keeps it open for a retry
 * (the #200 banner surfaces the ambiguity); a thrown (network) error likewise keeps
 * it open so the message and a retry survive.
 *
 * [#260] A thrown error is ALSO rendered INLINE here (`SpeleoPushFailureNotice`):
 * the shell's push-failure banner sits behind this dialog's modal scrim, so it was
 * unreadable and unclickable exactly when it mattered. The retry is this dialog's
 * own primary button; the standalone banner takes over only once the dialog is
 * dismissed with the failure unresolved.
 */
export default function SpeleoDoneDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  const mode = dialogData.mode === 'push' ? 'push' : 'release';
  const { busy, push, done, startEditing } = useSpeleoActions();
  const { handleCloseProject } = useProject();
  const path = useProjectStore((s) => s.project?.file_path);
  const [owner] = useState(captureUploadOwner);
  const sessionGeneration = useSpeleoStore((s) => s.sessionGeneration);
  const projectGeneration = useProjectStore((s) => s.projectTreeGeneration);
  const ownerCurrent = uploadOwnerStillCurrent(owner);
  const closeAfterSave = dialogData.closeAfterSave === true;
  const [message, setMessage] = useState(() => getUploadContinuation(path)?.message ?? getSetAsideUploadMessage(path) ?? '');
  // Set by "Resolve now", which hands the message over as set aside, not pending:
  // this dialog must not mirror it back while it closes.
  const messageSetAside = useRef(false);
  useEffect(() => {
    if (!ownerCurrent) {
      if (useUIStore.getState().activeDialog === 'speleodb-done' && useUIStore.getState().dialogData === dialogData) closeDialog();
      return;
    }
    if (path && !messageSetAside.current) setUploadContinuation({ path, message, closeAfterSave }, owner);
  }, [path, message, closeAfterSave, owner, ownerCurrent, sessionGeneration, projectGeneration, dialogData, closeDialog]);
  const link = useSpeleoStore((s) => s.link);
  const session = useSpeleoStore((s) => s.session);
  const loggedIn = session?.loggedIn;
  const canAcquire = canResumeSpeleoEditing(link, session) && link?.lockedByOther !== true;
  const uploadAllowed = link?.editState === 'editing' && canResumeSpeleoEditing(link, session);
  const [submitting, setSubmitting] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [uploadProgressExpired, setUploadProgressExpired] = useState(false);
  useEffect(() => {
    if (!uploading) return;
    const timer = window.setTimeout(() => setUploadProgressExpired(true), 120_000);
    return () => window.clearTimeout(timer);
  }, [uploading]);
  const [saveFailure, setSaveFailure] = useState<string | null>(null);
  const attempt = useRef(false);
  const busyAny = busy || submitting;
  const canDismiss = !busyAny || (uploading && uploadProgressExpired);
  const unpushed = useSpeleoStore((s) => s.link?.hasUnpushedEdits);
  const unsaved = useProjectStore((s) => s.isDirty);
  const needsMessage = mode === 'push' || unpushed || unsaved;
  // Errors in the survey go to every collaborator once uploaded: with changes to
  // upload, the dialog waits until the user fixes them or accepts uploading them
  // as they are. With nothing to upload (a Done, or a Save and Close, that only
  // releases the lock) it never asks.
  const errorCount = useProjectStore((s) => countUnresolvedErrors(s.warnings));
  const errorsKey = useProjectStore((s) => unresolvedErrorsKey(s.warnings));
  // The tick accepts the errors it was given for: any other set asks again.
  const [acceptedErrorsKey, setAcceptedErrorsKey] = useState<string | null>(null);
  const errorsAccepted = acceptedErrorsKey === errorsKey;
  const showErrors = uploadAllowed && Boolean(unpushed || unsaved) && errorCount > 0;
  const errorsBlock = showErrors && !errorsAccepted;
  // sdk#2: a description SpeleoDB cannot store is never sent (the backend refuses it too).
  const messageTooLong = commitMessageTooLong(message);
  // Closed by the user while the upload runs (allowed once the 120-second notice
  // shows): the result that lands later is then reported here (frontend#2). Any
  // other way the dialog goes away (the older-version confirmation replacing it,
  // a divergence) has its own surface for the outcome.
  const walkedAway = useRef(false);
  const dismiss = () => {
    if (uploading) walkedAway.current = true;
    else setUploadContinuation(null);
    closeDialog();
  };

  // "Resolve now": no upload, and none left pending - a Save and Close does not
  // close, since the project stays open for the fix. The typed message waits for
  // the next upload dialog.
  const resolveErrors = () => {
    if (busyAny) return;
    messageSetAside.current = true;
    closeDialog();
    setAsideUploadMessage(message, owner);
    useUIStore.getState().openWarningsTab();
  };

  // frontend#6: Save opened this dialog for a project we were not editing, after
  // saving it on this computer (`dispatchProjectSave`). Take the editing lock
  // once, so the upload can go ahead; a refusal says why on its own surface and
  // leaves the blocked panel, with the work already safe on disk.
  const acquireOnOpen = dialogData.acquire === true;
  const acquireTried = useRef(false);
  const [acquiring, setAcquiring] = useState(false);
  useEffect(() => {
    if (!acquireOnOpen || acquireTried.current || uploadAllowed || !canAcquire) return;
    acquireTried.current = true;
    setAcquiring(true);
    void startEditing().finally(() => setAcquiring(false));
  }, [acquireOnOpen, uploadAllowed, canAcquire, startEditing]);

  // frontend#6: when upload is impossible the work still goes somewhere safe -
  // this computer - rather than staying only discardable.
  const saveLocally = async () => {
    if (attempt.current) return;
    attempt.current = true;
    setSubmitting(true);
    try {
      const saved = await saveOpenProjectLocally();
      if (!saved?.success || !uploadOwnerStillCurrent(owner)) return;
      useUIStore.getState().showToast(t('speleodb.saveLocal.saved'), TOAST_KEYS.SPELEODB_SAVED_LOCALLY, 'warning');
      const ui = useUIStore.getState();
      if (ui.activeDialog !== 'speleodb-done' || ui.dialogData !== dialogData) return;
      closeDialog();
      if (closeAfterSave) await handleCloseProject();
    } finally { attempt.current = false; setSubmitting(false); }
  };

  const handleConfirm = async () => {
    const text = message.trim();
    if (!uploadOwnerStillCurrent(owner) || attempt.current || busy || !uploadAllowed || (needsMessage && !text) || messageTooLong || errorsBlock) return;
    attempt.current = true;
    setSubmitting(true);
    setSaveFailure(null);
    const state = useProjectStore.getState();
    const path = state.project?.file_path;
    const op = state.loading.clientOpId;
    const generation = useSpeleoStore.getState().sessionGeneration;
    // frontend#2: two different questions. `sessionCurrent` - is this still the
    // project, operation and SpeleoDB session the upload was started for - gates
    // APPLYING the upload's result, and is all `push` / `done` get as `isCurrent`.
    // Whether THIS dialog is still on screen must not: once the 120-second notice
    // lets the user close it, a result that lands later still updates the link,
    // toasts and raises the failure banner. The dialog's own follow-ups (its inline
    // messages, closing it, Save and Close) still need it open.
    const sessionCurrent = () => uploadOwnerStillCurrent(owner) && useProjectStore.getState().project?.file_path === path &&
      useProjectStore.getState().loading.clientOpId === op &&
      useSpeleoStore.getState().sessionGeneration === generation;
    const dialogOpen = () => useUIStore.getState().activeDialog === 'speleodb-done' &&
      useUIStore.getState().dialogData === dialogData;
    const current = () => sessionCurrent() && dialogOpen();
    /** The dialog is gone: if the user walked away mid-upload, report what it would have. */
    const settledAfterDismiss = (resolved: { kind: string } | null): boolean => {
      if (!sessionCurrent()) return true;
      if (dialogOpen()) return false;
      if (walkedAway.current) {
        if (resolved?.kind === 'pushed' || resolved?.kind === 'released') setUploadContinuation(null);
        toastLateUploadOutcome(resolved);
      }
      return true;
    };
    walkedAway.current = false;
    try {
      if (mode === 'push') {
        let saved;
        try { saved = await state.saveProject(); }
        catch (error) {
          if (current()) surfaceSaveFailure(getErrorMessage(error), useUIStore.getState().openDialog);
          return;
        }
        if (!current()) return;
        if (!saved?.success || saved.unlinked_copy_path) {
          surfaceSaveFailure(saved?.error ?? 'Could not save the linked project', useUIStore.getState().openDialog);
          return;
        }
        setUploadProgressExpired(false);
        setUploading(true);
        const resolved = closeAfterSave ? await done(text, { isCurrent: sessionCurrent, pendingExit: { kind: 'close', sessionGeneration: generation } }) : await push(text, { isCurrent: sessionCurrent });
        setUploading(false);
        if (settledAfterDismiss(resolved)) return;
        if (resolved?.kind === 'released' && closeAfterSave) {
          const latest = useProjectStore.getState();
          if (latest.isDirty || useSpeleoStore.getState().link?.hasUnpushedEdits) {
            setSaveFailure('Newer changes remain on this computer. The project stays open.');
            return;
          }
          const treeGeneration = latest.projectTreeGeneration;
          const closeCurrent = () => current() &&
            useProjectStore.getState().projectTreeGeneration === treeGeneration &&
            !useProjectStore.getState().isDirty && !useSpeleoStore.getState().link?.hasUnpushedEdits;
          const closed = await latest.closeProject({ isCurrent: closeCurrent });
          if (closed.success) {
            const ui = useUIStore.getState();
            if (!useProjectStore.getState().project &&
                useSpeleoStore.getState().sessionGeneration === generation &&
                ui.activeDialog === 'speleodb-done' && ui.dialogData === dialogData) {
              setUploadContinuation(null); closeDialog(); ui.setView('global');
            }
          } else if (current()) setSaveFailure(closed.error ?? 'Could not close project');
        } else if (resolved?.kind === 'pushed') { setUploadContinuation(null); closeDialog(); }
        else if (useSpeleoStore.getState().banner?.kind === 'push_failed') setSaveFailure(null);
        else setSaveFailure(resolved === null || resolved.kind === 'push_unconfirmed'
          ? 'Saved locally; upload could not be confirmed.'
          : 'Saved locally, but not uploaded to SpeleoDB.');
      } else {
        if (unpushed || unsaved) {
          setUploadProgressExpired(false);
          setUploading(true);
        }
        const resolved = await done(text, { isCurrent: sessionCurrent });
        setUploading(false);
        if (settledAfterDismiss(resolved)) return;
        if (resolved?.kind === 'released') closeDialog();
      }
    } finally { attempt.current = false; setUploading(false); setSubmitting(false); }
  };

  if (!ownerCurrent) return null;

  return (
    <>
    <DialogWrapper
      title={mode === 'push' ? t('speleodb.done.pushTitle') : t('speleodb.done.title')}
      onClose={canDismiss ? dismiss : () => {}}
      size="md"
      data-testid="speleodb-done-dialog"
      data-mode={mode}
    >
      <div className="space-y-4">
        {uploading && uploadProgressExpired && <p role="alert" className="text-sm text-warning" data-testid="speleodb-upload-timeout">
          Upload has taken more than 120 seconds. It may still finish. Close this dialog to continue working.
        </p>}
        {!uploadAllowed && <div className="space-y-3" data-testid="speleodb-upload-blocked">
          <p role="status" data-testid="speleodb-upload-blocked-reason">{acquiring ? t('speleodb.saveLocal.acquiring') : t('speleodb.saveLocal.blocked')}</p>
          <div className="flex flex-wrap gap-2">
            <Button onClick={() => void saveLocally()} disabled={busyAny || acquiring} data-testid="speleodb-upload-save-local">{t('speleodb.saveLocal.button')}</Button>
            {!loggedIn ? <Button onClick={() => useUIStore.getState().openDialog('settings', { initialCategory: 'speleodb' })} data-testid="speleodb-upload-sign-in">Connect in Settings</Button> : <Button onClick={() => void startEditing()} disabled={busyAny || acquiring || link?.noAccess} data-testid="speleodb-upload-resume">Resume editing</Button>}
          </div>
        </div>}
        <p className="text-sm text-text-secondary">
          {mode === 'push' ? t('speleodb.done.pushDescription') : t('speleodb.done.description')}
        </p>
        {showErrors && <SpeleoUploadErrorsNotice
          errorCount={errorCount}
          accepted={errorsAccepted}
          onAcceptedChange={(accepted) => setAcceptedErrorsKey(accepted ? errorsKey : null)}
          onResolve={resolveErrors}
          disabled={busyAny}
        />}
        <div>
          <label htmlFor="speleodb-done-message" className="block text-sm text-text-secondary mb-1">
            {t('speleodb.done.messageLabel')}
          </label>
          <textarea
            id="speleodb-done-message"
            disabled={busyAny}
            value={message}
            onChange={(e) => setMessage(e.target.value)}
            required
            rows={3}
            className="w-full px-2 py-1 bg-background border border-border rounded text-sm"
            placeholder={t('speleodb.done.messagePlaceholder')}
            data-testid="speleodb-done-message"
            aria-describedby="speleodb-done-message-limit"
          />
          <CommitMessageLimit message={message} id="speleodb-done-message-limit" testIdPrefix="speleodb-done-message" />
        </div>
        {/* #260: the failure of the LAST attempt, right above the button that made
            it - the shell banner is unreachable behind this dialog's scrim. The
            same button below is the retry; re-pressing it clears this first. */}
        {saveFailure && <p role="alert" data-testid="speleodb-save-upload-failure">{saveFailure}</p>}
        <SpeleoPushFailureNotice />
        <div className="flex justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={dismiss} disabled={!canDismiss} data-testid="speleodb-done-cancel">
            {uploading && uploadProgressExpired ? t('common.actions.close') : t('common.actions.cancel')}
          </Button>
          <Button
            variant="primary"
            onClick={() => void holdQuitDuring(handleConfirm)}
            disabled={busyAny || !uploadAllowed || (needsMessage && !message.trim()) || messageTooLong || errorsBlock}
            title={errorsBlock && !busyAny ? t('speleodb.done.errors.confirmBlocked') : undefined}
            data-testid="speleodb-done-confirm"
          >
            {busyAny
              ? t('speleodb.done.pushing')
              : mode === 'push'
                ? t('speleodb.done.pushConfirm')
                : t('speleodb.done.confirm')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
    {uploading && !uploadProgressExpired && <SpeleoProgressOverlay
      title="Saving changes to SpeleoDB" description="Uploading your survey's latest version." testId="speleodb-upload-progress" />}
    </>
  );
}
