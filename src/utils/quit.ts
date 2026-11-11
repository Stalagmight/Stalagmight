import { isProjectMutationCurrent, isProjectRequestCurrent, reserveProjectRequest } from './projectTransition';
import { confirmSpeleoFileOpen } from './confirmSpeleoOpen';
/**
 * Quit entry point, shared by every affordance (GlobalView menu, in-project
 * menu, the Ctrl+Q shortcut in both shortcut hooks, AND the OS window close
 * button) so the exit guards + close contract live in exactly one place.
 */
import { getCurrentWindow } from '@tauri-apps/api/window';
import i18n from '../i18n';
import { getErrorMessage, invokeIpc } from './ipc';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
import { askSpeleoExit, closeKeepingLock, openProjectNeedsSpeleoExitPrompt } from './speleoExit';
import { RELEASE_CANCELLED, settleOpenLockBeforeLeaving } from './speleoReleaseFailure';

/**
 * Set by {@link performQuit} for exactly one `tauri://close-requested` round
 * trip: the quit the user just confirmed in-app must not be re-guarded by
 * {@link guardQuit} when the window close comes back through the App-level
 * close listener. Without it, "Discard & Quit" would re-open the very dialog it
 * came from - an unbreakable loop, since the guard reads live store state that
 * a discard deliberately leaves dirty.
 *
 * Stored as an EXPIRY rather than a bare boolean, because "consumed exactly
 * once" only holds while the round trip actually completes. Every un-arming path
 * we have is on the emitting side (a rejected `close()`) or the receiving side
 * (a consume), so a request that is emitted successfully but whose event is never
 * DELIVERED leaves the flag armed forever - and the next genuine close consumes
 * it and skips `guardQuit` entirely, quitting silently over unpushed edits. That
 * delivery gap is narrow but real: the close listener's effect deps
 * (`[message, onConfirm]`) tear it down and re-register asynchronously, and the
 * listener's own early `return` when a close is already in flight consumes the
 * flag without acting on it. Nothing reachable was found that triggers it, so
 * the deadline is hardening, not a fix: it bounds the damage of any such gap to
 * the round trip it was armed for.
 */
let quitConfirmedUntil = 0;

/**
 * How long an armed confirmation stays valid. The round trip it covers is a
 * single IPC hop (request close -> Rust prevents it -> re-emit -> listener), so
 * this is orders of magnitude longer than it needs to be; it is sized to never
 * expire under a real quit, only to stop an UNDELIVERED one arming the next.
 */
const QUIT_CONFIRMATION_TTL_MS = 10_000;

/**
 * Read AND clear the one-shot confirmation. Consuming it (rather than peeking)
 * is what keeps it from going stale: if the close attempt is then abandoned for
 * any other reason, the next close request is guarded normally again.
 */
export function consumeQuitConfirmation(): boolean {
  const confirmed = Date.now() < quitConfirmedUntil;
  quitConfirmedUntil = 0;
  return confirmed;
}

/** Test-only reset so a leaked confirmation can't bleed across cases. */
export function resetQuitConfirmationForTests(): void {
  quitConfirmedUntil = 0;
}

/**
 * Ask the window to close. The Rust `on_window_event` handler PREVENTS the close
 * and re-emits `tauri://close-requested`, which the App-level `useBeforeUnload`
 * listener turns into the real destroy - so this is a request to run the exit
 * flow, never a close in itself. Requires the `core:window:allow-close`
 * capability; a rejection is logged (and runs `onRejected`) so a denied or
 * regressed capability is visible rather than failing silently.
 */
function requestWindowClose(onRejected?: () => void): void {
  getCurrentWindow()
    .close()
    .catch((err) => {
      onRejected?.();
      console.error('[quit] window.close() failed:', err);
    });
}

/**
 * Perform the actual quit: request a window close, pre-confirmed.
 *
 * The one-shot flag above tells the close listener this quit is already confirmed,
 * so it skips {@link guardQuit} instead of re-prompting. A rejected close un-arms
 * the flag, so the next close request is guarded again; `onRejected` lets the
 * caller drop whatever else it armed for this close.
 *
 * Call this only AFTER any exit confirmation - it does NOT prompt.
 */
