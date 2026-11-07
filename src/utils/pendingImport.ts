/**
 * #576 HFI-4-D2 "prompt after a good parse": "Import as new project" over a project
 * whose close must ask first (unsaved changes, a SpeleoDB hand-back) parses the file
 * with the project still open and only then asks (`routeOpenedFile`). The prompt
 * carries the parsed file as `pendingImport` - the `new-project-for-import` dialog data
 * the router would have opened - and its own close opens that dialog with it
 * (`UnsavedChangesDialog`, `handBackProject`). Cancelling the prompt keeps the project
 * and drops the file.
 */
import { useUIStore } from '../store/uiStore';

/** The `new-project-for-import` dialog data of a parsed import-as-new file. */
export type PendingImport = Record<string, unknown>;

/** A `pendingImport` read back out of untyped `dialogData`: an object, or null. */
export function readPendingImport(raw: unknown): PendingImport | null {
  return typeof raw === 'object' && raw !== null && !Array.isArray(raw) ? (raw as PendingImport) : null;
}

/**
 * Open the new-project dialog for a parsed file. The project it creates exists only for
 * this import, so the import dialog may offer to discard it again (#411 C2).
 */
export function openPendingImport(pending: PendingImport): void {
  const ui = useUIStore.getState();
  ui.setImportContext('new-project');
  ui.openDialog('new-project-for-import', pending);
}
