/**
 * Quit entry point, shared by every affordance (GlobalView menu, in-project
 * menu, the Ctrl+Q shortcut in both shortcut hooks, AND the OS window close
 * button) so the exit guards + close contract live in exactly one place.
 */
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invokeIpc } from './ipc';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
import { holdAutosave, withAutosaveHeld } from './autosaveHold';
import { openPendingImport, readPendingImport, type PendingImport } from './pendingImport';

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
 * the flag, so the next close request is guarded again.
 *
 * Call this only AFTER any exit confirmation - it does NOT prompt.
 */
export function performQuit(): void {
  quitConfirmedUntil = Date.now() + QUIT_CONFIRMATION_TTL_MS;
  // [#576 D34] The quit is confirmed - which may mean "discard" - so autosave must
  // not write the project while the window goes away. Held for the same window the
  // confirmation is valid for, and dropped at once if the close is refused, so a
  // quit that does not happen leaves autosave running.
  const release = holdAutosave();
  const expiry = setTimeout(release, QUIT_CONFIRMATION_TTL_MS);
  requestWindowClose(() => {
    quitConfirmedUntil = 0;
    clearTimeout(expiry);
    release();
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

/**
 * Work that exists ONLY on this computer: edits SpeleoDB has never seen.
 *
 * Shared by EVERY exit ({@link guardQuit} plus the three hand-back exits behind
 * {@link needsSpeleoHandBackPrompt}) so they cannot drift again. The predicate
 * keys on the WORK AT RISK, not on the lock, because the states where the work is
 * most stranded are precisely the ones that drop the lock while keeping the
 * edits: `notifyReverifyDivergence()` sets read-only, and any `ReLogin` error
 * runs `clearSession()`, which does too.
 *
 * Quit needs only this - and deliberately so: holding the lock with nothing
 * unpushed is safe to walk away from, because the backend's exit-time release
 * vetoes itself when there ARE unpushed edits (aab5121d). The hand-back exits
 * additionally have to resolve a held lock, since they hand the project back
 * while the app keeps running.
 */
export function hasUnpushedSpeleoWork(link: SpeleoLinkState | null): boolean {
  return link?.hasUnpushedEdits === true;
}

/**
 * Do we hold the SpeleoDB write lock?
 *
 * `editState === 'editing'` is the app's hold-lock signal (the backend enforces
 * hold-lock ⟹ edit-enabled), EXCEPT while force-editing (#213), which is editing
 * with no lock at all - so a force-editor must count as NOT holding it.
 *
 * Shared because that second half is the easy half to forget, and forgetting it
 * is not cosmetic: it offers a force-editor lock-owner actions (release, restore,
 * push-and-release) that the backend can only refuse. It lived spelled out at six
 * call sites and one had already drifted - `SpeleoProjectAdmin`'s Lock section
 * told a force-editor "held by you" and offered them Release.
 */
export function holdsWriteLock(link: SpeleoLinkState | null): boolean {
  return link?.editState === 'editing' && !link.forceEditing;
}

/**
 * The SpeleoDB guard for every exit that HANDS THE OPEN PROJECT BACK while the app
 * keeps running. There are three, and they are reached from the same in-project
 * hamburger menu, so a user has no reason to expect them to differ:
 *
 * - Close                (`useProject.handleCloseProject`)
 * - Open another `.stalx` (`routeOpenedFile`, also Ctrl+O and drag-drop)
 * - New Project          (`newProjectGuarded`, also Ctrl+N)
 *
 * Both terms are needed and neither subsumes the other. The held lock must be
 * resolved because the app keeps running, so nothing else will ever release it -
 * this is what {@link guardQuit} does NOT need, since the backend releases on
 * process exit. The unpushed work must be surfaced because nothing else surfaces
 * it, and it survives in the cache `.stalx` long after the project is swapped out.
 *
 * Only the close exit used to have EITHER: opening another project or starting a
 * new one gated on disk-dirty alone, so a remote project saved to disk with
 * unpushed edits was swapped out with no notice at all - the same defect, on the
 * two exits nobody had looked at.
 */
export function needsSpeleoHandBackPrompt(link: SpeleoLinkState | null): boolean {
  return hasUnpushedSpeleoWork(link) || holdsWriteLock(link);
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
      /** A parsed "Import as new project" file the close prompt displaced (#576 HFI-4-D2). */
      pendingImport?: PendingImport | null;
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
  const { kind, pendingOpenPath, pendingNewProject } = raw as Record<string, unknown>;
  if (kind === 'quit') return { kind: 'quit' };
  if (kind !== 'close') return null;
  const pendingImport = readPendingImport((raw as Record<string, unknown>).pendingImport);
  return {
    kind: 'close',
    pendingOpenPath: typeof pendingOpenPath === 'string' ? pendingOpenPath : null,
    pendingNewProject: pendingNewProject === true,
    ...(pendingImport ? { pendingImport } : {}),
  };
}

/**
 * Hand the open project back, then replay whatever the user actually asked for.
 *
 * The close has to land first in every case: it is what releases (or deliberately keeps)
 * the server lock and clears the SpeleoDB link, so replaying before it would leave the
 * shell linked to a project that is on its way out. A FAILED close replays nothing - the
 * project is still open, so the exit did not happen.
 *
 * Lifted out of `SpeleoCloseDialog` (where it was `closeThen`) so the unwound-push
 * confirmation completes a displaced close EXACTLY as the close prompt would, rather than
 * growing a second copy of the ordering argument above.
 */
export async function handBackProject(
  exit: { pendingOpenPath?: string | null; pendingNewProject?: boolean; pendingImport?: PendingImport | null },
  opts?: { releaseLock?: boolean },
): Promise<void> {
  const { closeProject, loadProject } = useProjectStore.getState();
  // [#576 D34] The project is being handed back, possibly with local edits the user
  // chose not to save: autosave must not write them while the close runs. A close
  // that fails leaves the project open, and the release re-arms autosave.
  const result = await withAutosaveHeld(() => closeProject(opts));
  if (!result.success) return;
  // loadProject switches the view and surfaces its own failures.
  if (exit.pendingOpenPath) await loadProject(exit.pendingOpenPath);
  // `createProject` seeds the shell from here; the close above already left us on the
  // global view (App falls back to it whenever no project is open).
  else if (exit.pendingNewProject) useUIStore.getState().openDialog('new-project');
  // [#576 HFI-4-D2] The parsed file of an "Import as new project" this close stood in for.
  else if (exit.pendingImport) openPendingImport(exit.pendingImport);
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
 * The exit guards, in the order the close flow uses them (see
 * `useProject.handleCloseProject`). Returns `true` when a dialog was opened and
 * the caller must NOT quit; `false` when there is nothing to protect.
 *
 * Ordering - SpeleoDB first, unsaved-to-disk second - is the same as the
 * close-project flow's, and matters because the two conditions overlap: a project
 * can be both unsaved to disk AND unpushed to the server. The SpeleoDB prompt is
 * the one that subsumes the other (it carries its own "Save changes locally"
 * checkbox, defaulted to on, exactly like `SpeleoCloseDialog`), so running it
 * first composes the two into ONE decision. The reverse order could not: the
 * unsaved-changes dialog knows nothing about the lock, so its "Discard & Quit"
 * would quit straight past the unpushed edits.
 *
 * Both branches terminate: each dialog quits via {@link performQuit}, which
 * arms the one-shot confirmation so the resulting close request skips this
 * guard entirely.
 */
export function guardQuit(): boolean {
  const { project, isDirty } = useProjectStore.getState();
  if (!project) return false;

  const link = useSpeleoStore.getState().link;
  // Keyed on the WORK AT RISK only - see {@link hasUnpushedSpeleoWork}. Whether we
  // still hold the write lock changes which choices the dialog can offer (it
  // adapts), not whether the user should be told. Holding the lock with nothing
  // unpushed needs no prompt at QUIT time specifically: the backend's exit-time
  // release is safe then, and vetoes itself otherwise (aab5121d). The close flow
  // does not get that for free, so it guards on the lock as well.
  if (hasUnpushedSpeleoWork(link)) {
    openExitPrompt('speleodb-quit');
    return true;
  }

  if (isDirty) {
    openExitPrompt('unsaved-changes', { pendingQuit: true });
    return true;
  }

  return false;
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
