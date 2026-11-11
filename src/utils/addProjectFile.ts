/**
 * Add a `.stalx` into the open project (#674) and finish the way every other
 * import into an existing project does: mark dirty via `saveAfterImport`, offer
 * orphan placement for sections the add left unanchored, and surface the
 * receipt.
 *
 * The `add-or-open-project` dialog stays open (busy) for the whole call, so it
 * owns the screen while the backend copies and loads the source: the router,
 * Ctrl+O and the launch channel all refuse while a dialog is active. Every
 * successful ending closes or replaces THAT dialog - orphan placement chains
 * from it, `completeImportToSingleView` completes and closes it - so its
 * completion telemetry fires once, for the dialog that is actually open. A
 * failed command is returned for the dialog to show inline: the store's
 * catch-all toast yields to an open dialog.
 */
import { emptyCommandResult, useProjectStore } from '../store/projectStore';
import { useUIStore } from '../store/uiStore';
import { getErrorMessage, invokeIpc } from './ipc';
import { finalizeDeferredSaveWarnings, finishImportWithoutOrphanSuggestions } from './saveFeedback';
import type { CommandResult } from '../types/ipc/CommandResult';
import type { OrphanPlacementSuggestion } from '../types/ipc/OrphanPlacementSuggestion';

const DIALOG = 'add-or-open-project';

/** The open project's identity: `created_at` is stable across edits and differs per project. */
function openProjectKey(): string | null {
  return useProjectStore.getState().project?.created_at ?? null;
}

export async function runAddProjectFile(path: string): Promise<CommandResult> {
  // Captured at request time: the tail (save state, orphan placement, receipt)
  // belongs to THIS project and must not land on whatever replaced it.
  const requestKey = openProjectKey();
  const stillOpen = () => requestKey !== null && openProjectKey() === requestKey;
  const dropContext = () => useUIStore.getState().setImportContext(null);

  useUIStore.getState().setImportContext('existing-project');
  let result: CommandResult;
  try {
    result = await useProjectStore.getState().addProjectFile(path);
  } catch (e) {
    // A transport rejection, not a returned failure: report it the same way.
    result = emptyCommandResult(false, getErrorMessage(e));
  }
  if (!result.success || !stillOpen()) {
    dropContext();
    return result;
  }
  const { closeDialog, setView, triggerFitToBounds, openDialog, showToast } = useUIStore.getState();
  const importUi = { closeDialog, setView, triggerFitToBounds, openDialog, showToast };
  // A never-saved project is written by the add itself (`finalize_deferred_import`);
  // if that save failed, say so and stop, like every other import route.
  if (finalizeDeferredSaveWarnings(result.warnings, DIALOG, importUi, result.import_receipt)) return result;
  if (!(await useProjectStore.getState().saveAfterImport())) {
    dropContext();
    return result;
  }
  const newSectionIds = result.section_ids;
  if (newSectionIds.length > 0) {
    let suggestions: OrphanPlacementSuggestion[] = [];
    try {
      suggestions = await invokeIpc<OrphanPlacementSuggestion[]>(
        'suggest_orphan_placements',
        { newlyImportedSectionIds: newSectionIds },
        { purpose: 'suggest orphan placements after adding a project file' },
      );
    } catch {
      // Non-fatal: proceed without orphan detection
    }
    if (!stillOpen()) {
      dropContext();
      return result;
    }
    if (suggestions.length > 0) {
      openDialog('orphan-placement', { suggestions, receipt: result.import_receipt });
      return result;
    }
  }
  // Re-checked inside the helper after its count IPC, right before completing.
  await finishImportWithoutOrphanSuggestions(DIALOG, importUi, newSectionIds, result.import_receipt, () => {
    const current = stillOpen();
    if (!current) dropContext();
    return current;
  });
  return result;
}
