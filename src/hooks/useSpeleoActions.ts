import { showSpeleoReadOnlyNotice } from '../utils/autoAcquireSpeleoLock';
import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../store/uiStore';
import { useProjectStore } from '../store/projectStore';
import {
  useSpeleoStore,
  asPushKind,
  asDoneKind,
  isUnwoundNotEdited,
  speleoProjectSessionIssuedFor,
  speleoProjectSessionStillCurrent,
} from '../store/speleoStore';
import { invokeIpc } from '../utils/ipc';
import { openExitPrompt, type PendingExit } from '../utils/quit';
import { speleoErrorAction, humanizeSpeleoError } from '../utils/speleoError';
import { applySpeleoTreeReplacement } from '../utils/speleoTreeReplacement';
import { confirmSpeleoReloadDiscard } from '../utils/speleoReloadConfirm';
import { TOAST_KEYS } from '../utils/toastKeys';
import type {
  DoneResult,
  PushResult,
  SpeleoFileAccess,
  StartEditingResult,
  UpdateResult,
} from '../types/ipc.generated';

/**
 * The single-writer hand-off verbs (#204), with backend-result routing. Keeps
 * the SpeleoEditBar / SpeleoDoneDialog presentational: each verb invokes the IPC,
 * feeds the result to the shared `speleoStore` (which owns the edit-state
 * transitions + #198 divergence / #200 banner seams), reloads the project on a
 * fast-forward (the backend swapped the tree in place), and surfaces outcomes as
 * toasts. A `ReLogin` error drops the session so the bar collapses to read-only.
 */
export interface SpeleoActions {
  busy: boolean;
  /** Acquire the write lock + reconcile. Resolves `true` only when we actually
      hold the lock afterwards AND the view is showing it (`editing`, or
      `fast_forwarded` whose reload landed); `false` on a contended lock, a
      reported divergence, a reload that was superseded or failed, or a thrown
      call - all of which already surface their own toast / routing. Callers that
      hand the shell over on success (the held-lock prompt) MUST check it:
      treating a refusal as success there dismisses the prompt and discards the
      remaining locks. */
  startEditing: () => Promise<boolean>;
  /** Discard the local copy and route the optional Start Editing result returned
      when the caller asks to keep editing. The discard error is left to the caller. */
  discardChanges: (
    keepLock: boolean,
  ) => Promise<'editing' | 'released' | 'discarded_without_lock' | 'stale'>;
  update: () => Promise<void>;
  /** Push edits without releasing the lock (T2d); uploading UI requires a message.
      Returns the backend outcome (or `null` if the call threw) so callers can keep
      a dialog open on a `divergence_needed` / `push_unconfirmed` result. */
  push: (
    message?: string | null,
    opts?: { acceptOlderVersion?: boolean; isCurrent?: () => boolean },
  ) => Promise<PushResult | null>;
  /** The backend outcome, or `null` if the call threw. Callers key off `.kind`
      (e.g. the close flow only leaves once the lock is actually `released`).
      [#303] Also `null` - having raised its own confirmation instead - when the copy is
      dirty only because a chain step walked it back to an older version, since pushing
      that publishes a revert rather than saving work. `acceptOlderVersion` is that
      confirmation reporting the user chose it knowingly.
      [#303] `pendingExit` is for the callers whose CONTINUATION is the thing at risk when
      that happens: an exit prompt reads the `null` as "did not release" and stays put,
      but it has already been evicted from uiStore's single dialog slot, so the quit or
      close the user asked for is simply gone. Handing the intent over lets the
      confirmation finish it. */
  done: (
    message?: string | null,
    opts?: { acceptOlderVersion?: boolean; pendingExit?: PendingExit; isCurrent?: () => boolean },
  ) => Promise<DoneResult | null>;
  /** #198 "Take remote": DISCARD local unpushed edits and reset to the server
      tip. Returns `true` once the tree is reloaded, `false` if the call threw.
      The caller must gate this behind a destroy-confirmation. */
  takeRemote: () => Promise<boolean>;
}

