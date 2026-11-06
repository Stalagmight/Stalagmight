/**
 * Shared, React-free project actions (New / Save As) invoked from BOTH the
 * hamburger menu (the `useProject` hook handlers) and the keyboard dispatchers
 * (`fileShortcutActions` / `useKeyboardShortcuts`).
 *
 * Centralizing them keeps the New-project unsaved-changes guard and the Save-As
 * + overwrite-confirm flow in exactly one place so the menu and keyboard paths
 * can't drift - they previously each had their own copy (the New-project guard
 * existed only on the menu path, and Save-As's `closeAfterSave` handling was
 * added to the menu copy but not the keyboard one).
 *
 * Stores are read via `getState()` so these are callable from raw keydown
 * handlers (outside React). Callers tag the telemetry entry-point source before
 * calling.
 */
import { useProjectStore } from '../store/projectStore';
import { useUIStore } from '../store/uiStore';
import { useSpeleoStore } from '../store/speleoStore';
import { needsSpeleoHandBackPrompt } from './quit';
import { saveFailureOf, surfaceSaveFailure } from './saveFeedback';
import {
  ensureExtension,
  projectSaveFilters,
  saveDialog,
  PROJECT_EXTENSION,
} from './fileDialogs';
import { extractFilename } from './path';
import { saveTargetPresence } from './overwriteGuard';
import { chooseUnlinkDestination, unlinkOpenProjectTo } from './speleoUnlink';
import type { CommandOutcome } from '../store/commandResult';
import type { SaveOutcome } from '../types/ipc/SaveOutcome';

/**
 * Open the New Project dialog, guarding a dirty open project: defer to the
 * unsaved-changes dialog (which opens new-project on discard) instead of letting
 * `createProject` silently clobber unsaved edits. Mirrors the `.stalx`-open guard
 * in `routeOpenedFile`. In GlobalView no project is open, so this falls straight
 * through to the new-project dialog.
 *
 * #270: it also mirrors that guard's SpeleoDB half, which it was missing. Starting
 * a new project is one of the four ways to leave an open one, and gating on the
 * DISK-dirty bit alone let a remote project that was saved but still held a lock -
 * or still had edits SpeleoDB has never seen - be swapped out with no notice, while
 * closing the same project prompted. SpeleoDB first for the same reason `guardQuit`
 * orders it that way: that prompt subsumes the disk one (it has its own "save
 * changes locally" checkbox), and the reverse order would let "Discard Changes"
 * walk straight past the lock. The prompt reopens the new-project dialog itself
 * once resolved, so the user's New Project is not swallowed by the prompt.
 */
export function newProjectGuarded(): void {
  const { openDialog } = useUIStore.getState();
  const ps = useProjectStore.getState();
  if (!ps.project) {
    openDialog('new-project');
  } else if (needsSpeleoHandBackPrompt(useSpeleoStore.getState().link)) {
    openDialog('speleodb-close', { pendingNewProject: true });
  } else if (ps.isDirty) {
    openDialog('unsaved-changes', { pendingNewProject: true });
  } else {
    openDialog('new-project');
  }
}

/**
 * Save As: pick a path, then either defer to the overwrite-confirm dialog (the
 * file already exists, or whether it does could not be checked) or save directly.
 * Carries the caller's "close after a successful save" intent (Save and Close)
 * through the deferred overwrite flow.
 *
 * A failed save is routed through `surfaceSaveFailure` (integrity blocks → Review
 * & Repair; permission denials already surfaced their own guidance dialog). The
 * guard is `!success`, NOT "has an error string": a refused save is routed on its
 * code, and an integrity block is signaled via `integritySaveBlocked`, so it must
 * still surface rather than be mistaken for a clean save.
 */
export async function saveProjectAs(opts?: {
  closeAfterSave?: boolean;
}): Promise<CommandOutcome<SaveOutcome> | null> {
  const { openDialog } = useUIStore.getState();
  try {
    const selected = await saveDialog({ filters: projectSaveFilters() });
    if (!selected) return null;

    const finalPath = ensureExtension(selected, PROJECT_EXTENSION);
    // [#576 E10-D1] A probe that FAILS (permissions, an unreachable folder, a
    // network drive) says nothing about whether the file is there, so it asks
    // exactly like a probe that found it. It used to fall through to the save,
    // replacing an existing survey with no prompt at all. [#576 HFI-2] The
    // backend answers: the webview's fs plugin threw for every user path.
    // [#576 HFI-2-D2] A refused path (an app or system folder) goes straight to
    // the save, which refuses it with the reason: one dialog, not two.
    const { present, existenceUnknown } = await saveTargetPresence(finalPath);
    if (present) {
      // The overwrite-confirm dialog performs the actual save. Carry the
      // close-after-save intent so the deferred flow still closes the project.
      openDialog('confirm-overwrite', {
        path: finalPath,
        fileName: extractFilename(finalPath),
        closeAfterSave: opts?.closeAfterSave ?? false,
        ...(existenceUnknown ? { existenceUnknown: true } : {}),
      });
      return null;
    }

    const result = await useProjectStore.getState().saveProject(finalPath);
    if (result && !result.success) {
      surfaceSaveFailure(result, openDialog, useProjectStore.getState());
    }
    return result;
  } catch (e) {
    // store.saveProject rejects only on an IPC-transport failure (a refused save
    // resolves as the refused outcome, handled above). Surface
    // it here rather than letting it escape as an unhandled rejection - the
    // keyboard (`void saveProjectAs()`) and menu (`void handleSaveProjectAs()`)
    // call sites don't catch. Return null (not a failure result) so the
    // delegating handleSaveProject doesn't surface a second dialog and
    // saveAndClose treats it as "not saved" and keeps the project open.
    surfaceSaveFailure(saveFailureOf(e, 'Failed to save project'), openDialog, useProjectStore.getState(), {
      skipIntegrityRouting: true,
    });
    return null;
  }
}

/**
 * Detach the open SpeleoDB project from SpeleoDB, keeping the user's work as a
 * plain unmanaged `.stalx` at a path they pick (#204 - the shared action behind
 * the no-permission prompt (10) and the divergence "keep my copy" path (11)).
 *
 * [#576 FZ-SDB-5-D1/D2] The copy MOVES out of the SpeleoDB cache:
 *
 * 1. Ask where, with a Save As that starts in the default projects folder with the
 *    project name filled in, then the overwrite ask for a rewritten name (D35).
 *    Cancelling either changes NOTHING and calls nothing.
 * 2. `speleodb_unlink` writes the unlinked copy there, closes the backend session
 *    on the cache file, deletes the cache file and releases a held lock
 *    best-effort. On any error nothing has changed.
 * 3. Re-seed the link, tell what the copy went without, and load the copy: the
 *    backend session is closed, so the session lands on it only through this load
 *    (`unlinkOpenProjectTo`).
 *
 * Returns true once converted; the caller keys only on the boolean.
 */
export async function convertToUnmanagedStalx(): Promise<boolean> {
  const { openDialog } = useUIStore.getState();
  let destination: string | null;
  try {
    destination = await chooseUnlinkDestination(useProjectStore.getState().project?.name ?? '');
  } catch (e) {
    surfaceSaveFailure(saveFailureOf(e, 'Failed to save a local copy'), openDialog, useProjectStore.getState(), {
      skipIntegrityRouting: true,
    });
    return false;
  }
  if (!destination) return false;
  return unlinkOpenProjectTo(destination);
}