export function performQuit(onRejected?: () => void): void {
  quitConfirmedUntil = Date.now() + QUIT_CONFIRMATION_TTL_MS;
  requestWindowClose(() => {
    quitConfirmedUntil = 0;
    onRejected?.();
  });
}

/**
 * Set by {@link requestUpdateRestart}: unlike {@link quitConfirmedUntil} it does
 * NOT pre-confirm the close (an update restart still has to run every exit
 * guard, same as a plain quit), so it is consumed only in `useBeforeUnload`'s
 * FINAL branch - after every guard has passed - where it swaps the window
 * destroy for `install_app_update_and_restart`.
 *
 * A plain boolean, NOT a deadline, and that is the whole point. It was a
 * 10-second TTL borrowed from {@link QUIT_CONFIRMATION_TTL_MS}, which is sized
 * for a single IPC round trip - but this flag spans HUMAN time: the guard it
 * runs opens the unsaved-changes dialog, and a user who reads it, saves their
 * work and then confirms takes far longer than ten seconds. Past the deadline
 * the restart silently degraded into an ordinary quit: the app closed, the
 * downloaded update was never applied, and nothing said so.
 *
 * What replaces the deadline is explicit ownership - every path that abandons
 * the attempt calls {@link abandonUpdateRestart}: the tier-1 native confirm
 * declined, either exit dialog cancelled, the install itself rejecting. A
 * guard that merely OPENS a dialog leaves this armed on purpose, because the
 * dialog's own action re-issues the close via `performQuit` and that re-issued
 * close is the same attempt continuing.
 */
let updateRestartArmed = false;

/**
 * Arm the update restart on BOTH sides, then request a window close through the
 * same entry point a plain quit uses (`quitApp`, not `performQuit`): an update
 * restart is not pre-confirmed, so it still runs the import/dive-plan prompt
 * and `guardQuit` exactly like every other exit.
 *
 * The backend arm is what makes the exit guards a real guarantee rather than a
 * convention: `install_app_update_and_restart` refuses unless it has been
 * armed, so a script in the webview cannot `invoke` its way past the dialogs by
 * calling the install command directly. It is armed FIRST and awaited, so a
 * refusal (nothing downloaded, a newer check superseded it) aborts the restart
 * before the window is asked to close.
 *
 * Rejects if the backend refuses; the caller surfaces that.
 */
export async function requestUpdateRestart(): Promise<void> {
  await invokeIpc('arm_app_update_restart', undefined, {
    purpose: 'arm the in-app update restart',
  });
  updateRestartArmed = true;
  // Awaited here rather than delegating to `quitApp`, which is fire-and-forget:
  // if the close is REFUSED (a revoked `core:window:allow-close` capability, an
  // IPC failure) the guard chain never runs, so nothing downstream will ever
  // consume the arm we just took - and the next ordinary close the user makes
  // would find it still set and install an update they never asked to install.
  // Both halves are released here, and the rejection is re-thrown so the caller
  // can say the restart did not start.
  try {
    await getCurrentWindow().close();
  } catch (err) {
    abandonUpdateRestart();
    console.error('[quit] window.close() failed:', err);
    throw err;
  }
}

/**
 * Read AND clear the frontend half of the arm. Consumed only once every exit
 * guard has passed - see {@link updateRestartArmed}.
 *
 * The backend half is spent by the install command itself, so a consumed arm
 * needs no matching unarm call.
 */
export function consumeUpdateRestartArmed(): boolean {
  const armed = updateRestartArmed;
  updateRestartArmed = false;
  return armed;
}

/**
 * Give up on a restart attempt, on both sides.
 *
 * Safe (and free) to call on any exit that was never a restart - it returns
 * immediately when nothing is armed, so the two exit dialogs can call it from
 * their ordinary Cancel handlers without knowing why they were opened.
 */
