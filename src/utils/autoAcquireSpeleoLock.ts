import { create } from 'zustand';
import i18n from '../i18n';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore } from '../store/speleoStore';
import { invokeIpc } from './ipc';
import { humanizeSpeleoError, speleoErrorAction } from './speleoError';
import { applySpeleoTreeReplacement } from './speleoTreeReplacement';
import type { StartEditingResult } from '../types/ipc.generated';

interface ReadOnlyNotice {
  path: string;
  sessionGeneration: number;
  message: string;
}

// Independent of activeDialog: an open-project picker closes when its load
// finishes, and must not dismiss the acquisition result that follows it.
export const useSpeleoReadOnlyNotice = create<{ notice: ReadOnlyNotice | null }>(() => ({ notice: null }));

export function showSpeleoReadOnlyNotice(message: string): void {
  const path = useProjectStore.getState().project?.file_path;
  if (!path) return;
  useSpeleoReadOnlyNotice.setState({ notice: {
    path,
    sessionGeneration: useSpeleoStore.getState().sessionGeneration,
    message,
  } });
}

/** Called once after a new remote project load, never by a refresh or same-file reload. */
export async function autoAcquireSpeleoLock(path: string, current: () => boolean): Promise<void> {
  const generation = useSpeleoStore.getState().sessionGeneration;
  const stillCurrent = () => current() && useProjectStore.getState().project?.file_path === path &&
    useSpeleoStore.getState().sessionGeneration === generation;
  if (!stillCurrent()) return;
  const token = useSpeleoStore.getState().beginBusy();
  try {
    const result = await invokeIpc<StartEditingResult>('speleodb_start_editing');
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
    if (result.kind === 'locked_by') {
      // Opened with un-uploaded work of our own (a fast-forward copy, or "Open my
      // local copy"): say that it is safe, not just that the project is read-only.
      const keeps = useSpeleoStore.getState().link?.hasUnpushedEdits === true;
      const key = result.email
        ? (keeps ? 'speleoExitPrompt.readOnly.lockedByKeepsChanges' : 'speleodb.readOnlyNotice.lockedBy')
        : (keeps ? 'speleoExitPrompt.readOnly.unavailableKeepsChanges' : 'speleodb.readOnlyNotice.unavailable');
      showSpeleoReadOnlyNotice(i18n.t(key, { email: result.email }));
    }
    // Divergence already has its own resolution dialog. Successful acquisition is silent.
    void useSpeleoStore.getState().refreshLinkStatus();
  } catch (error) {
    if (!stillCurrent()) return;
    if (speleoErrorAction(error) === 'ReLogin') useSpeleoStore.getState().clearSession();
    showSpeleoReadOnlyNotice(humanizeSpeleoError(error, i18n.t('speleodb.readOnlyNotice.failed')));
  } finally {
    useSpeleoStore.getState().endBusy(token);
  }
}
