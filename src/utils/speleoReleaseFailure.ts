/**
 * "Couldn't hand back the lock": never strand a SpeleoDB lock silently.
 *
 * Leaving a SpeleoDB project we hold the lock on - Close, a project switch (file
 * open, New Project, open from SpeleoDB) or a quit the app drives (menu Quit,
 * Ctrl+Q, the window X) - hands the lock back first, holder-checked, and the
 * backend reports what that came to on the reply of the call that released
 * (`CommandResult.lock_release`). A release that failed or is uncertain (the
 * holder could not be read, a transport error, a 5xx, a lost reply, a reply that
 * still names this account) closes nothing: the backend keeps the claim unbound,
 * so editing is already paused, and this prompt asks what to do:
 *
 * - **Retry** - the same holder-checked release again (the dialog stays up, with
 *   the new reason, while it keeps failing);
 * - **Close and keep the lock** - leave on purpose: the claim is kept, so no
 *   automatic release hands it back later, and the lock appears in the next
 *   sign-in's held-lock prompt;
 * - **Cancel** - stay in the project, read-only.
 *
 * Un-uploaded work never reaches this question: the exit prompt
 * (`utils/speleoExit`) asks about it first, and the backend never releases over
 * it. The bounded silent release at process exit is left only for the exits the
 * app cannot intercept.
 *
 * An independent overlay (its own store, mounted by App) like the exit prompt,
 * because the close it interrupts can be raised from inside another dialog, and
 * its callers await the answer. A second request while one is open replaces it
 * (the older caller gets Cancel); one arriving while a retry is in flight is
 * refused.
 */
import { create } from 'zustand';
import { useSpeleoStore } from '../store/speleoStore';
import type { CommandResult } from '../types/commands';
import { getErrorMessage, invokeIpc } from './ipc';

/** What the user is leaving the project for. Picks the wording only. */
export type ReleaseFailureReason = 'close' | 'switch' | 'quit';

/** How one Retry went: settled (released, not ours, or another outcome), or failed again. */
export type ReleaseRetryOutcome = { ok: true } | { ok: false; detail: string };

/** `settled`: a Retry settled the release. `keep`: Close and keep the lock. `cancel`: stay. */
export type ReleaseFailureAnswer = 'settled' | 'keep' | 'cancel';

export interface ReleaseFailureRequest {
  id: number;
  reason: ReleaseFailureReason;
  /** Why the release did not happen, as the backend said it. */
  detail: string;
  /** Run the release again; the dialog stays up while it fails. */
  retry: () => Promise<ReleaseRetryOutcome>;
  /** Finish the request. Idempotent; only the first call counts. */
  settle: (answer: ReleaseFailureAnswer) => void;
}

interface ReleaseFailureState {
  request: ReleaseFailureRequest | null;
  /** A Retry is in flight: a quit waits for it, and a new request is refused. */
  working: boolean;
}

export const useReleaseFailurePrompt = create<ReleaseFailureState>(() => ({ request: null, working: false }));

let nextRequestId = 0;

/** Raise the prompt and wait for the answer (see the module doc). */
export function askReleaseFailure(
  reason: ReleaseFailureReason,
  detail: string,
  retry: () => Promise<ReleaseRetryOutcome>,
): Promise<ReleaseFailureAnswer> {
  const { request, working } = useReleaseFailurePrompt.getState();
  if (working) return Promise.resolve('cancel');
  request?.settle('cancel');
  return new Promise<ReleaseFailureAnswer>((resolve) => {
    const id = ++nextRequestId;
    let settled = false;
    const settle = (answer: ReleaseFailureAnswer) => {
      if (settled) return;
      settled = true;
      if (useReleaseFailurePrompt.getState().request?.id === id) {
        useReleaseFailurePrompt.setState({ request: null, working: false });
      }
      resolve(answer);
    };
    useReleaseFailurePrompt.setState({ request: { id, reason, detail, retry, settle }, working: false });
  });
}

