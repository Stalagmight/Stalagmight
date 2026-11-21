import { invokeIpc } from './ipc';
import { useProjectStore } from '../store/projectStore';

/**
 * How long an exit waits for the backend's unsaved-changes flag before it
 * asks the user anyway. The close guard asks it while it holds the close
 * listener's latch, so an IPC that never answers (a stalled backend) made the
 * window X dead for good (pre-PR delta review finding 1).
 */
export const UNSAVED_ASK_BOUND_MS = 2000;

/** The answer {@link askOpenProjectUnsaved} gives. */
export interface UnsavedAnswer {
  /** Ask the user before leaving. True too when the backend did not answer
   *  in time, the ask failed, or the project moved meanwhile (fail-safe: an
   *  extra question is cheap, silently dropped changes are not). */
  unsaved: boolean;
  /** The project the answer is about is still the open one: the same file,
   *  the same `created_at` and the same load (`loading.clientOpId`, which
   *  every open bumps), as `checkFileStatus` correlates. False after a load,
   *  a close or a switch landed during the ask: a caller must then not act
   *  on the answer (no prompt, no save, no close), because the project it
   *  asked about is gone. */
  stillOpen: boolean;
}

/** The open project, as an answer that crossed an await is checked against. */
interface OpenProjectIdentity {
  path: string | null;
  createdAt: string | null;
  opId: number;
}

function openProjectIdentity(): OpenProjectIdentity | null {
  const { project, loading } = useProjectStore.getState();
  if (!project) return null;
  return { path: project.file_path ?? null, createdAt: project.created_at ?? null, opId: loading.clientOpId };
}

function isStillOpen(asked: OpenProjectIdentity): boolean {
  const now = openProjectIdentity();
  return now !== null && now.path === asked.path && now.createdAt === asked.createdAt && now.opId === asked.opId;
}

/** The backend's flag, or `true` when it does not answer within the bound. */
function backendFlagWithinBound(): Promise<boolean> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const bound = new Promise<boolean>((resolve) => {
    timer = setTimeout(() => {
      console.warn(`The unsaved-changes flag did not answer within ${UNSAVED_ASK_BOUND_MS} ms; asking the user`);
      resolve(true);
    }, UNSAVED_ASK_BOUND_MS);
  });
  const asked = (async () => {
    try {
      const unsaved = await invokeIpc<boolean>('has_unsaved_changes', undefined, {
        purpose: 'ask before an exit drops changes only the backend tracks',
      });
      return unsaved === true;
    } catch (e) {
      console.error('Failed to read the unsaved-changes flag before leaving the project:', e);
      return true;
    }
  })();
  return Promise.race([asked, bound]).finally(() => clearTimeout(timer));
}

/**
 * Whether the open project has changes no save has written yet, asked at the
 * moment an exit decides: quit, close, opening another project (local or
 * remote), New Project, an account change, the saves "Keep the lock" and
 * "Quit now" make before they leave, and an explicit Save of a SpeleoDB copy
 * with nothing to upload. Every way out of a project asks THIS, so they all
 * keep the same rule (`exitGuardParity.test.tsx`).
 *
 * `isDirty` is the edits the frontend made. The backend's flag
 * (`has_unsaved_changes`) also counts state only the backend tracks: on a
 * project opened from a 0.6.0 file, the SpeleoDB bookkeeping that waits for
 * the converting save and that a close would lose (the SpeleoDB name fold,
 * link state, a pruned history; `local_pending_asks`), which 0.6.0 wrote in
 * place at once and a close without saving now drops (pre-PR review finding
 * 2). That state deliberately leaves `isDirty` alone: autosave would then
 * convert a 0.6.0 file the user only looked at.
 *
 * Asked at the decision rather than mirrored into the store after every IPC
 * that can set it: a mirror goes stale the first time a new bookkeeping path
 * forgets to refresh it, and the question is asked rarely. The ask is bounded
 * ({@link UNSAVED_ASK_BOUND_MS}) and correlated with the project it was asked
 * about ({@link UnsavedAnswer.stillOpen}).
 */
export async function askOpenProjectUnsaved(): Promise<UnsavedAnswer> {
  const { project, isDirty } = useProjectStore.getState();
  // Nothing open: nothing to ask about, and nothing that could move.
  if (!project) return { unsaved: false, stillOpen: true };
  if (isDirty) return { unsaved: true, stillOpen: true };
  const asked = openProjectIdentity();
  const unsaved = await backendFlagWithinBound();
  if (asked !== null && isStillOpen(asked)) return { unsaved, stillOpen: true };
  return { unsaved: true, stillOpen: false };
}
