import { invokeIpc } from '../utils/ipc';

/**
 * Keeps the unsaved-changes state honest after a display toggle.
 *
 * A display toggle is durable at once on a version 2 file (the backend's
 * local-only save). On a 0.6.0 (SQLite) file not yet upgraded it cannot be:
 * the backend writes a legacy file only by converting it, which only a save
 * does, so the toggle waits in memory for that save and the backend's
 * `has_unsaved_changes` answers true (review finding D-migration-upgrade-5).
 * This mirrors that answer into the project store's `isDirty`, so the close,
 * switch and quit prompts ask, and autosave writes it, instead of losing the
 * toggle on a close nothing asked about.
 *
 * The project store registers the sink (this module imports no store, so the
 * UI store can call it without an import cycle). It only ever SETS the flag:
 * a clean backend answer says nothing about edits the frontend still tracks.
 */
export interface LocalStateDirtySink {
  /** Identifies the open project now (null: none open). */
  current: () => unknown;
  /** Mark the project unsaved if `token` still identifies the open project. */
  markUnsaved: (token: unknown) => void;
}

let sink: LocalStateDirtySink | null = null;

export function registerLocalStateDirtySink(next: LocalStateDirtySink): void {
  sink = next;
}

/** Ask the backend after a display setting was written (see above). */
export async function syncDirtyAfterLocalSave(): Promise<void> {
  const target = sink;
  if (target === null) return;
  const issuedFor = target.current();
  if (issuedFor === null) return;
  try {
    const dirty = await invokeIpc<boolean>('has_unsaved_changes', undefined, {
      purpose: 'keep the unsaved-changes state in step after a display toggle',
    });
    if (dirty === true) target.markUnsaved(issuedFor);
  } catch (e) {
    // The toggle itself landed (in memory at least); a failed question only
    // leaves the flag as it was.
    console.error('Failed to read the unsaved-changes flag:', e);
  }
}
