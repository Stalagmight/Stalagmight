/**
 * [#576 FZ-SDB-5] Unlink MOVES the open project's copy out of the SpeleoDB cache.
 *
 * Two entry points share it: `convertToUnmanagedStalx` (the no-permission and
 * divergence prompts, and the Admin tab after Delete project on SpeleoDB,
 * #576 SDB-DEL-D1) and the refused-open prompt's keep-local branch for a
 * link-less copy an Unlink made before this change left in the cache
 * (`UnpushedLocalEdits.unlinked`, FZ-SDB-5-D3). Both ask the same Save As and run
 * the same tail, so the two can never drift apart.
 */
import { useProjectStore } from '../store/projectStore';
import { expectedOpenProject } from '../store/expectedOpenProject';
import { useSpeleoStore } from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
import { saveFailureOf, surfaceSaveFailure, surfaceSaveWarnings } from './saveFeedback';
import i18n from '../i18n';
import { invokeIpc } from './ipc';
import { projectSaveFilters, saveDialog, PROJECT_EXTENSION } from './fileDialogs';
import { resolveSavePath } from './overwriteGuard';
import type { ProjectIdentity } from '../types/ipc/ProjectIdentity';
import type { SaveOutcome } from '../types/ipc/SaveOutcome';

/**
 * Where the Save As starts: the default projects folder (`<Documents>/stalagmight`,
 * the one the new-project-for-import dialog offers) with the project name filled in,
 * sanitised the way both new-project dialogs sanitise it. Without a documents dir it
 * is the bare file name, which `saveDialog` resolves against the documents dir
 * itself when it can.
 */
async function unlinkDefaultPath(projectName: string): Promise<string> {
  const fileName = `${projectName.replace(/[^a-zA-Z0-9]/g, '_')}.${PROJECT_EXTENSION}`;
  try {
    const { documentDir } = await import('@tauri-apps/api/path');
    const dir = await documentDir();
    if (typeof dir === 'string' && dir.trim()) {
      const sep = dir.includes('\\') ? '\\' : '/';
      return `${dir.replace(/[\\/]+$/, '')}${sep}stalagmight${sep}${fileName}`;
    }
  } catch {
    // No documents dir: the bare name below.
  }
  return fileName;
}

/**
 * Ask where the unlinked copy goes: the Save As, then the overwrite ask for a
 * rewritten name (#576 D35), because the backend writes to the NORMALISED name,
 * which the OS prompt never saw. `null` when the user cancelled either one; nothing
 * has been called then. The write-path policy is the backend's, on the write.
 */
export async function chooseUnlinkDestination(projectName: string): Promise<string | null> {
  const selected = await saveDialog({
    filters: projectSaveFilters(),
    defaultPath: await unlinkDefaultPath(projectName),
  });
  if (!selected) return null;
  return resolveSavePath(selected, PROJECT_EXTENSION);
}

/**
 * Move the OPEN project out of the SpeleoDB cache to `destination` and land the
 * session on the moved copy. `speleodb_unlink` writes the unlinked copy there,
 * closes the backend session on the cache file, deletes the cache file and releases
 * a held lock best-effort; on any error nothing has changed. Because the backend
 * session is closed, the copy is then loaded here.
 *
 * Returns true once moved. A failed unlink is surfaced the way a failed save is
 * (its own Detach Failed title; it ran no integrity re-check). A failed load
 * surfaces its own error through `loadProject`; the move itself already happened.
 *
 * [#576 RVF-c-B4] `madeFor` is the project the user asked to move, captured BEFORE
 * the Save As (`expectedOpenProject()`): a load that committed while the dialog was
 * up must not have its copy moved under the name chosen for the first one. Defaults
 * to the project open now, for a caller that opened the copy itself just before.
 */
export async function unlinkOpenProjectTo(
  destination: string,
  madeFor: ProjectIdentity | null = expectedOpenProject(),
): Promise<boolean> {
  let outcome: SaveOutcome;
  try {
    // [#576 RVF-c-C1] Names the project to move: refused (nothing written) once
    // another is open.
    outcome = await invokeIpc('speleodb_unlink', { destination, expectedProject: madeFor });
  } catch (e) {
    surfaceSaveFailure(
      saveFailureOf(e, 'Detaching from SpeleoDB failed'),
      useUIStore.getState().openDialog,
      useProjectStore.getState(),
      { skipIntegrityRouting: true, title: i18n.t('dialogs.saveFailure.detachTitle') },
    );
    return false;
  }
  // Re-seed the shell as a plain local project (is_remote:false clears the link).
  await useSpeleoStore.getState().refreshLinkStatus();
  // What the copy went without, told the way every save tells it (#576 A3-D2).
  surfaceSaveWarnings(outcome.warnings);
  await useProjectStore.getState().loadProject(outcome.saved_path ?? destination);
  return true;
}
