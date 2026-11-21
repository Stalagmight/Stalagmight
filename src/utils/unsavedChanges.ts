import { invokeIpc } from './ipc';
import { useProjectStore } from '../store/projectStore';

/**
 * Whether the open project has changes no save has written yet, asked at the
 * moment an exit decides: quit, close, opening another project (local or
 * remote), New Project, an account change, and the saves "Keep the lock" and
 * "Quit now" make before they leave. Every way out of a project asks THIS, so
 * they all keep the same rule (`exitGuardParity.test.tsx`).
 *
 * `isDirty` is the edits the frontend made. The backend's flag
 * (`has_unsaved_changes`) also counts state only the backend tracks: on a
 * project opened from a 0.6.0 file, the SpeleoDB bookkeeping that waits for
 * the converting save (an undo mode materialized by the open, the SpeleoDB
 * name fold, link state; `local_pending_asks`), which 0.6.0 wrote in place at
 * once and a close without saving now drops (pre-PR review finding 2). That
 * state deliberately leaves `isDirty` alone: autosave would then convert a
 * 0.6.0 file the user only looked at.
 *
 * Asked at the decision rather than mirrored into the store after every IPC
 * that can set it: a mirror goes stale the first time a new bookkeeping path
 * forgets to refresh it, and the question is asked rarely. A failed ask
 * answers true: an extra question is cheap, silently dropped changes are not.
 */
export async function openProjectHasUnsavedChanges(): Promise<boolean> {
  const { project, isDirty } = useProjectStore.getState();
  if (!project) return false;
  if (isDirty) return true;
  try {
    const unsaved = await invokeIpc<boolean>('has_unsaved_changes', undefined, {
      purpose: 'ask before an exit drops changes only the backend tracks',
    });
    return unsaved === true;
  } catch (e) {
    console.error('Failed to read the unsaved-changes flag before leaving the project:', e);
    return true;
  }
}
