import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { doneFinished, isUnwoundNotEdited, useSpeleoStore } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { isProjectChangedRefusal } from '../../store/commandResult';
import { useSpeleoActions } from '../../hooks/useSpeleoActions';
import { surfaceSaveFailure } from '../../utils/saveFeedback';
import { getErrorMessage } from '../../utils/ipc';
import { toastProjectChanged } from '../../utils/projectChangedNotice';
import { handBackProject, holdsWriteLock, type PendingExit } from '../../utils/quit';
import { readPendingImport } from '../../utils/pendingImport';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { SpeleoPushFailureNotice } from './SpeleoPushFailureNotice';

/**
 * Close-flow guard (T2d): shown when closing a SpeleoDB project while you still
 * hold the write lock. This path bypasses the normal unsaved-changes prompt, so
 * it handles both dirty states itself:
 * - **unsaved local changes** (`projectStore.isDirty`) → a "Save changes locally"
 *   checkbox (default on) so closing can't silently drop them;
 * - **saved locally but not pushed** (`link.hasUnpushedEdits`) → a note that the
 *   work is safe on disk, it just isn't on SpeleoDB yet.
 * Then it encourages pushing + releasing the lock before leaving.
 *
 * #270: "closing" here means any of the three exits that hand the open project
 * back while the app keeps running - Close, opening another `.stalx`, and New
 * Project (see `needsSpeleoHandBackPrompt`). The latter two carry their request in
 * as `pendingOpenPath` / `pendingNewProject`, the same shape `UnsavedChangesDialog`
 * uses, and it is replayed once the SpeleoDB side is resolved.
 */
