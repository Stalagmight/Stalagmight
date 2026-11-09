import { isProjectRequestCurrent, reserveProjectRequest } from '../../utils/projectTransition';
import { releaseSpeleoLock } from '../../utils/releaseSpeleoLock';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore, isUnwoundNotEdited } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoActions } from '../../hooks/useSpeleoActions';
import { surfaceSaveFailure } from '../../utils/saveFeedback';
import { getErrorMessage } from '../../utils/ipc';
import { humanizeSpeleoError } from '../../utils/speleoError';
import { abandonUpdateRestart, holdsWriteLock, performQuit } from '../../utils/quit';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { SpeleoPushFailureNotice } from './SpeleoPushFailureNotice';
import { TOAST_KEYS } from '../../utils/toastKeys';

/**
 * [#270] Quit-flow guard: shown when the app is asked to quit (menu Quit, Ctrl+Q,
 * or the OS window X - `guardQuit` is wired into all of them) while we still hold
 * the SpeleoDB write lock AND have edits the server has not seen.
 *
 * Keep lock closes locally; release choices surrender the held lock. Uploading requires a commit message;
 * leaving without uploading keeps the local file and warns about unpublished work.
 *
 * `projectStore.isDirty` (unsaved to DISK) and `link.hasUnpushedEdits` (not on the
 * SERVER) are independent, so this dialog handles both: the checkbox flushes the
 * .stalx first, then the chosen lock action runs. That is also why this guard runs
 * BEFORE the plain unsaved-changes prompt - it subsumes it, and one prompt can
 * never swallow the other.
 */
