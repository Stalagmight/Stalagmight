/**
 * When autosave must NOT write the open project (#576 D34).
 *
 * A "leave this project" prompt (Unsaved Changes, the SpeleoDB hand-back
 * prompts) is the user deciding what happens to the unsaved edits. Autosave used
 * to keep its debounce running underneath, so the edits could land on disk while
 * the prompt was still asking - and after "Discard Changes" the same timer kept
 * running while the user named the new project or the next one loaded, writing
 * the edits they had just discarded into the old file.
 *
 * Two sources hold autosave, and `useAutoSave` honours both when it arms and
 * again when the timer fires; when the last hold goes it re-arms a full interval
 * if the project is still dirty (D34-D2, "Pause while prompt open"):
 *
 * - a leave prompt on screen, derived from the `activeDialog` slot so every way
 *   of dismissing it (Cancel, X, Escape, backdrop, being replaced) ends the hold;
 * - an explicit hold, taken for the switch a discard starts (closing, loading the
 *   next project, quitting) and released however that switch ends.
 *
 * Discard therefore takes effect only once the switch lands. Backing out of New
 * Project, or a load that fails, leaves the project open with its edits, and
 * autosave picks them up again (D34-D1, "Keep edits, resume autosave").
 */
import { create } from 'zustand';
import { useUIStore } from '../store/uiStore';

/**
 * The prompts that ask whether to leave the open project. `new-project` is one
 * because it only opens over a DIRTY project after "Discard Changes"
 * (`newProjectGuarded` routes a dirty project through Unsaved Changes first);
 * over a clean one there is nothing to hold.
 */
const LEAVE_PROMPTS: ReadonlySet<string> = new Set([
  'unsaved-changes',
  'speleodb-close',
  'speleodb-quit',
  'new-project',
]);

interface AutosaveHoldState {
  holds: number;
}

export const useAutosaveHoldStore = create<AutosaveHoldState>(() => ({ holds: 0 }));

/** Whether `activeDialog` is a prompt about leaving the open project. */
export function isLeavePromptOpen(activeDialog: string | null, dialogData: Record<string, unknown>): boolean {
  if (activeDialog === null) return false;
  if (LEAVE_PROMPTS.has(activeDialog)) return true;
  // The unwound-push confirmation raised OUT of an exit prompt carries that exit
  // (`pendingExit`, see utils/quit) and is still the same decision.
  return activeDialog === 'speleodb-unwound-push' && dialogData.pendingExit != null;
}

/** Whether autosave is held right now, by a leave prompt or an explicit hold. */
export function isAutosaveHeld(): boolean {
  if (useAutosaveHoldStore.getState().holds > 0) return true;
  const { activeDialog, dialogData } = useUIStore.getState();
  return isLeavePromptOpen(activeDialog, dialogData);
}

/** Hold autosave until the returned release is called (idempotent). */
export function holdAutosave(): () => void {
  useAutosaveHoldStore.setState((s) => ({ holds: s.holds + 1 }));
  let released = false;
  return () => {
    if (released) return;
    released = true;
    useAutosaveHoldStore.setState((s) => ({ holds: Math.max(0, s.holds - 1) }));
  };
}

/** Run `work` with autosave held, releasing the hold however `work` ends. */
export async function withAutosaveHeld<T>(work: () => Promise<T>): Promise<T> {
  const release = holdAutosave();
  try {
    return await work();
  } finally {
    release();
  }
}
