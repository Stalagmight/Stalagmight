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
import { exists } from '@tauri-apps/plugin-fs';
import { useProjectStore } from '../store/projectStore';
import { useUIStore } from '../store/uiStore';
import { useSpeleoStore } from '../store/speleoStore';
import { askSpeleoExit, openProjectNeedsSpeleoExitPrompt } from './speleoExit';
import { confirmWorkspaceDrafts, hasWorkspaceDrafts } from './workspaceDrafts';
import { surfaceSaveFailure } from './saveFeedback';
import i18n from '../i18n';
import { getErrorMessage, invokeIpc } from './ipc';
import {
  ensureExtension,
  projectSaveFilters,
  saveDialog,
  PROJECT_EXTENSION,
} from './fileDialogs';
import { extractFilename } from './path';
import type { CommandResult } from '../types/commands';

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
  if (ps.project && hasWorkspaceDrafts()) {
    // Unsent SpeleoDB input is asked about first; then the guards below run as usual.
    void confirmWorkspaceDrafts().then((discarded) => { if (discarded) newProjectGuarded(); });
    return;
  }
  if (!ps.project) {
    openDialog('new-project');
  } else if (openProjectNeedsSpeleoExitPrompt()) {
    // The exit prompt closes the project per the user's answer; only then does
    // the New Project they asked for open.
    void askSpeleoExit('switch').then((closed) => {
      if (closed) useUIStore.getState().openDialog('new-project');
    });
  } else if (ps.isDirty) {
    openDialog('unsaved-changes', { pendingNewProject: true });
  } else {
    openDialog('new-project');
  }
}

/**
 * Save As: pick a path, then either defer to the overwrite-confirm dialog (the
 * file already exists) or save directly. Carries the caller's "close after a
 * successful save" intent (Save and Close) through the deferred overwrite flow.
 *
 * A failed save is routed through `surfaceSaveFailure` (integrity blocks → Review
 * & Repair; permission denials already surfaced their own guidance dialog). The
 * guard is `!success`, NOT "has an error string": a blocked save can legitimately
 * carry a null error (the block is signaled via `integritySaveBlocked`), and must
 * still surface rather than be mistaken for a clean save.
 */
export async function saveProjectAs(opts?: {
  closeAfterSave?: boolean;
}): Promise<CommandResult | null> {
  const { openDialog } = useUIStore.getState();
  try {
    const selected = await saveDialog({ filters: projectSaveFilters() });
    if (!selected) return null;

    const finalPath = ensureExtension(selected, PROJECT_EXTENSION);
    try {
      if (await exists(finalPath)) {
        // The overwrite-confirm dialog performs the actual save. Carry the
        // close-after-save intent so the deferred flow still closes the project.
        openDialog('confirm-overwrite', {
          path: finalPath,
          fileName: extractFilename(finalPath),
          closeAfterSave: opts?.closeAfterSave ?? false,
        });
        return null;
      }
    } catch {
      // exists() failed - proceed with the save (the backend reports real errors).
    }

    const result = await useProjectStore.getState().saveProject(finalPath);
    if (result && !result.success) {
      surfaceSaveFailure(result.error, openDialog);
    }
    return result;
  } catch (e) {
    // store.saveProject rejects only on an IPC-transport failure (real save
    // failures resolve as CommandResult{success:false}, handled above). Surface
    // it here rather than letting it escape as an unhandled rejection - the
    // keyboard (`void saveProjectAs()`) and menu (`void handleSaveProjectAs()`)
    // call sites don't catch. Return null (not a failure result) so the
    // delegating handleSaveProject doesn't surface a second dialog and
    // saveAndClose treats it as "not saved" and keeps the project open.
    surfaceSaveFailure(getErrorMessage(e, 'Failed to save project'), openDialog, {
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
 * Order is the data-safety argument:
 *
 * 1. Save first. On a linked project an explicit destination writes a detached
 *    UNLINKED copy (`unlinked_copy_path`; the session stays on the managed
 *    file). The copy carries the survey data, settings and undo history - the
 *    user's work is durable at `finalPath` before anything is given up. If the
 *    dialog is cancelled or the save fails, NOTHING has changed.
 * 2. Only then `speleodb_unlink`: the ABANDONING lock release + gate reset for
 *    the open (managed) project. Its link-row delete lands on the cache copy,
 *    which is the right end state - an abandoned cache file is plain debris the
 *    next pull replaces, instead of a linked-and-dirty leftover that nags the
 *    next Open-from-SpeleoDB.
 * 3. Load the copy, so the session actually lands on the file the user chose
 *    (the save no longer relocates it there).
 *
 * Returns true once converted; the caller keys only on the boolean.
 */
export async function convertToUnmanagedStalx(): Promise<boolean> {
  const { openDialog } = useUIStore.getState();
  let finalPath: string;
  let wroteDetachedCopy: boolean;
  try {
    const selected = await saveDialog({ filters: projectSaveFilters() });
    if (!selected) return false;
    finalPath = ensureExtension(selected, PROJECT_EXTENSION);
    const result = await useProjectStore.getState().saveProject(finalPath);
    if (!result || !result.success) {
      surfaceSaveFailure(result?.error ?? null, openDialog);
      return false;
    }
    wroteDetachedCopy = Boolean(result.unlinked_copy_path);
  } catch (e) {
    surfaceSaveFailure(getErrorMessage(e, 'Failed to save a local copy'), openDialog, {
      skipIntegrityRouting: true,
    });
    return false;
  }

  // The project turned out not to be linked: the save was an ordinary Save As
  // that already relocated the session to finalPath, and there is nothing to
  // detach. Degenerate, but keep the old contract for it.
  if (!wroteDetachedCopy) {
    await useSpeleoStore.getState().refreshLinkStatus();
    return true;
  }

  // The copy is safe on disk - now drop the SpeleoDB link and re-seed the shell as
  // a plain local project (link_status returns is_remote:false → link cleared). The
  // unlink can still reject (e.g. a poisoned database lock); guard it so it can't
  // escape as an unhandled rejection that leaves the dialog open while the project is
  // silently still SpeleoDB-linked though the user was told it converted (fix 6).
  try {
    await invokeIpc('speleodb_unlink');
  } catch (e) {
    surfaceSaveFailure(
      getErrorMessage(e, 'The local copy was saved, but detaching from SpeleoDB failed'),
      openDialog,
      { skipIntegrityRouting: true, title: i18n.t('dialogs.saveFailure.detachTitle') },
    );
    return false;
  }
  await useSpeleoStore.getState().refreshLinkStatus();
  // Land the session on the file the user chose. A failed load surfaces its own
  // error through loadProject; the conversion itself already happened, so still
  // report success (the copy exists and the project is detached).
  await useProjectStore.getState().loadProject(finalPath);
  return true;
}
