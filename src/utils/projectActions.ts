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
import { editsSettledForHandBack, needsSpeleoHandBackPrompt } from './quit';
import { saveFailureOf, surfaceSaveFailure } from './saveFeedback';
import i18n from '../i18n';
import { invokeIpc } from './ipc';
import {
  ensureExtension,
  projectSaveFilters,
  saveDialog,
  PROJECT_EXTENSION,
} from './fileDialogs';
import { extractFilename } from './path';
import { resolveSavePath, saveTargetPresence } from './overwriteGuard';
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
 *
 * [#576 W84] With a project open, the guards run once the edits queued so far
 * have settled, as at quit (`editsSettledForExit`): an edit still in flight (a
 * grid cell committed on blur on the way to New Project) reads as a clean
 * project, and the switch to the new one waits for it to land on this project
 * and then replaces it, unsaved. One that has not settled in time counts as
 * unsaved work. With no project open there is nothing to wait for, and the
 * dialog opens synchronously as before. [#576 W85] A New Project asked for while
 * another hand-back exit waits (Ctrl+N twice) is dropped: that exit owns the answer.
 */
export async function newProjectGuarded(): Promise<void> {
  const { openDialog } = useUIStore.getState();
  const editsSettled = useProjectStore.getState().project ? await editsSettledForHandBack() : true;
  if (editsSettled === null) return;
  const ps = useProjectStore.getState();
  if (!ps.project) {
    openDialog('new-project');
  } else if (needsSpeleoHandBackPrompt(useSpeleoStore.getState().link)) {
    openDialog('speleodb-close', { pendingNewProject: true });
  } else if (ps.isDirty || !editsSettled) {
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
    // [#576 D35] The copy lands on the NORMALISED name, which the OS overwrite
    // prompt never saw when it differs from the answer - and the backend's
    // copy-only branch renames over whatever is there. Ask first; declining
    // changes nothing, like a cancelled picker.
    const resolved = await resolveSavePath(selected, PROJECT_EXTENSION);
    if (!resolved) return false;
    finalPath = resolved;
    const result = await useProjectStore.getState().saveProject(finalPath);
    if (!result || !result.success) {
      surfaceSaveFailure(result, openDialog, useProjectStore.getState());
      return false;
    }
    wroteDetachedCopy = Boolean(result.unlinked_copy_path);
  } catch (e) {
    surfaceSaveFailure(saveFailureOf(e, 'Failed to save a local copy'), openDialog, useProjectStore.getState(), {
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
      saveFailureOf(e, 'The local copy was saved, but detaching from SpeleoDB failed'),
      openDialog,
      useProjectStore.getState(),
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