export function abandonUpdateRestart(): void {
  if (!updateRestartArmed) return;
  updateRestartArmed = false;
  void invokeIpc('unarm_app_update_restart', undefined, {
    purpose: 'abandon the in-app update restart',
  }).catch((err) => {
    // The backend arm is one-shot and only reachable through this same flow, so
    // a failed unarm is not dangerous - but it must not be silent.
    console.error('[quit] unarm_app_update_restart failed:', err);
  });
}

/** Test-only reset so a leaked arm can't bleed across cases. */
export function resetUpdateRestartForTests(): void {
  updateRestartArmed = false;
}

/** Saved local work that has not reached SpeleoDB. This does not require an exit prompt. */
export function hasUnpushedSpeleoWork(link: SpeleoLinkState | null): boolean {
  return link?.hasUnpushedEdits === true;
}

export function holdsWriteLock(link: SpeleoLinkState | null): boolean {
  return link?.editState === 'editing';
}


/**
 * What the user asked for, in a shape a DIALOG can finish on its own.
 *
 * [#303] Exists because an exit prompt is not always the last dialog in the chain.
 * Both prompts hand off to `useSpeleoActions.done`, which SHORT-CIRCUITS on a copy that
 * is dirty only because a chain step walked it back: it raises its own confirmation and
 * answers `null`, so the `if (resolved?.kind === 'released')` continuation - the quit, or
 * the close plus whatever that close was standing in for - never runs. Pressing "Upload
 * this older version" from there published the revert and then left the user sitting in
 * the app they had asked to leave. Carrying the intent through the confirmation is what
 * lets it finish the job instead of dropping it.
 */
export type PendingExit =
  | { kind: 'quit' }
  | {
      kind: 'close';
      /** The `.stalx` this close was standing in for (Ctrl+O / drag-drop) (#270). */
      pendingOpenPath?: string | null;
      /** A New Project request the close prompt displaced (#270). */
      pendingNewProject?: boolean;
      accessConfirmed?: boolean;
      request?: number;
      sessionGeneration?: number;
    };

/**
 * Read a {@link PendingExit} back out of `dialogData`, which is an untyped
 * `Record<string, unknown>` - so the confirmation that carries one VALIDATES rather than
 * trusts, exactly as the `cancelReturnsTo` readers do. `null` means "nobody is waiting on
 * this", which is the case for every non-exit route into the same confirmation (the
 * status button, the lock menu).
 */
export function readPendingExit(data: Record<string, unknown>): PendingExit | null {
  const raw = data.pendingExit;
  if (typeof raw !== 'object' || raw === null) return null;
  const { kind, pendingOpenPath, pendingNewProject, accessConfirmed, request, sessionGeneration } = raw as Record<string, unknown>;
  if (kind === 'quit') return { kind: 'quit' };
  if (kind !== 'close') return null;
  return {
    kind: 'close',
    pendingOpenPath: typeof pendingOpenPath === 'string' ? pendingOpenPath : null,
    pendingNewProject: pendingNewProject === true,
    ...(typeof request === 'number' ? { request } : {}),
    ...(typeof sessionGeneration === 'number' ? { sessionGeneration } : {}),
    ...(accessConfirmed === true ? { accessConfirmed: true } : {}),
  };
}

export function closeExitStillCurrent(exit: { request?: number; sessionGeneration?: number }, dispatched = false): boolean {
  return (exit.request === undefined || isProjectRequestCurrent(exit.request) || (dispatched && isProjectMutationCurrent(exit.request))) &&
    (exit.sessionGeneration === undefined || useSpeleoStore.getState().sessionGeneration === exit.sessionGeneration);
}

/**
 * Hand the open project back, then replay whatever the user actually asked for.
 *
 * The destination access warning runs before the close. The close releases
 * the server lock and clears the SpeleoDB link, so replaying before it would leave the
 * shell linked to a project that is on its way out. A FAILED close replays nothing - the
 * project is still open, so the exit did not happen.
 *
 * Lifted out of `SpeleoCloseDialog` (where it was `closeThen`) so the unwound-push
 * confirmation completes a displaced close EXACTLY as the close prompt would, rather than
 * growing a second copy of the ordering argument above.
 */