export default function SpeleoCloseDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const openDialog = useUIStore((s) => s.openDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  const showToast = useUIStore((s) => s.showToast);
  const link = useSpeleoStore((s) => s.link);
  const saveProject = useProjectStore((s) => s.saveProject);
  const isDirtyLocal = useProjectStore((s) => s.isDirty);
  const { busy, done } = useSpeleoActions();

  // What the user actually asked for, when this prompt was raised by something
  // other than plain Close. Routing those exits here is only half the fix: a
  // prompt that swallowed the action that raised it would trade a silent swap for
  // a dead end, and the user would be sitting in the same project wondering why
  // Ctrl+O did nothing.
  const pendingOpenPath =
    typeof dialogData.pendingOpenPath === 'string' ? dialogData.pendingOpenPath : null;
  const pendingNewProject = dialogData.pendingNewProject === true;
  // [#576 HFI-4-D2] "Import as new project" after the file parsed: the parsed file,
  // opened in the new-project dialog once the project is handed back.
  const pendingImport = readPendingImport(dialogData.pendingImport);

  const sdbDirty = link?.hasUnpushedEdits ?? false;
  // [#303] Dirty ONLY because a chain step walked the copy back - the user edited nothing.
  // The wording below must not call that "unpushed changes", and pushing it must not be
  // the primary action: it publishes the older survey over the newer one. `done` raises
  // its own confirmation, so the button here stays available but stops being the default.
  const unwound = isUnwoundNotEdited(link);
  // #270: the close guard now also fires on un-pushed edits ALONE, matching
  // `guardQuit` - so this dialog is reachable with no lock (a re-verify divergence,
  // or a token loss that ran `clearSession()`). Every verb below needs one, so it
  // adapts exactly the way SpeleoQuitDialog does rather than offering two buttons
  // that can only fail. `forceEditing` is the #213 state of editing WITHOUT a lock,
  // which is equally nothing to push or release.
  const holdsLock = holdsWriteLock(link);
  const [saveLocally, setSaveLocally] = useState(isDirtyLocal);
  const [saving, setSaving] = useState(false);
  const busyAny = busy || saving;

  // Persist the local .stalx first when there are unsaved changes and the user
  // left the checkbox on (so neither the push nor the close loses them). Returns
  // false when a requested save FAILED (or was cancelled) so the caller aborts and
  // the dialog stays open - closing anyway would drop the unsaved work (fix 3).
  const maybeSaveLocally = async (): Promise<boolean> => {
    if (!(isDirtyLocal && saveLocally)) return true;
    setSaving(true);
    try {
      const result = await saveProject();
      if (!result?.success) {
        // [#576 W87a-NF1] A save refused because a project switch is under way is no
        // failure to report, so `surfaceSaveFailure` shows nothing for it; the user
        // pressed a button, though, and the prompt stays open. Say why, as
        // SpeleoQuitDialog does: the quiet notice an edit refused during a switch gets
        // (`switchEditNotice.ts`), and for the backend's own PROJECT_CHANGED a
        // user-started verb's short line. The project is not handed back.
        if (isProjectChangedRefusal(result)) {
          if (result.code === 'PROJECT_CHANGED') toastProjectChanged(result);
          else showToast(t('editGate.switchPendingNotice'), TOAST_KEYS.EDIT_DURING_SWITCH, 'info');
          return false;
        }
        // Surface WHY the save failed (integrity block -> Review & Repair, a real
        // IO error -> error dialog; a permission denial is already surfaced by
        // saveProject) instead of silently keeping this dialog open with no
        // feedback - matches every other save path (fix S4). The abort below still
        // keeps the project open so unsaved work is never dropped.
        surfaceSaveFailure(result, openDialog, useProjectStore.getState());
        return false;
      }
      return true;
    } catch (e) {
      // A thrown save (rare - real failures resolve as {success:false}) must not be
      // treated as success; surface it and keep the project open over unsaved work.
      openDialog('error', {
        title: t('speleodb.errors.saveFailedTitle'),
        message: getErrorMessage(e, t('speleodb.errors.saveFailedMessage')),
      });
      return false;
    } finally {
      setSaving(false);
    }
  };

  /**
   * The exit this prompt is standing in for, in the shape a dialog raised OVER this one
   * can finish by itself (#303). The hand-back and the replay live in `handBackProject`
   * so both finishers order them identically.
   */
  const closeIntent: PendingExit = { kind: 'close', pendingOpenPath, pendingNewProject, pendingImport };
  const closeThen = (opts?: { releaseLock?: boolean }) => handBackProject(closeIntent, opts);

  const pushReleaseThenClose = async () => {
    if (!(await maybeSaveLocally())) return;
    // done(null) pushes (when dirty) and releases the lock. Only leave once the
    // lock is actually RELEASED - a divergence / unconfirmed push keeps us here so
    // the user resolves it rather than walking away still holding the lock (fix 2).
    //
    // [#303] ...and on an unwound copy it does not push at all: it raises its own
    // confirmation over this dialog and answers `null`. The close (and the Open / New
    // Project it may be standing in for) would be lost with this prompt, so the intent
    // goes with it and the confirmation completes it.
    const resolved = await done(null, { pendingExit: closeIntent });
    if (doneFinished(resolved?.kind)) {
      closeDialog();
      await closeThen();
    }
  };

  const closeKeepingLock = async () => {
    if (!(await maybeSaveLocally())) return;
    closeDialog();
    // Explicitly retain the server lock: this is the one close path that opts out of
    // the default lock-release (fix 1), so the user keeps their editing claim.
    // Also the lock-less exit: there is no lock to release, and asking the backend
    // to release one it does not hold is the only other option.
    await closeThen({ releaseLock: false });
  };

  return (
    <DialogWrapper
      title={t('speleodb.close.title')}
      // Not dismissable mid-verb, for the same reason as the quit dialog: the
      // in-flight push/release outlives the dismissal and goes on to act on a
      // decision the user can no longer see.
      onClose={busyAny ? () => {} : closeDialog}
      size="md"
      data-testid="speleodb-close-dialog"
    >
      <div className="space-y-4">
        <p className="text-sm text-text-secondary">
          {!holdsLock
            ? t('speleodb.close.messageNoLock')
            : unwound
              ? t('speleodb.close.unwoundMessage')
              : sdbDirty
                ? t('speleodb.close.dirtyMessage')
                : t('speleodb.close.message')}
        </p>

        {/* Say what happens AFTER the close when the user did not ask for one. Every
            verb below is honest as written - the project really is closed - but the
            title and buttons all say "close", and someone who pressed Ctrl+O, New
            Project or Import as new project needs to be told their request survived
            the prompt. */}
        {(pendingOpenPath || pendingNewProject || pendingImport) && (
          <p className="text-xs text-text-secondary" data-testid="speleodb-close-pending-note">
            {pendingOpenPath
              ? t('speleodb.close.thenOpen')
              : pendingNewProject
                ? t('speleodb.close.thenNew')
                : t('speleodb.close.thenImport')}
          </p>
        )}

        {isDirtyLocal ? (
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={saveLocally}
              onChange={(e) => setSaveLocally(e.target.checked)}
              className="rounded"
              data-testid="speleodb-close-save-locally"
            />
            {t('speleodb.close.saveLocally')}
          </label>
        ) : sdbDirty && !unwound ? (
          <p className="text-xs text-text-secondary" data-testid="speleodb-close-saved-locally-note">
            {t('speleodb.close.savedLocallyNote')}
          </p>
        ) : null}

        {/* #260: same problem as the Finish-editing dialog - this one also stays
            open when `done` throws, with the shell banner stranded behind its
            scrim. Show the failure here; "Push & release" below is the retry. */}
        <SpeleoPushFailureNotice />
        <div className="flex justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={closeDialog} disabled={busyAny} data-testid="speleodb-close-cancel">
            {t('common.actions.cancel')}
          </Button>
          {holdsLock ? (
            <>
              <Button
                variant={unwound ? 'primary' : 'secondary'}
                onClick={() => void closeKeepingLock()}
                disabled={busyAny}
                data-primary-action={unwound ? '' : undefined}
                data-testid="speleodb-close-keep-lock"
              >
                {t('speleodb.close.keepLock')}
              </Button>
              <Button
                variant={unwound ? 'danger' : 'primary'}
                onClick={() => void pushReleaseThenClose()}
                disabled={busyAny}
                data-testid="speleodb-close-push-release"
              >
                {unwound
                  ? t('speleodb.close.pushOlderVersion')
                  : sdbDirty
                    ? t('speleodb.close.pushRelease')
                    : t('speleodb.close.release')}
              </Button>
            </>
          ) : (
            // No lock: nothing to push and nothing to release. The one honest action
            // is to flush the .stalx (the checkbox above) and hand the project back,
            // with the work still only on this computer. Same shape as
            // SpeleoQuitDialog's "Quit anyway", and not `danger` - nothing is lost.
            <Button
              variant="primary"
              onClick={() => void closeKeepingLock()}
              disabled={busyAny}
              data-testid="speleodb-close-anyway"
            >
              {t('speleodb.close.closeAnyway')}
            </Button>
          )}
        </div>
      </div>
    </DialogWrapper>
  );
}