/** Test-only: drop any pending request without settling it. */
export function resetReleaseFailurePromptForTests(): void {
  useReleaseFailurePrompt.setState({ request: null, working: false });
}

/** The backend reported a release that failed or is uncertain. */
export function releaseFailed(result: CommandResult | null | undefined): boolean {
  return result?.lock_release === 'failed';
}

/** The error the callers of a close already treat as "the user backed out". */
export const RELEASE_CANCELLED = 'Cancelled';

function failure(error: string): CommandResult {
  return { success: false, warnings: [], error, section_ids: [] };
}

/**
 * Ask, and carry the answer out. `attempt(releaseLock)` is the call whose release
 * failed, made again: `true` for Retry (the holder-checked release), `false` for
 * Close and keep the lock. Resolves with the result the caller goes on from: a
 * success (released, not ours, or kept), a failure that is not about the lock
 * (shown by the caller as before), or a failure whose error is
 * {@link RELEASE_CANCELLED} - the user stays, with editing paused.
 */
export async function resolveReleaseFailure(
  reason: ReleaseFailureReason,
  failed: CommandResult,
  attempt: (releaseLock: boolean) => Promise<CommandResult>,
): Promise<CommandResult> {
  let settled: CommandResult = failed;
  const answer = await askReleaseFailure(reason, failed.error ?? '', async () => {
    let next: CommandResult;
    try {
      next = await attempt(true);
    } catch (error) {
      return { ok: false, detail: getErrorMessage(error) };
    }
    if (releaseFailed(next)) return { ok: false, detail: next.error ?? '' };
    settled = next;
    return { ok: true };
  });
  if (answer === 'settled') return settled;
  if (answer === 'cancel') return { ...failed, error: RELEASE_CANCELLED };
  try {
    return await attempt(false);
  } catch (error) {
    return failure(getErrorMessage(error));
  }
}

/**
 * The open project's editing is paused: the backend shut its edit gate when the
 * release ended (or failed to end) the claim. Mirror it now, and re-read the link,
 * without waiting for that read.
 */
export function pauseOpenEditing(): void {
  const { link } = useSpeleoStore.getState();
  if (link?.editState === 'editing') useSpeleoStore.setState({ link: { ...link, editState: 'read-only' } });
  void useSpeleoStore.getState().refreshLinkStatus();
}

/**
 * Before a project switch or a quit leaves the open project, settle its SpeleoDB
 * lock: hand it back (holder-checked), or - when that fails or is uncertain - ask
 * (Retry / Close and keep the lock / Cancel). `true` = go on; `false` = the user
 * cancelled (or the request went stale), nothing was left.
 *
 * `projectOpen` is the caller's view of whether a project is open at all; a
 * project the shell holds no SpeleoDB link for (a local one) asks the backend
 * nothing. A SpeleoDB project asks whatever its edit state: the backend decides
 * from its own claim (a failed release earlier leaves one with editing paused),
 * so a read-only project with no lock is a cheap local no-op.
 */
export async function settleOpenLockBeforeLeaving(
  reason: Exclude<ReleaseFailureReason, 'close'>,
  projectOpen: boolean,
  isCurrent: () => boolean = () => true,
): Promise<boolean> {
  if (!projectOpen || useSpeleoStore.getState().link === null) return true;
  const settle = (releaseLock: boolean) => invokeIpc<CommandResult>(
    'speleodb_settle_open_lock',
    { releaseLock },
    { purpose: releaseLock ? 'hand back the open project lock before leaving it' : 'keep the open project lock on purpose' },
  );
  let result: CommandResult;
  try {
    // A missing reply reports nothing, which is not "nothing to release".
    result = (await settle(true)) ?? { ...failure(''), lock_release: 'failed' };
  } catch (error) {
    // The reply never arrived: whether the lock went back is unknown.
    result = { ...failure(getErrorMessage(error)), lock_release: 'failed' };
  }
  if (result.lock_release) pauseOpenEditing();
  if (!releaseFailed(result)) return result.success === true;
  if (!isCurrent()) return false;
  return (await resolveReleaseFailure(reason, result, settle)).success;
}