export async function handBackProject(
  exit: { pendingOpenPath?: string | null; pendingNewProject?: boolean; accessConfirmed?: boolean; request?: number; sessionGeneration?: number },
  opts?: { releaseLock?: boolean; isCurrent?: () => boolean; onClosed?: () => void; onFailure?: (message: string) => void },
): Promise<void> {
  const request = exit.request ?? reserveProjectRequest();
  let dispatched = false;
  const current = () => (isProjectRequestCurrent(request) || (dispatched && isProjectMutationCurrent(request))) && closeExitStillCurrent(exit, dispatched);
  if (!current()) return;
  const { closeProject, loadProject } = useProjectStore.getState();
  if (exit.pendingOpenPath && !exit.accessConfirmed) {
    try {
      if (!await confirmSpeleoFileOpen(exit.pendingOpenPath, current)) return;
    } catch (error) {
      if (!current()) return;
      // The access check rejects with a structured SpeleoErrorDto, which String() renders
      // as '[object Object]'; read its message the way the plain open path does.
      useUIStore.getState().openDialog('error', { title: 'Open Failed', message: getErrorMessage(error) });
      return;
    }
  }
  const canClose = () => current() && (opts?.isCurrent?.() ?? true);
  if (!canClose()) return;
  dispatched = true;
  const result = await closeProject({ ...(opts?.releaseLock === undefined ? {} : { releaseLock: opts.releaseLock }), request, isCurrent: canClose });
  if (!current()) return;
  if (!result.success) {
    // The user cancelled "Couldn't hand back the lock": they stay, nothing to report.
    if (result.error === RELEASE_CANCELLED) return;
    const message = result.error || i18n.t('projectClose.failedMessage');
    if (opts?.onFailure) opts.onFailure(message);
    else useUIStore.getState().openDialog('error', { title: i18n.t('projectClose.failedTitle'), message });
    return;
  }
  opts?.onClosed?.();
  // loadProject switches the view and surfaces its own failures.
  if (exit.pendingOpenPath) await loadProject(exit.pendingOpenPath, { accessConfirmed: true, request });
  // `createProject` seeds the shell from here; the close above already left us on the
  // global view (App falls back to it whenever no project is open).
  else if (exit.pendingNewProject) useUIStore.getState().openDialog('new-project');
}

/**
 * Raise one of the exit prompts, remembering the dialog it displaces so its own
 * Cancel can put that dialog back.
 *
 * `uiStore` keeps a SINGLE `activeDialog` slot and `openDialog` replaces it
 * unconditionally, so an exit prompt evicts whatever was on screen. That is fine for
 * the exit itself - the app is going away - but not for CANCEL, which leaves the user
 * in the app with the evicted dialog gone. The sharp case is a multi-step import
 * wizard: press X mid-placement and tier 1 asks "Import is in progress. Discard all
 * import data and close?"; confirming that swaps the wizard out for this prompt, and
 * cancelling here drops the user back into the app with the wizard - and the sections
 * they had placed - discarded. They consented to that discard only as PART of
 * quitting. Both new prompts on this branch solved the same eviction with a return
 * hop (`SpeleoUnpushedLocalDialog`'s `cancelReturnsTo`, `SpeleoRevertDialog`'s
 * `returnTo`); this is that hop for the quit guards.
 *
 * The wizard's own state lives in `dialogData`, so carrying the pair back restores
 * it. (`openDialog` also resets `dmpImportWizardPhase`, which drives onboarding
 * hints only - the wizard itself does not read it.)
 *
 * [#303] Exported because the chain does not stop at the exit prompt: the unwound-push
 * confirmation is raised OUT of one and evicts it from the same single slot, so its
 * Cancel needs the same return hop or the exit request evaporates with nothing on screen
 * to say so.
 */
export function openExitPrompt(dialog: string, data?: Record<string, unknown>): void {
  const { activeDialog, dialogData, openDialog } = useUIStore.getState();
  // Re-opening ourselves is not a return hop, it is a loop - and there is nothing to
  // restore when nothing was displaced.
  const displaced =
    activeDialog && activeDialog !== dialog
      ? { cancelReturnsTo: activeDialog, cancelReturnsData: dialogData }
      : null;
  if (!displaced && !data) openDialog(dialog);
  else openDialog(dialog, { ...data, ...displaced });
}

