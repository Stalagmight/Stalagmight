/**
 * The SpeleoDB exit prompt: the ONE question every way out of a SpeleoDB project
 * asks when we hold the write lock over work SpeleoDB has not seen.
 *
 * Why it exists: closing, switching or quitting used to hand the lock back while
 * the local copy still carried `has_unpushed_edits` (autosave had cleared the
 * unsaved flag, so nothing prompted). The next open then downloaded, refused to
 * overwrite the local copy and offered only "take the server copy and discard my
 * changes" - which the user experienced as a conflict with themselves.
 *
 * The prompt is an independent overlay (its own store, mounted by App), not a
 * `uiStore.activeDialog`: it is raised from flows that are themselves dialogs
 * (the SpeleoDB browser, an import wizard) and must not evict them, and its
 * callers AWAIT the answer rather than handing it a continuation.
 *
 * Contract of {@link askSpeleoExit}: it resolves `true` only once the user's
 * answer has been CARRIED OUT and the project is closed - uploaded and released,
 * kept (saved locally, lock held for the next open to resume), or discarded
 * (local copy removed, lock released). Anything else - Cancel, a failure the
 * user backed out of, a newer prompt replacing this one - resolves `false` with
 * the project still open, so the caller simply stops.
 *
 * The backend does not trust this prompt: a release over un-uploaded edits is
 * refused there unless the close carries the discard (see
 * `speleodb_lock_release.rs`). A path that cannot show UI therefore lands on the
 * data-safe answer - keep the lock and the local changes.
 */
import { create } from 'zustand';
import i18n from '../i18n';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../store/speleoStore';
import { getErrorMessage } from './ipc';
import { RELEASE_CANCELLED } from './speleoReleaseFailure';

/** What the user is trying to do. Picks the wording only - the choices are the same. */
export type SpeleoExitReason = 'close' | 'switch' | 'quit';

/**
 * The single predicate behind every exit path: we hold the write lock AND there
 * is work SpeleoDB has not seen - unsaved edits, or saved ones not uploaded yet.
 * A read-only project (no lock) gets the ordinary unsaved-changes handling; a
 * clean one is left silently, with the lock released as before.
 */
export function needsSpeleoExitPrompt(link: SpeleoLinkState | null, isDirty: boolean): boolean {
  return link?.editState === 'editing' && (isDirty || link.hasUnpushedEdits);
}

/** {@link needsSpeleoExitPrompt} for whatever is open right now. */
export function openProjectNeedsSpeleoExitPrompt(): boolean {
  const { project, isDirty } = useProjectStore.getState();
  return project !== null && needsSpeleoExitPrompt(useSpeleoStore.getState().link, isDirty);
}

export interface SpeleoExitRequest {
  id: number;
  reason: SpeleoExitReason;
  /** The project this prompt is about; an answer for another project is refused. */
  projectPath: string | null;
  /** Finish the request. Idempotent; only the first call counts. */
  settle: (closed: boolean) => void;
}

interface SpeleoExitPromptState {
  request: SpeleoExitRequest | null;
  /** An answer is being carried out (upload, save, close). A new request waits it out. */
  working: boolean;
}

export const useSpeleoExitPrompt = create<SpeleoExitPromptState>(() => ({ request: null, working: false }));

let nextRequestId = 0;

/**
 * Raise the exit prompt and wait for the user's answer to be carried out. See the
 * module doc for what `true` guarantees.
 *
 * A second request while one is open REPLACES it (the older caller gets `false`):
 * e.g. the window X pressed while a Close prompt is up means the user now wants
 * to quit. While an answer is being carried out the new request is refused
 * instead - the upload or close in flight decides what happens next.
 */
export function askSpeleoExit(reason: SpeleoExitReason): Promise<boolean> {
  const { request, working } = useSpeleoExitPrompt.getState();
  if (working) return Promise.resolve(false);
  request?.settle(false);
  const projectPath = useProjectStore.getState().project?.file_path ?? null;
  return new Promise<boolean>((resolve) => {
    const id = ++nextRequestId;
    let settled = false;
    const settle = (closed: boolean) => {
      if (settled) return;
      settled = true;
      if (useSpeleoExitPrompt.getState().request?.id === id) {
        useSpeleoExitPrompt.setState({ request: null, working: false });
      }
      resolve(closed);
    };
    useSpeleoExitPrompt.setState({ request: { id, reason, projectPath, settle }, working: false });
  });
}

/** Test-only: drop any pending request without settling it. */
export function resetSpeleoExitPromptForTests(): void {
  useSpeleoExitPrompt.setState({ request: null, working: false });
}

/** How carrying out an answer ended. `warning` is a non-fatal note for the user. */
export type ExitCloseOutcome = { ok: true; warning?: string } | { ok: false; error: string };

async function closeWith(opts: { releaseLock?: boolean; discardLocal?: boolean }): Promise<ExitCloseOutcome> {
  let result;
  try {
    result = await useProjectStore.getState().closeProject(opts);
  } catch (error) {
    return { ok: false, error: getErrorMessage(error, i18n.t('speleoExitPrompt.errors.closeFailed')) };
  }
  if (!result.success) {
    // The user stayed at "Couldn't hand back the lock": say so, not "Cancelled".
    if (result.error === RELEASE_CANCELLED) return { ok: false, error: i18n.t('speleoReleaseFailed.stillOpen') };
    return { ok: false, error: result.error || i18n.t('speleoExitPrompt.errors.closeFailed') };
  }
  // A close that happened but left something behind - the discard could not
  // remove the copy (lock kept), or the copy went but the lock could not be
  // released - is still a close; the user is told what was left.
  const warning = result.warnings?.[0]?.message;
  return warning ? { ok: true, warning } : { ok: true };
}

/**
 * "Keep the lock": put any unsaved edits on disk first - keeping the lock is
 * pointless if the work it protects is lost - then close WITHOUT releasing. The
 * next open finds the copy sitting on the server tip and resumes editing under
 * the same lock, with no prompt (`speleodb_local_fast_forward_open`).
 */
export async function closeKeepingLock(): Promise<ExitCloseOutcome> {
  const store = useProjectStore.getState();
  if (store.isDirty) {
    let saved;
    try {
      saved = await store.saveProject();
    } catch (error) {
      return { ok: false, error: getErrorMessage(error, i18n.t('speleoExitPrompt.errors.saveFailed')) };
    }
    if (!saved?.success) {
      return { ok: false, error: saved?.error || i18n.t('speleoExitPrompt.errors.saveFailed') };
    }
  }
  return closeWith({ releaseLock: false });
}

/** "Discard changes", confirmed: the backend removes the local copy, then releases. */
export function closeDiscardingLocal(): Promise<ExitCloseOutcome> {
  return closeWith({ discardLocal: true });
}

/** After a successful upload the copy is clean and the lock already released. */
export function closeAfterUpload(): Promise<ExitCloseOutcome> {
  return closeWith({});
}
