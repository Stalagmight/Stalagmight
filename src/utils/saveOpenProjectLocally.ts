import { useProjectStore } from '../store/projectStore';
import { useUIStore } from '../store/uiStore';
import { getErrorMessage } from './ipc';
import { surfaceSaveFailure } from './saveFeedback';

/**
 * Write the open project to its own file, surfacing a failure the usual way. The
 * backend `save_project` has no edit gate, so this works for a linked SpeleoDB
 * project whatever the lock state - which is what keeps edits from ever being
 * discard-only when SpeleoDB cannot take them (frontend#6).
 *
 * Kept apart from `dispatchProjectSave` (which needs the Save As flow and its
 * file dialogs) so the stores' exit guards can use it without pulling those in.
 */
export async function saveOpenProjectLocally() {
  const ui = useUIStore.getState();
  try {
    const result = await useProjectStore.getState().saveProject();
    if (result && !result.success) surfaceSaveFailure(result.error, ui.openDialog);
    return result;
  } catch (error) {
    const message = getErrorMessage(error, 'Failed to save');
    surfaceSaveFailure(message, ui.openDialog);
    return { success: false, error: message };
  }
}