/**
 * The in-app quit guard (menu Quit, Ctrl+Q, the window X - all through the
 * `useBeforeUnload` close listener). Returns true when it took the close over: it
 * re-issues the quit itself (`performQuit`) once its question is answered.
 *
 * - We hold the SpeleoDB write lock over work SpeleoDB has not seen: the exit
 *   prompt (upload / keep the lock / discard). It closes the project per the
 *   answer; the quit then continues.
 * - The same state on an UPDATE RESTART asks nothing: it keeps the lock and the
 *   local changes (saving unsaved edits first) and restarts, so the user resumes
 *   exactly where they were - the "Keep the lock" end state without the question.
 * - Otherwise unsaved changes get the ordinary unsaved-changes dialog.
 * - A clean SpeleoDB project hands its lock back before the quit goes on
 *   ({@link quitAfterHandingBackLock}): a release that fails or is uncertain asks
 *   "Couldn't hand back the lock", and Cancel there abandons the quit. The bounded
 *   silent release at process exit stays only for the exits the app cannot
 *   intercept.
 */
export function guardQuit(): boolean {
  const { project, isDirty } = useProjectStore.getState();
  if (!project) return false;

  if (openProjectNeedsSpeleoExitPrompt()) {
    if (updateRestartArmed) {
      void keepLockThenRestart();
      return true;
    }
    void askSpeleoExit('quit').then((closed) => {
      if (closed) performQuit();
      else abandonUpdateRestart();
    });
    return true;
  }

  if (isDirty) {
    openExitPrompt('unsaved-changes', { pendingQuit: true });
    return true;
  }

  if (useSpeleoStore.getState().link !== null) {
    void quitAfterHandingBackLock();
    return true;
  }

  return false;
}

/**
 * Settle the open project's SpeleoDB lock, then quit (pre-confirmed): the
 * holder-checked release, or - when it fails or is uncertain - the user's answer
 * to "Couldn't hand back the lock" (Retry until it settles, or Close and keep the
 * lock). Cancel abandons the quit, and any update restart riding on it. A local
 * project, or a SpeleoDB project holding no lock, quits straight away.
 */
export async function quitAfterHandingBackLock(): Promise<void> {
  const projectOpen = useProjectStore.getState().project !== null;
  if (await settleOpenLockBeforeLeaving('quit', projectOpen)) performQuit();
  else abandonUpdateRestart();
}

/** The update restart's answer for un-uploaded work under our lock (see `guardQuit`). */
async function keepLockThenRestart(): Promise<void> {
  const kept = await closeKeepingLock();
  if (kept.ok) {
    performQuit();
    return;
  }
  abandonUpdateRestart();
  useUIStore.getState().openDialog('error', {
    title: i18n.t('speleoExitPrompt.restart.failedTitle'),
    message: kept.error,
  });
}

/**
 * Quit the application.
 *
 * Deliberately runs NO guard here: it asks for a close, un-confirmed, and the
 * App-level `useBeforeUnload` listener runs the whole exit flow - the native
 * import / dive-plan confirm first, then {@link guardQuit}. That listener is the
 * only place both tiers exist, and the ONLY ordering in which every cancellable
 * question is asked before anything irreversible happens.
 *
 * It used to call `guardQuit()` directly and then `performQuit()`, which put the
 * tiers in the opposite order for menu Quit / Ctrl+Q: `SpeleoQuitDialog` ran and
 * its "Push, release & quit" pushed AND RELEASED the server lock, and only then
 * did the re-issued close reach the native "You have unsaved changes to this dive
 * plan. Discard and close?" prompt. Cancelling there left the user in the app with
 * the lock irreversibly surrendered and the project silently read-only, with
 * nothing saying why. The window X never had the bug - it always arrived at the
 * listener first - so the two paths now share that one order.
 */
export function quitApp(): void {
  requestWindowClose();
}
