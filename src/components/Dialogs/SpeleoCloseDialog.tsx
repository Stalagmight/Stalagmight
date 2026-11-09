import { releaseSpeleoLock } from '../../utils/releaseSpeleoLock';
import { isProjectRequestCurrent, reserveProjectRequest } from '../../utils/projectTransition';
import { confirmSpeleoFileOpen } from '../../utils/confirmSpeleoOpen';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore, isUnwoundNotEdited } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoActions } from '../../hooks/useSpeleoActions';
import { surfaceSaveFailure } from '../../utils/saveFeedback';
import { getErrorMessage } from '../../utils/ipc';
import { handBackProject, holdsWriteLock, type PendingExit } from '../../utils/quit';
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
  const [message, setMessage] = useState('');
  const needsMessage = isDirtyLocal || (link?.hasUnpushedEdits ?? false);
  const [operating, setOperating] = useState(false);
  const attempt = useRef(false);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const busyAny = busy || saving || operating;
  const dismiss = () => { if (!attempt.current && !busyAny) closeDialog(); };

  // Capture stable project/session ownership, allowing ordinary saves to refresh project objects.
  const runClose = async (work: (current: () => boolean, request: number) => Promise<void>) => {
    if (attempt.current || busy) return;
    attempt.current = true;
    setOperating(true);
    const project = useProjectStore.getState();
    const path = project.project?.file_path;
    const op = project.loading.clientOpId;
    const sessionGeneration = useSpeleoStore.getState().sessionGeneration;
    const request = reserveProjectRequest();
    const current = () => mounted.current && isProjectRequestCurrent(request) &&
      useUIStore.getState().activeDialog === 'speleodb-close' &&
      useUIStore.getState().dialogData === dialogData &&
      useProjectStore.getState().project?.file_path === path &&
      useProjectStore.getState().loading.clientOpId === op &&
      useSpeleoStore.getState().sessionGeneration === sessionGeneration;
    try { await work(current, request); }
    finally { attempt.current = false; if (mounted.current) setOperating(false); }
  };

  // Persist the local .stalx first when there are unsaved changes and the user
  // left the checkbox on (so neither the push nor the close loses them). Returns
  // false when a requested save FAILED (or was cancelled) so the caller aborts and
  // the dialog stays open - closing anyway would drop the unsaved work (fix 3).
  const maybeSaveLocally = async (current: () => boolean): Promise<boolean> => {
    if (!(isDirtyLocal && saveLocally)) return true;
    setSaving(true);
    try {
      const result = await saveProject();
      if (!current()) return false;
      if (!result?.success) {
        // Surface WHY the save failed (integrity block -> Review & Repair, a real
        // IO error -> error dialog; a permission denial is already surfaced by
        // saveProject) instead of silently keeping this dialog open with no
        // feedback - matches every other save path (fix S4). The abort below still
        // keeps the project open so unsaved work is never dropped.
        surfaceSaveFailure(result?.error ?? null, openDialog);
        return false;
      }
      return true;
    } catch (e) {
      if (!current()) return false;
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
  const closeIntent: PendingExit = { kind: 'close', pendingOpenPath, pendingNewProject };
  const closeThen = (opts?: { releaseLock?: boolean }) => handBackProject(closeIntent, opts);
  const confirmDestination = async (current: () => boolean, localOnly = false) => {
    if (!pendingOpenPath) return true;
    try {
      if (!await confirmSpeleoFileOpen(pendingOpenPath, current, localOnly)) return false;
      if (!current()) return false;
      closeIntent.accessConfirmed = true;
      return true;
    } catch (error) {
      if (!current()) return false;
      openDialog('error', { title: 'Open Failed', message: getErrorMessage(error) });
      return false;
    }
  };

  const pushReleaseThenClose = () => runClose(async (current, request) => {
    closeIntent.request = request;
    closeIntent.sessionGeneration = useSpeleoStore.getState().sessionGeneration;
    if (needsMessage && !message.trim()) return;
    if (!await confirmDestination(current) || !current()) return;
    if (!(await maybeSaveLocally(current)) || !current()) return;
    // done(message) pushes (when dirty) and releases the lock. Only leave once the
    // lock is actually RELEASED - a divergence / unconfirmed push keeps us here so
    // the user resolves it rather than walking away still holding the lock (fix 2).
    //
    // [#303] ...and on an unwound copy it does not push at all: it raises its own
    // confirmation over this dialog and answers `null`. The close (and the Open / New
    // Project it may be standing in for) would be lost with this prompt, so the intent
    // goes with it and the confirmation completes it.
    const resolved = await done(message.trim() || null, { pendingExit: closeIntent, isCurrent: current });
    if (current() && resolved?.kind === 'released') {
      closeDialog();
      await closeThen();
    }
  });

  const keepLockAndClose = () => runClose(async (current, request) => {
    closeIntent.request = request;
    closeIntent.sessionGeneration = useSpeleoStore.getState().sessionGeneration;
    if (!await confirmDestination(current, true) || !current()) return;
    if (!await maybeSaveLocally(current) || !current()) return;
    closeDialog();
    await closeThen({ releaseLock: false });
  });

  const closeWithoutUpload = () => runClose(async (current, request) => {
    closeIntent.request = request;
    closeIntent.sessionGeneration = useSpeleoStore.getState().sessionGeneration;
    if (!await confirmDestination(current) || !current()) return;
    if (!(await maybeSaveLocally(current)) || !current()) return;
    if (holdsLock) {
      try {
        if (!await releaseSpeleoLock(current)) return;
      } catch (e) {
        if (!current()) return;
        openDialog('error', { title: t('speleodb.lock.releaseFailed'), message: getErrorMessage(e) });
        return;
      } finally { setSaving(false); }
    }
    if (!current()) return;
    closeDialog();
    await closeThen();
  });

  return (
    <DialogWrapper
      title={t('speleodb.close.title')}
      // Not dismissable mid-verb, for the same reason as the quit dialog: the
      // in-flight push/release outlives the dismissal and goes on to act on a
      // decision the user can no longer see.
      onClose={dismiss}
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
            title and buttons all say "close", and someone who pressed Ctrl+O or New
            Project needs to be told their request survived the prompt. */}
        {(pendingOpenPath || pendingNewProject) && (
          <p className="text-xs text-text-secondary" data-testid="speleodb-close-pending-note">
            {pendingOpenPath ? t('speleodb.close.thenOpen') : t('speleodb.close.thenNew')}
          </p>
        )}

        {isDirtyLocal ? (
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              disabled={busyAny}
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
        {holdsLock && needsMessage && (
          <div>
            <label htmlFor="speleodb-close-message" className="block text-sm text-text-secondary mb-1">{t('speleodb.done.messageLabel')}</label>
            <textarea id="speleodb-close-message" disabled={busyAny} value={message} onChange={(e) => setMessage(e.target.value)} rows={3} className="w-full px-2 py-1 bg-background border border-border rounded text-sm" data-testid="speleodb-close-message" />
          </div>
        )}
        <SpeleoPushFailureNotice />
        <div className="flex justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={dismiss} disabled={busyAny} data-testid="speleodb-close-cancel">
            {t('common.actions.cancel')}
          </Button>
          {holdsLock && (
            <Button variant="primary" onClick={() => void keepLockAndClose()} disabled={busyAny} data-primary-action="" data-testid="speleodb-close-keep-lock">
              Keep lock and close
            </Button>
          )}
          {holdsLock ? (
            <>
              <Button
                variant="secondary"
                onClick={() => void closeWithoutUpload()}
                disabled={busyAny}

                data-testid="speleodb-close-release"
              >
                {t('speleodb.close.releaseWithoutUpload')}
              </Button>
              <Button
                variant={unwound ? 'danger' : 'secondary'}
                onClick={() => void pushReleaseThenClose()}
                disabled={busyAny || (needsMessage && !message.trim())}
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
              onClick={() => void closeWithoutUpload()}
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