export function useSpeleoActions(): SpeleoActions {
  const { t } = useTranslation();
  // Shared busy so every SpeleoDB entry point gates on the same in-flight lock and
  // can't double-issue a verb (fix 12).
  const busy = useSpeleoStore((s) => s.busy);
  // Claim/release rather than a bare `setBusy(false)`: a close or a rolled-back
  // remote open resets the shared flag out of band, and an unowned release in a
  // verb's `finally` would clear it for whatever started after (#270).
  const beginBusy = useSpeleoStore((s) => s.beginBusy);
  const endBusy = useSpeleoStore((s) => s.endBusy);
  const showToast = useUIStore((s) => s.showToast);
  const openDialog = useUIStore((s) => s.openDialog);
  const loadProject = useProjectStore((s) => s.loadProject);
  const applyStartEditing = useSpeleoStore((s) => s.applyStartEditing);
  const applyUpdate = useSpeleoStore((s) => s.applyUpdate);
  const applyDone = useSpeleoStore((s) => s.applyDone);
  const applyPush = useSpeleoStore((s) => s.applyPush);
  const clearSession = useSpeleoStore((s) => s.clearSession);
  const refreshLinkStatus = useSpeleoStore((s) => s.refreshLinkStatus);
  const raiseNoAnchorBanner = useSpeleoStore((s) => s.raiseNoAnchorBanner);
  const raisePushFailedBanner = useSpeleoStore((s) => s.raisePushFailedBanner);
  const clearPushFailedBanner = useSpeleoStore((s) => s.clearPushFailedBanner);

  // #200 (D2): a push/unlock of a survey with NO georeferenced zero datum means the
  // server generates no new web-viewer GeoJSON. Surface the non-blocking notice; the
  // push itself already succeeded and is never blocked. Checked from the local
  // project (the same zero-datum set export_tml roots the .tml anchor on).
  const noteNoAnchorIfNeeded = useCallback(() => {
    if ((useProjectStore.getState().project?.zero_datums.length ?? 0) === 0) {
      raiseNoAnchorBanner();
    }
  }, [raiseNoAnchorBanner]);

  // A confirmed push/unlock already flushed the survey to disk (the backend's
  // speleodb_upload runs save_project before the upload), but the projectStore's
  // `isDirty` flag is a separate concept that stays set until the FE saves. Left
  // alone it clears only on the 30s autosave debounce, so exiting in that window
  // fires a false "Unsaved Changes" prompt for edits that are already on disk (and
  // pushed to SpeleoDB). Re-flush from the FE now to clear `isDirty` immediately;
  // saveProject writes the current in-memory state, so a late edit is captured too.
  // Best-effort and quiet (autosave semantics): a failed flush leaves `isDirty` set,
  // which is the safe direction since the push already persisted the same edits.
  const flushToDiskAfterPush = useCallback(async () => {
    try {
      await useProjectStore.getState().saveProject(undefined, true);
    } catch {
      /* best-effort; isDirty stays set, which safely re-prompts on exit */
    }
  }, []);

  /** Humanize the failure and apply its session side effect (a dead token
      collapses the shell to read-only) WITHOUT toasting - for push/done, whose
      failures get a durable surface instead (the push_failed banner, and the
      inline notice inside the Speleo dialogs). One event, one surface
      (docs/notifications.md); the toast-and-banner pair predates the banner. */
  const humanizeError = useCallback(
    (e: unknown, prefixKey: string): string => {
      if (speleoErrorAction(e) === 'ReLogin') clearSession();
      return humanizeSpeleoError(e, t(prefixKey));
    },
    [clearSession, t],
  );

  /** Toast the failure - for the transient verbs (start/update/force/take-remote)
      that have no durable surface of their own. */
  const handleError = useCallback(
    (e: unknown, prefixKey: string): string => {
      const message = humanizeError(e, prefixKey);
      showToast(message, TOAST_KEYS.SPELEODB_ACTION_ERROR, 'error');
      return message;
    },
    [humanizeError, showToast],
  );

  const routeStartEditingResult = useCallback(
    async (
      result: StartEditingResult,
      showEditingToast: boolean,
      issuedFor: ReturnType<typeof speleoProjectSessionIssuedFor>,
    ): Promise<{ editing: boolean; treeReplaced: boolean; landed: boolean }> => {
      if (!speleoProjectSessionStillCurrent(issuedFor)) {
        return { editing: false, treeReplaced: false, landed: false };
      }
      applyStartEditing(result);
      switch (result.kind) {
        case 'editing':
          if (showEditingToast) {
            showToast(t('speleodb.editBar.toasts.editing'), TOAST_KEYS.SPELEODB_EDITING, 'success');
          }
          return { editing: true, treeReplaced: false, landed: true };
        case 'fast_forwarded': {
          const loaded = await loadProject(result.local_path, { acquireLock: false });
          if (!speleoProjectSessionStillCurrent(issuedFor)) {
            return { editing: false, treeReplaced: false, landed: false };
          }
          return {
            editing: loaded.success,
            treeReplaced: true,
            landed: loaded.success,
          };
        }
        case 'locked_by':
          showSpeleoReadOnlyNotice(t(result.email ? 'speleodb.readOnlyNotice.lockedBy' : 'speleodb.readOnlyNotice.unavailable', { email: result.email }));
          return { editing: false, treeReplaced: false, landed: true };
        case 'divergence_needed':
          return { editing: false, treeReplaced: false, landed: true };
        default: {
          const _exhaustive: never = result;
          void _exhaustive;
          console.warn('[speleodb] unhandled start-editing result:', result);
          return { editing: false, treeReplaced: false, landed: false };
        }
      }
    },
    [applyStartEditing, loadProject, showToast, t],
  );

  const startEditing = useCallback(async (): Promise<boolean> => {
    const issuedFor = speleoProjectSessionIssuedFor(
      useSpeleoStore.getState().remoteProjectId,
    );
    const busyToken = beginBusy();
    try {
      const result = await invokeIpc<StartEditingResult>('speleodb_start_editing');
      const routed = await routeStartEditingResult(result, true, issuedFor);
      if (!routed.landed) return false;
      if (routed.treeReplaced && !(await applySpeleoTreeReplacement(issuedFor, true))) {
        return false;
      }
      return routed.editing;
    } catch (e) {
      if (speleoProjectSessionStillCurrent(issuedFor)) {
        showSpeleoReadOnlyNotice(humanizeError(e, 'speleodb.readOnlyNotice.failed'));
      }
      return false;
    } finally {
      endBusy(busyToken);
    }
  }, [routeStartEditingResult, humanizeError, beginBusy, endBusy]);

  const discardChanges = useCallback(
    async (
      keepLock: boolean,
    ): Promise<'editing' | 'released' | 'discarded_without_lock' | 'stale'> => {
      const issuedFor = speleoProjectSessionIssuedFor(
        useSpeleoStore.getState().remoteProjectId,
      );
      const busyToken = beginBusy();
      try {
        const result = await invokeIpc<StartEditingResult | null>('speleodb_discard_changes', {
          keepLock,
          acknowledged: true,
        });
        const routed = result
          ? await routeStartEditingResult(result, false, issuedFor)
          : null;
        if (routed && !routed.landed) return 'stale';
        // [#473] ALWAYS a tree replacement, whatever `result` is. Discard runs
        // `speleodb_return_to_tip_discarding` - "replace the local copy with the server
        // tip, throwing away whatever it held" - UNCONDITIONALLY, and only afterwards
        // decides whether to hand the lock back (`speleodb_push.rs`). So a `null` result
        // means "released AND swapped", not "nothing happened": gating on `result !== null`
        // here skipped the discarded-history re-read on the very flow that replaced the
        // bytes, which is silent loss of the banner in one direction and a stale banner
        // over the new tip in the other.
        if (!(await applySpeleoTreeReplacement(issuedFor, true))) return 'stale';
        if (!result) return 'released';
        return routed?.editing ? 'editing' : 'discarded_without_lock';
      } finally {
        endBusy(busyToken);
      }
    },
    [routeStartEditingResult, beginBusy, endBusy],
  );

  const update = useCallback(async () => {
    const issuedFor = speleoProjectSessionIssuedFor(
      useSpeleoStore.getState().remoteProjectId,
    );
    const project = useProjectStore.getState();
    const path = project.project?.file_path;
    const tree = project.projectTreeGeneration;
    const current = () => speleoProjectSessionStillCurrent(issuedFor) &&
      useProjectStore.getState().project?.file_path === path &&
      useProjectStore.getState().projectTreeGeneration === tree;
    const pending = project.isDirty || useSpeleoStore.getState().link?.hasUnpushedEdits;
    const busyToken = beginBusy();
    try {
      let discardChanges = false;
      if (pending) {
        discardChanges = await confirmSpeleoReloadDiscard();
        if (!discardChanges || !current()) return;
      }
      if (!current() || !path) return;
      const access = await invokeIpc<SpeleoFileAccess | null>('speleodb_project_access_at', { path, localOnly: true });
      if (!current()) return;
      if (!access?.release_context) throw new Error('The project or account changed. Try again.');
      const result = await invokeIpc<UpdateResult>('speleodb_update', { discardChanges, expected: access.release_context });
      if (!current()) return;
      applyUpdate(result);
      switch (result.kind) {
        case 'up_to_date':
          // Revision equality says nothing about changed locks or permissions.
          // Manual refresh must refresh those too now that there is no timer.
          await refreshLinkStatus();
          if (!speleoProjectSessionStillCurrent(issuedFor)) return;
          showToast(t('speleodb.editBar.toasts.upToDate'), TOAST_KEYS.SPELEODB_UP_TO_DATE, 'info');
          break;
        case 'fast_forwarded':
          // See startEditing: the FF reload swapped the tree in the backend, but
          // loadProject short-circuits on the reopened path - pull geometry + link
          // state in so Update actually resets the view to the new tip. Clear the stale
          // selection for the same reason as the startEditing arm.
          {
            // Same staleness stop as the startEditing arm: a superseded (or
            // failed) reload means the swapped tree is not on screen, so the
            // unguarded `refreshProjectState` below - which overwrites
            // project/stations/surveyData wholesale - must not paint this
            // project's state over whatever the user actually opened.
            const loaded = await loadProject(result.local_path, { acquireLock: false });
            if (!loaded.success) break;
          }
          await applySpeleoTreeReplacement(issuedFor, true);
          break;
        case 'divergence_needed':
          break;
      }
    } catch (e) {
      if (current()) handleError(e, 'speleodb.editBar.toasts.updateFailed');
    } finally {
      endBusy(busyToken);
    }
  }, [applyUpdate, loadProject, refreshLinkStatus, showToast, t, handleError, beginBusy, endBusy]);

  const push = useCallback(
    async (
      message?: string | null,
      opts?: { acceptOlderVersion?: boolean; isCurrent?: () => boolean },
    ): Promise<PushResult | null> => {
      if (!opts?.acceptOlderVersion && isUnwoundNotEdited(useSpeleoStore.getState().link)) {
        openDialog('speleodb-unwound-push', { message: message ?? null, mode: 'push' });
        return null;
      }
      const issuedFor = speleoProjectSessionIssuedFor(useSpeleoStore.getState().remoteProjectId);
      const path = useProjectStore.getState().project?.file_path;
      const operation = useProjectStore.getState().loading.clientOpId;
      const current = () => speleoProjectSessionStillCurrent(issuedFor) &&
        useProjectStore.getState().project?.file_path === path &&
        useProjectStore.getState().loading.clientOpId === operation && (opts?.isCurrent?.() ?? true);
      const treeGeneration = useProjectStore.getState().projectTreeGeneration;
      const busyToken = beginBusy();
      // A fresh attempt supersedes the previous failure banner (#260); a landed
      // push clears the whole slot via applyPush, and any outcome that leaves the
      // work on this computer re-raises it - a throw below, or the `not_editing`
      // arm, which neither lands nor throws.
      clearPushFailedBanner();
      try {
        let result = await invokeIpc<PushResult>('speleodb_push', {
          message: message ?? null,
          acceptOlderVersion: opts?.acceptOlderVersion ?? false,
        });
        if (!current()) return null;
        // An edit can land after the backend decides the snapshot is clean but
        // before this reply arrives. Keep those edits pending in the frontend too.
        if (result.kind === 'pushed' && useProjectStore.getState().projectTreeGeneration !== treeGeneration) {
          result = { kind: 'push_unconfirmed' };
        }
        applyPush(result);
        const kind = asPushKind(result.kind);
        if (kind === 'pushed') {
          // sdk#3: the upload left its undo history out to fit the server's size
          // limit. Same event, one surface: the success toast becomes a caveat.
          if (result.kind === 'pushed' && result.undo_history_omitted) {
            showToast(t('speleodb.editBar.toasts.pushedWithoutHistory'), TOAST_KEYS.SPELEODB_PUSHED, 'warning');
          } else {
            showToast(t('speleodb.editBar.toasts.pushed'), TOAST_KEYS.SPELEODB_PUSHED, 'success');
          }
          noteNoAnchorIfNeeded();
          await flushToDiskAfterPush();
        } else if (kind === 'not_editing') {
          // Nothing was uploaded: the edit gate was already closed (a 401 auto-drop
          // is the common way in, and the push-failure banner's Retry is the common
          // way to re-issue from there). A green "Changes pushed to SpeleoDB." here
          // was a lie that also cost the flush + no-anchor check their meaning, so
          // say what actually happened and name the way out.
          showToast(t('speleodb.editBar.toasts.notEditing'), TOAST_KEYS.SPELEODB_NOT_EDITING, 'warning');
          // ...and keep a DURABLE surface for it. This arm neither lands nor
          // throws, so nothing else re-raises the failure the attempt started by
          // clearing - and the toast is gone in 3 s. Since the usual way here is
          // the banner's own Retry, that left the user staring at the same button
          // with the error text wiped (inside `SpeleoDoneDialog`, which stays open
          // on any kind but `released`, it blanked the inline notice too). Re-raise
          // with what actually happened, carrying their commit message so the next
          // Retry replays it verbatim.
          raisePushFailedBanner({
            message: t('speleodb.editBar.toasts.notEditing'),
            origin: 'push',
            pushMessage: message ?? null,
          });
        }
        // divergence_needed / push_unconfirmed surface via the store (#198/#200 seams);
        // the returned kind lets the dialog stay open to resolve them.
        return result;
      } catch (e) {
        if (!current()) return null;
        // Banner only (durable; inline notice inside the Speleo dialogs while
        // one is open). The old extra toast was pre-#260 redundancy.
        const humanized = humanizeError(e, 'speleodb.editBar.toasts.pushFailed');
        raisePushFailedBanner({
          message: humanized,
          origin: 'push',
          pushMessage: message ?? null,
        });
        return null;
      } finally {
        endBusy(busyToken);
      }
    },
    [
      applyPush,
      showToast,
      t,
      humanizeError,
      beginBusy,
      endBusy,
      noteNoAnchorIfNeeded,
      flushToDiskAfterPush,
      raisePushFailedBanner,
      clearPushFailedBanner,
      openDialog,
    ],
  );

  const done = useCallback(
    async (
      message?: string | null,
      opts?: { acceptOlderVersion?: boolean; pendingExit?: PendingExit; isCurrent?: () => boolean },
    ): Promise<DoneResult | null> => {
      // [#303] The copy is dirty ONLY because a chain step walked it back to an older
      // version - the user has edited nothing. Pushing here does not save anyone's work,
      // it records the OLD survey over the newer one for every collaborator, and that is
      // not what "finish editing" means to the person pressing it.
      //
      // Guarded HERE rather than in each caller because every route to a push comes
      // through this hook - the upload dialog (from the SpeleoDB tab, Save or a banner),
      // the close prompt and the quit prompt - and the trap was that one of them made
      // pushing the primary action.
      // `acceptOlderVersion` is the confirmation reporting that the user was shown what it
      // does and chose it anyway.
      if (!opts?.acceptOlderVersion && isUnwoundNotEdited(useSpeleoStore.getState().link)) {
        const data = { message: message ?? null };
        if (opts?.pendingExit) {
          // Raised out of an exit prompt, so it inherits that prompt's obligations: this
          // call EVICTS the prompt from uiStore's single `activeDialog` slot, and
          // `openExitPrompt` is what records the eviction so Cancel can put it back. The
          // intent rides along so "Upload this older version" ends in the quit / close the
          // user actually asked for, instead of a released lock and a window still open.
          openExitPrompt('speleodb-unwound-push', { ...data, pendingExit: opts.pendingExit });
        } else {
          // Nothing is waiting on the outcome (the SpeleoDB tab): the
          // confirmation is the whole interaction, exactly as it shipped.
          openDialog('speleodb-unwound-push', data);
        }
        // Null, not a result: the caller is mid-flow (the close prompt only leaves once a
        // push RELEASES) and must stay put while the confirmation is up.
        return null;
      }
      const issuedFor = speleoProjectSessionIssuedFor(useSpeleoStore.getState().remoteProjectId);
      const path = useProjectStore.getState().project?.file_path;
      const operation = useProjectStore.getState().loading.clientOpId;
      const treeGeneration = useProjectStore.getState().projectTreeGeneration;
      const current = () => speleoProjectSessionStillCurrent(issuedFor) &&
        useProjectStore.getState().project?.file_path === path &&
        useProjectStore.getState().loading.clientOpId === operation && (opts?.isCurrent?.() ?? true);
      const busyToken = beginBusy();
      clearPushFailedBanner();
      // #260: "Changes uploaded and the write lock released." lied whenever there was nothing to
      // upload (releasing a lock taken but never edited against). The remote dirty bit
      // is the one that means "local edits not yet on the server" - projectStore's
      // `isDirty` is the unrelated unsaved-to-disk flag - and `done` clears it, so read
      // it BEFORE the call.
      const hadUnpushedEdits = useSpeleoStore.getState().link?.hasUnpushedEdits ?? false;
      try {
        const result = await invokeIpc<DoneResult>('speleodb_done', {
          message: message ?? null,
          acceptOlderVersion: opts?.acceptOlderVersion ?? false,
        });
        if (!current()) return null;
        if (result.kind === 'released' && useProjectStore.getState().projectTreeGeneration !== treeGeneration) {
          // The release happened, but newer frontend work is not covered by that
          // reply. Preserve it without pretending the already-released lock is held.
          applyDone(result);
          useSpeleoStore.getState().markDirty();
          raisePushFailedBanner({
            message: 'Newer changes remain on this computer. The editing lock was released; resume editing before uploading them.',
            origin: 'done', pushMessage: message ?? null,
          });
          return { kind: 'push_unconfirmed' };
        }
        applyDone(result);
        switch (asDoneKind(result.kind)) {
          case 'released':
            // sdk#3: see `push` - an upload that left its undo history out says so.
            if (result.kind === 'released' && result.undo_history_omitted) {
              showToast(t('speleodb.editBar.toasts.releasedWithoutHistory'), TOAST_KEYS.SPELEODB_RELEASED, 'warning');
            } else {
              showToast(
                hadUnpushedEdits
                  ? t('speleodb.editBar.toasts.released')
                  : t('speleodb.editBar.toasts.releasedNothingToUpload'),
                TOAST_KEYS.SPELEODB_RELEASED,
                'success',
              );
            }
            noteNoAnchorIfNeeded();
            await flushToDiskAfterPush();
            break;
          case 'push_unconfirmed':
            // #200 seam: applyDone raised the push_unconfirmed banner.
            break;
          case 'divergence_needed':
            // #198 seam: applyDone recorded the divergence origin.
            break;
          case 'not_editing':
            // No lock was held, so nothing was uploaded AND nothing was released.
            // `applyDone` deliberately left the dirty bit alone; callers key off
            // `kind === 'released'`, so the close/quit dialogs correctly stay open.
            showToast(t('speleodb.editBar.toasts.notEditing'), TOAST_KEYS.SPELEODB_NOT_EDITING, 'warning');
            // See the same arm in `push`: neither landed nor threw, so re-raise the
            // durable banner this attempt cleared on its way in - otherwise the only
            // trace of a release that released nothing is a 3 s toast, and the
            // dialogs that stay open here would show their retry button with no
            // error text beside it.
            raisePushFailedBanner({
              message: t('speleodb.editBar.toasts.notEditing'),
              origin: 'done',
              pushMessage: message ?? null,
            });
            break;
        }
        return result;
      } catch (e) {
        if (!current()) return null;
        const humanized = humanizeError(e, 'speleodb.editBar.toasts.doneFailed');
        raisePushFailedBanner({
          message: humanized,
          origin: 'done',
          pushMessage: message ?? null,
        });
        return null;
      } finally {
        endBusy(busyToken);
      }
    },
    [
      applyDone,
      showToast,
      t,
      humanizeError,
      beginBusy,
      endBusy,
      noteNoAnchorIfNeeded,
      flushToDiskAfterPush,
      raisePushFailedBanner,
      clearPushFailedBanner,
    ],
  );

  const takeRemote = useCallback(async (): Promise<boolean> => {
    const issuedFor = speleoProjectSessionIssuedFor(
      useSpeleoStore.getState().remoteProjectId,
    );
    const busyToken = beginBusy();
    try {
      // Backend releases the lock, re-pulls + resets the tree to the server tip and
      // returns the reloaded path; loadProject re-seeds the link status (→ read-only:
      // the backend's edit gate is the lock claim, and Take remote handed it back).
      const localPath = await invokeIpc<string>('speleodb_take_remote');
      // If the reload was superseded (the user opened another project mid-pull),
      // don't claim success: the backend reset already happened, but the view the
      // caller thinks it reloaded is not showing. Skip the toast + return false so
      // the divergence dialog doesn't close over a project the user left.
      //
      // `acquireLock: false`, like every other in-place reload: the user chose to
      // end the edit session, so the reopen must not take the lock straight back
      // (data#3). They resume with Resume editing.
      const { success } = await loadProject(localPath, { acquireLock: false });
      if (!success) return false;
      if (!(await applySpeleoTreeReplacement(issuedFor, true))) return false;
      showToast(t('speleodb.editBar.toasts.tookRemote'), TOAST_KEYS.SPELEODB_TOOK_REMOTE, 'success');
      return true;
    } catch (e) {
      handleError(e, 'speleodb.editBar.toasts.takeRemoteFailed');
      return false;
    } finally {
      endBusy(busyToken);
    }
  }, [loadProject, showToast, t, handleError, beginBusy, endBusy]);

  return { busy, startEditing, discardChanges, update, push, done, takeRemote };
}
