import { create } from 'zustand';
import i18n from '../i18n';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore } from '../store/speleoStore';
import { invokeIpc } from './ipc';
import { humanizeSpeleoError, lockMayBeHeld, speleoErrorAction } from './speleoError';
import { emailsMatch } from './email';
import { applySpeleoTreeReplacement } from './speleoTreeReplacement';
import type { StartEditingResult } from '../types/ipc.generated';

/**
 * What the notice reports, which picks its title: a project that just OPENED
 * read-only (the automatic Start Editing after an open did not get the lock), or
 * a Start Editing on a project already open that left it read-only (Resume
 * editing, Start editing; demo item 7: that one is not an open), or an edit
 * session a status refresh found over because SpeleoDB no longer gives this
 * account the lock (sdbdemo2 issue 4).
 */
export type ReadOnlyNoticeKind = 'opened' | 'stillReadOnly' | 'lockLost';

interface ReadOnlyNotice {
  path: string;
  sessionGeneration: number;
  message: string;
  kind: ReadOnlyNoticeKind;
}

// Independent of activeDialog: an open-project picker closes when its load
// finishes, and must not dismiss the acquisition result that follows it.
export const useSpeleoReadOnlyNotice = create<{ notice: ReadOnlyNotice | null }>(() => ({ notice: null }));

export function showSpeleoReadOnlyNotice(message: string, kind: ReadOnlyNoticeKind): void {
  const path = useProjectStore.getState().project?.file_path;
  if (!path) return;
  useSpeleoReadOnlyNotice.setState({ notice: {
    path,
    sessionGeneration: useSpeleoStore.getState().sessionGeneration,
    message,
    kind,
  } });
}

/**
 * The read-only notice's text for a failed Start Editing: `message`, and when the
 * refusal says this account may now hold the lock (`lockMayBeHeld`: a lock
 * request that may have reached SpeleoDB, which nothing could confirm), one
 * sentence saying so, so the close that later hands it back, or asks "Couldn't
 * hand back the lock", is not a surprise (demo item 3).
 */
export function startEditingFailureNotice(message: string, error: unknown): string {
  return lockMayBeHeld(error) ? `${message} ${i18n.t('speleodb.readOnlyNotice.lockMayBeHeld')}` : message;
}

/** Called once after a new remote project load, never by a refresh or same-file reload. */
export async function autoAcquireSpeleoLock(path: string, current: () => boolean): Promise<void> {
  const generation = useSpeleoStore.getState().sessionGeneration;
  const stillCurrent = () => current() && useProjectStore.getState().project?.file_path === path &&
    useSpeleoStore.getState().sessionGeneration === generation;
  if (!stillCurrent()) return;
  const token = useSpeleoStore.getState().beginBusy();
  try {
    // `afterOpen`: a lock this account already holds and this run never claimed
    // is reported, not taken over (sdbdemo2 issue 3): it may be the lock
    // Stalagmight on another computer with this account is editing under.
    const result = await invokeIpc<StartEditingResult>('speleodb_start_editing', { afterOpen: true });
    if (!stillCurrent()) return;
    if (result.kind === 'fast_forwarded') {
      const reconciled = await applySpeleoTreeReplacement({
        projectId: useSpeleoStore.getState().remoteProjectId,
        sessionGeneration: generation,
      }, true);
      if (!reconciled) return;
    }
    if (!stillCurrent()) return;
    useSpeleoStore.getState().applyStartEditing(result);
    if (result.kind === 'editing' || result.kind === 'fast_forwarded') {
      const id = useSpeleoStore.getState().remoteProjectId;
      useSpeleoStore.setState(state => {
        const heldLocks = state.heldLocks?.filter(lock => lock.projectId !== id) ?? [];
        return { heldLocks: heldLocks.length ? heldLocks : null,
          heldLockPromptOpen: heldLocks.length > 0 && state.heldLockPromptOpen };
      });
    }
    if (result.kind === 'locked_by' && emailsMatch(result.email, useSpeleoStore.getState().session?.userEmail ?? null)) {
      // This account's own lock, which this computer did not take: the same
      // words as the held-lock prompt, and Resume editing edits it here too.
      showSpeleoReadOnlyNotice(i18n.t('speleodb.readOnlyNotice.heldByYourAccount'), 'opened');
    } else if (result.kind === 'locked_by') {
      // Opened with un-uploaded work of our own (a fast-forward copy, or "Open my
      // local copy"): say that it is safe, not just that the project is read-only.
      const keeps = useSpeleoStore.getState().link?.hasUnpushedEdits === true;
      const key = result.email
        ? (keeps ? 'speleoExitPrompt.readOnly.lockedByKeepsChanges' : 'speleodb.readOnlyNotice.lockedBy')
        : (keeps ? 'speleoExitPrompt.readOnly.unavailableKeepsChanges' : 'speleodb.readOnlyNotice.unavailable');
      showSpeleoReadOnlyNotice(i18n.t(key, { email: result.email }), 'opened');
    }
    // Divergence already has its own resolution dialog. Successful acquisition is silent.
    void useSpeleoStore.getState().refreshLinkStatus();
  } catch (error) {
    if (!stillCurrent()) return;
    if (speleoErrorAction(error) === 'ReLogin') useSpeleoStore.getState().clearSession();
    showSpeleoReadOnlyNotice(startEditingFailureNotice(
      humanizeSpeleoError(error, i18n.t('speleodb.readOnlyNotice.failed')), error,
    ), 'opened');
  } finally {
    useSpeleoStore.getState().endBusy(token);
  }
}
