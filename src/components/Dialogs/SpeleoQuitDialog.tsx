import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { doneFinished, isUnwoundNotEdited, useSpeleoStore } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { expectedOpenProject } from '../../store/expectedOpenProject';
import { useSpeleoActions } from '../../hooks/useSpeleoActions';
import { surfaceSaveFailure } from '../../utils/saveFeedback';
import { invokeIpc, getErrorMessage } from '../../utils/ipc';
import { humanizeSpeleoError } from '../../utils/speleoError';
import { toastProjectChanged } from '../../utils/projectChangedNotice';
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
 * Sibling of {@link SpeleoCloseDialog} rather than a mode of it: the close flow
 * hands the project back and always ends with a decision about the lock, while
 * quitting has a genuinely different third option - walk away WITHOUT touching
 * the lock, which is now the safe default because the backend's exit-time release
 * vetoes itself when there are unpushed edits (aab5121d). Wording, the
 * save-locally checkbox and the failure surface are kept in lock-step with the
 * close dialog on purpose.
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
  const refreshLinkStatus = useSpeleoStore((s) => s.refreshLinkStatus);
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
  const [releasing, setReleasing] = useState(false);
  // The destructive option is two-step on purpose: the first press only arms it.
  const [discardArmed, setDiscardArmed] = useState(false);
  const busyAny = busy || saving || releasing;

  // Persist the local .stalx first when there are unsaved changes and the user left
  // the checkbox on, so neither the push nor the quit loses them. Returns false when
  // a requested save FAILED (or was cancelled) so the caller aborts and the dialog
  // stays open - quitting anyway would drop the unsaved work. Mirrors
  // SpeleoCloseDialog.maybeSaveLocally.
  const maybeSaveLocally = async (): Promise<boolean> => {
    if (!(isDirtyLocal && saveLocally)) return true;
    setSaving(true);
    try {
      const result = await saveProject();
      if (!result?.success) {
        surfaceSaveFailure(result, openDialog, useProjectStore.getState());
        return false;
      }
      return true;
    } catch (e) {
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
  const pushReleaseThenQuit = async () => {
    if (!(await maybeSaveLocally())) return;
    // [#303] On an unwound copy `done` never pushes: it raises its own confirmation over
    // this dialog and answers `null`, so the quit below cannot run and this prompt is
    // already gone from the single dialog slot - the quit request evaporated. Hand the
    // intent to the confirmation, which finishes the quit once the push actually releases
    // and puts this prompt back on Cancel.
    const resolved = await done(null, { pendingExit: { kind: 'quit' } });
    if (doneFinished(resolved?.kind)) {
      closeDialog();
      performQuit();
    }
  };

  // 2. Keep the lock and quit: the safe default. NOTHING is released here - the
  // edits stay local and the lock stays ours, so nobody else can start editing from
  // the older server tip while our work is still on this computer. The backend's
  // exit-time release sees the unpushed edits and stands down.
  const keepLockAndQuit = async () => {
    if (!(await maybeSaveLocally())) return;
    closeDialog();
    performQuit();
  };

  // 3. Discard and release: deliberately give the lock back WITHOUT pushing, so the
  // unpushed edits stay behind on this computer only and anyone else may edit from
  // the older server version. Destructive, two-step, and never the default. A failed
  // release does not quit - we'd otherwise claim to have freed a lock we still hold.
  const discardReleaseThenQuit = async () => {
    // [#576 RVF-c-C1] The project whose lock this releases, before the save's await.
    const expectedProject = expectedOpenProject();
    if (!(await maybeSaveLocally())) return;
    setReleasing(true);
    try {
      await invokeIpc('speleodb_release_current_lock', { expectedProject });
    } catch (e) {
      // [#576 Q-W44-7-D1] Refused because the project changed: the short line instead.
      if (!toastProjectChanged(e)) {
        showToast(humanizeSpeleoError(e, t('speleodb.lock.releaseFailed')), TOAST_KEYS.SPELEODB_LOCK, 'error');
      }
      setReleasing(false);
      // Re-probe so the button/badge reflect whatever the server actually thinks.
      void refreshLinkStatus();
      return;
    }
    closeDialog();
    performQuit();
  };

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
            {unwound ? t('speleodb.quit.keepLockHintUnwound') : t('speleodb.quit.keepLockHint')}
          </p>
        )}

        {discardArmed && (
          <div
            className="rounded border border-error bg-error-20 text-error text-sm px-3 py-2"
            role="alert"
            data-testid="speleodb-quit-discard-warning"
          >
            {unwound
              ? t('speleodb.quit.discardWarningUnwound')
              : t('speleodb.quit.discardWarning')}
          </div>
        )}

        {/* Same inline failure surface the Finish-editing / close dialogs use: a
            failed upload leaves this dialog open, and the shell banner would sit
            behind its scrim (#260). "Push, release & quit" below is the retry. */}
        <SpeleoPushFailureNotice />

        <div className="flex flex-wrap justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={dismiss} disabled={busyAny} data-testid="speleodb-quit-cancel">
            {t('common.actions.cancel')}
          </Button>
          {holdsLock ? (
            <>
              <Button
                variant="danger"
                onClick={() => {
                  if (!discardArmed) {
                    setDiscardArmed(true);
                    return;
                  }
                  void discardReleaseThenQuit();
                }}
                disabled={busyAny}
                data-testid="speleodb-quit-discard-release"
              >
                {discardArmed
                  ? t('speleodb.quit.discardConfirm')
                  : t('speleodb.quit.discardRelease')}
              </Button>
              {/* [#303] On an unwound copy the roles swap: keeping the lock and walking
                  away changes nothing on the server, so THAT becomes the default a stray
                  Enter reaches. Same swap as SpeleoCloseDialog. */}
              <Button
                variant={unwound ? 'primary' : 'secondary'}
                onClick={() => void keepLockAndQuit()}
                disabled={busyAny}
                data-primary-action={unwound ? '' : undefined}
                data-testid="speleodb-quit-keep-lock"
              >
                {t('speleodb.quit.keepLock')}
              </Button>
              {/* Primary + [data-primary-action]: DialogWrapper's initial-focus
                  heuristic lands here, so a stray Enter pushes - it can never
                  discard or release. Except on an unwound copy, where pushing IS the
                  destructive answer (it records the older survey over the newer one),
                  so it hands the default to "keep the lock" above and turns danger. */}
              <Button
                variant={unwound ? 'danger' : 'primary'}
                onClick={() => void pushReleaseThenQuit()}
                disabled={busyAny}
                data-primary-action={unwound ? undefined : ''}
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
              onClick={() => void keepLockAndQuit()}
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