export default function SpeleoQuitDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const openDialog = useUIStore((s) => s.openDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  const showToast = useUIStore((s) => s.showToast);
  const saveProject = useProjectStore((s) => s.saveProject);
  const isDirtyLocal = useProjectStore((s) => s.isDirty);
  const { busy, done } = useSpeleoActions();
  const link = useSpeleoStore((s) => s.link);
  // Do we still hold the write lock? `forceEditing` is the #213 state of editing
  // WITHOUT one, so there is nothing to push/keep/release there either.
  //
  // The guard now fires on unpushed edits alone, so this dialog is also reached
  // read-only - after a re-verify divergence, or once a token loss ran
  // `clearSession()`. Every lock verb below would fail in that state, so it
  // collapses to the one honest choice: save locally, then leave, with the work
  // still only on this computer.
  const holdsLock = holdsWriteLock(link);
  // [#303] Dirty ONLY because a chain step walked the copy back - the user edited nothing.
  // Kept in lock-step with SpeleoCloseDialog, which is the whole reason these two are
  // siblings: the wording must not call that "changes that have not been uploaded", and
  // pushing it must not be the pre-focused default, because it records the older survey
  // over the newer one for everyone. `done` raises its own confirmation, so the button
  // stays available - it just stops being what a stray Enter reaches.
  const unwound = isUnwoundNotEdited(link);

  // Which dialog this prompt displaced in the single `activeDialog` slot, if any.
  // Cancelling means the app keeps running, so whatever was on screen has to come
  // back - see `openExitPrompt` in utils/quit for the import wizard this exists for.
  const cancelReturnsTo =
    typeof dialogData.cancelReturnsTo === 'string' ? dialogData.cancelReturnsTo : null;
  const cancelReturnsData =
    typeof dialogData.cancelReturnsData === 'object' && dialogData.cancelReturnsData !== null
      ? (dialogData.cancelReturnsData as Record<string, unknown>)
      : undefined;

  /**
   * Cancel / X / backdrop. Only these: the three verbs below all end in
   * `performQuit()`, where restoring a dialog into a window that is going away would
   * be pointless at best.
   */
  const dismiss = () => {
    // [#579] Same as UnsavedChangesDialog's Cancel: abandoning the exit
    // abandons an update restart riding on it, on both sides.
    abandonUpdateRestart();
    if (cancelReturnsTo) openDialog(cancelReturnsTo, cancelReturnsData);
    else closeDialog();
  };

  const [saveLocally, setSaveLocally] = useState(isDirtyLocal);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState('');
  const needsMessage = isDirtyLocal || (link?.hasUnpushedEdits ?? false);
  const [releasing, setReleasing] = useState(false);
  // Guard all asynchronous exit operations against duplicate submissions.
  const operation = useRef(false);
  const busyAny = busy || saving || releasing;

  const runQuit = async (work: (current: () => boolean, request: number) => Promise<void>) => {
    if (operation.current || busyAny) return;
    operation.current = true;
    setReleasing(true);
    const request = reserveProjectRequest();
    const state = useProjectStore.getState();
    const path = state.project?.file_path;
    const op = state.loading.clientOpId;
    const generation = useSpeleoStore.getState().sessionGeneration;
    const current = () => isProjectRequestCurrent(request) && useProjectStore.getState().project?.file_path === path &&
      useProjectStore.getState().loading.clientOpId === op &&
      useSpeleoStore.getState().sessionGeneration === generation &&
      useUIStore.getState().activeDialog === 'speleodb-quit' &&
      useUIStore.getState().dialogData === dialogData;
    try { await work(current, request); }
    catch (error) { if (current()) showToast(getErrorMessage(error), TOAST_KEYS.SPELEODB_LOCK, 'error'); }
    finally { operation.current = false; setReleasing(false); }
  };

  // Persist the local .stalx first when there are unsaved changes and the user left
  // the checkbox on, so neither the push nor the quit loses them. Returns false when
  // a requested save FAILED (or was cancelled) so the caller aborts and the dialog
  // stays open - quitting anyway would drop the unsaved work. Mirrors
  // SpeleoCloseDialog.maybeSaveLocally.
  const maybeSaveLocally = async (current: () => boolean): Promise<boolean> => {
    if (!(isDirtyLocal && saveLocally)) return true;
    setSaving(true);
    try {
      const result = await saveProject();
      if (!current()) return false;
      if (!result?.success) {
        surfaceSaveFailure(result?.error ?? null, openDialog);
        return false;
      }
      return true;
    } catch (e) {
      if (!current()) return false;
      openDialog('error', {
        title: t('speleodb.errors.saveFailedTitle'),
        message: getErrorMessage(e, t('speleodb.errors.saveFailedMessage')),
      });
      return false;
    } finally {
      setSaving(false);
    }
  };

  // 1. Push and release, then quit. `done(null)` is the existing hand-off verb
  // (push when dirty + release); we only leave once the lock is actually RELEASED.
  // A failed upload, a divergence or an unconfirmed push all keep us here with the
  // failure on screen (SpeleoPushFailureNotice) - quitting on a failed push would
  // be exactly the data loss this dialog exists to prevent.
  const pushReleaseThenQuit = () => runQuit(async (current) => {
    if (needsMessage && !message.trim()) return;
    if (!(await maybeSaveLocally(current)) || !current()) return;
    // [#303] On an unwound copy `done` never pushes: it raises its own confirmation over
    // this dialog and answers `null`, so the quit below cannot run and this prompt is
    // already gone from the single dialog slot - the quit request evaporated. Hand the
    // intent to the confirmation, which finishes the quit once the push actually releases
    // and puts this prompt back on Cancel.
    const resolved = await done(message.trim() || null, { pendingExit: { kind: 'quit' }, isCurrent: current });
    if (current() && resolved?.kind === 'released') {
      closeDialog();
      performQuit();
    }
  });

  const keepLockAndQuit = () => runQuit(async (current, request) => {
    const generation = useSpeleoStore.getState().sessionGeneration;
    if (!await maybeSaveLocally(current) || !current()) return;
    const result = await useProjectStore.getState().closeProject({ releaseLock: false, request });
    if (!result.success || !isProjectRequestCurrent(request) || useProjectStore.getState().project !== null ||
      useSpeleoStore.getState().sessionGeneration !== generation || useUIStore.getState().dialogData !== dialogData) return;
    closeDialog();
    performQuit();
  });

  // Save locally if requested, release our lock, then quit. A failed release stays here.
  const releaseWithoutUploadAndQuit = () => runQuit(async (current) => {
    if (!(await maybeSaveLocally(current)) || !current()) return;
    if (holdsLock) {
      try {
        if (!await releaseSpeleoLock(current)) return;
      } catch (e) {
        if (!current()) return;
        showToast(humanizeSpeleoError(e, t('speleodb.lock.releaseFailed')), TOAST_KEYS.SPELEODB_LOCK, 'error');
        return;
      }
    }
    if (!current()) return;
    closeDialog();
    performQuit();
  });


  return (
    <DialogWrapper
      title={t('speleodb.quit.title')}
      // Not dismissable while a verb is in flight. Escape / X / the backdrop used
      // to close this dialog mid-push, and the in-flight `pushReleaseThenQuit`
      // then went on to resolve `released` and call `performQuit()` - which arms
      // the one-shot confirmation, so the resulting close request skips
      // `guardQuit` entirely and the window goes away with no prompt, dropping
      // anything edited in that window. Same idiom as the other dialogs whose
      // work outlives their own dismissal (BulkDelete / DeleteShotChoice).
      onClose={busyAny ? () => {} : dismiss}
      size="md"
      data-testid="speleodb-quit-dialog"
    >
      <div className="space-y-4">
        <p className="text-sm text-text-secondary">
          {!holdsLock
            ? t('speleodb.quit.messageNoLock')
            : unwound
              ? t('speleodb.quit.unwoundMessage')
              : t('speleodb.quit.message')}
        </p>

        {isDirtyLocal ? (
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              disabled={busyAny}
              checked={saveLocally}
              onChange={(e) => setSaveLocally(e.target.checked)}
              className="rounded"
              data-testid="speleodb-quit-save-locally"
            />
            {t('speleodb.quit.saveLocally')}
          </label>
        ) : unwound ? null : (
          // Nothing of the user's is pending on an unwound copy, so "your changes are
          // saved locally" would be describing work that does not exist.
          <p className="text-xs text-text-secondary" data-testid="speleodb-quit-saved-locally-note">
            {t('speleodb.quit.savedLocallyNote')}
          </p>
        )}

        {holdsLock && (
          <p className="text-xs text-text-secondary">
            {unwound ? t('speleodb.quit.releaseWithoutUploadHintUnwound') : t('speleodb.quit.releaseWithoutUploadHint')}
          </p>
        )}

        {/* Same inline failure surface the Finish-editing / close dialogs use: a
            failed upload leaves this dialog open, and the shell banner would sit
            behind its scrim (#260). "Push, release & quit" below is the retry. */}
        {holdsLock && needsMessage && (
          <div>
            <label htmlFor="speleodb-quit-message" className="block text-sm text-text-secondary mb-1">{t('speleodb.done.messageLabel')}</label>
            <textarea id="speleodb-quit-message" disabled={busyAny} value={message} onChange={(e) => setMessage(e.target.value)} rows={3} className="w-full px-2 py-1 bg-background border border-border rounded text-sm" data-testid="speleodb-quit-message" />
          </div>
        )}
        <SpeleoPushFailureNotice />

        <div className="flex flex-wrap justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={dismiss} disabled={busyAny} data-testid="speleodb-quit-cancel">
            {t('common.actions.cancel')}
          </Button>
          {holdsLock && (
            <Button variant="primary" onClick={() => void keepLockAndQuit()} disabled={busyAny} data-primary-action="" data-testid="speleodb-quit-keep-lock">
              Keep lock and quit
            </Button>
          )}
          {holdsLock ? (
            <>
              <Button
                variant="secondary"
                onClick={() => void releaseWithoutUploadAndQuit()}
                disabled={busyAny}

                data-testid="speleodb-quit-release"
              >
                {t('speleodb.quit.releaseWithoutUpload')}
              </Button>
              <Button
                variant={unwound ? 'danger' : 'secondary'}
                onClick={() => void pushReleaseThenQuit()}
                disabled={busyAny || (needsMessage && !message.trim())}

                data-testid="speleodb-quit-push-release"
              >
                {unwound
                  ? t('speleodb.quit.pushOlderVersion')
                  : t('speleodb.quit.pushRelease')}
              </Button>
            </>
          ) : (
            // No lock: every verb above needs one. The only thing left to do is
            // flush the .stalx (the checkbox) and leave, knowing the work is still
            // only local. Not `danger` - nothing is destroyed, and the edits are
            // still here on the next launch.
            <Button
              variant="primary"
              onClick={() => void releaseWithoutUploadAndQuit()}
              disabled={busyAny}
              data-primary-action=""
              data-testid="speleodb-quit-anyway"
            >
              {t('speleodb.quit.quitAnyway')}
            </Button>
          )}
        </div>
      </div>
    </DialogWrapper>
  );
}
