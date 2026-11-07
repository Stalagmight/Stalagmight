import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useProject } from '../../hooks/useProject';
import { abandonUpdateRestart, performQuit } from '../../utils/quit';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';

export default function UnsavedChangesDialog() {
  const { closeDialog, setView, openDialog, dialogData } = useUIStore();
  const { closeProject, loadProject } = useProjectStore();
  const { handleSaveProject } = useProject();

  // Set by handleOpenProject when discarding the current project to open another
  // one: on discard we load this project over the dirty one instead of going
  // back to the global view.
  const pendingOpenPath = typeof dialogData?.pendingOpenPath === 'string' ? dialogData.pendingOpenPath : null;
  // Set by handleNewProject when discarding to start a fresh project: on discard
  // we open the new-project dialog over the current (dirty) one - createProject
  // replaces it. Backing out of that dialog leaves the current project untouched.
  const pendingNewProject = dialogData?.pendingNewProject === true;
  // Set by quitApp when the app is quitting with unsaved changes: Discard quits,
  // Save saves then quits, Cancel stays. Unlike the New/Open paths, quit offers a
  // Save option so the user isn't forced to choose between losing work and staying.
  const pendingQuit = dialogData?.pendingQuit === true;
  // Set by `openExitPrompt` when this prompt displaced a dialog in the single
  // `activeDialog` slot. Cancelling leaves the app running, so that dialog - which
  // may be a multi-step import wizard whose state lives in its `dialogData` - has to
  // come back rather than silently vanish. Absent on the New/Open paths, which are
  // raised from a bare view.
  const cancelReturnsTo =
    typeof dialogData?.cancelReturnsTo === 'string' ? dialogData.cancelReturnsTo : null;
  const cancelReturnsData =
    typeof dialogData?.cancelReturnsData === 'object' && dialogData.cancelReturnsData !== null
      ? (dialogData.cancelReturnsData as Record<string, unknown>)
      : undefined;

  const handleDiscard = async () => {
    // Concurrency guard is handled at store level (store.isClosing)
    // This prevents double-click and race conditions with Back button
    trackDialogComplete('unsaved-changes');
    closeDialog();
    if (pendingQuit) {
      // Discard the unsaved edits and quit the app.
      performQuit();
      return;
    }
    if (pendingNewProject) {
      // Discard by opening the new-project dialog over the current (dirty) one.
      openDialog('new-project');
      return;
    }
    if (pendingOpenPath) {
      // Discard by loading the chosen project over the current (dirty) one.
      await loadProject(pendingOpenPath);
      return;
    }
    const result = await closeProject();
    if (result.success) {
      setView('global');
    }
  };

  const handleSaveAndQuit = async () => {
    // Save the project, then quit only if the save actually succeeded. On a
    // failed/blocked/cancelled save, handleSaveProject already surfaced the
    // outcome; the app stays open with the project intact (no data lost).
    trackDialogComplete('unsaved-changes');
    closeDialog();
    const result = await handleSaveProject();
    if (result && result.success) {
      performQuit();
    }
  };

  // Cancel / X / backdrop only. Discard and Save & Quit both end in `performQuit()`
  // or a load that replaces the screen, where putting the old dialog back would be
  // pointless at best.
  const handleCancel = () => {
    // [#579] Cancelling here abandons the whole exit, including an update
    // restart that was riding on it - otherwise the arm survives, and the next
    // ordinary quit the user makes silently installs the update instead. No-op
    // (and no IPC) when no restart is in flight, which is the usual case.
    abandonUpdateRestart();
    if (cancelReturnsTo) openDialog(cancelReturnsTo, cancelReturnsData);
    else closeDialog();
  };

  return (
    <DialogWrapper title="Unsaved Changes" onClose={handleCancel} size="md" data-testid="unsaved-changes-dialog">
      <div className="space-y-4">
        <p>
          {pendingQuit
            ? 'You have unsaved changes. Quit anyway?'
            : pendingNewProject
            ? 'You have unsaved changes. Create a new project anyway?'
            : pendingOpenPath
            ? 'You have unsaved changes. Open another project anyway?'
            : 'You have unsaved changes. Are you sure you want to go back?'}
        </p>
        <p className="text-text-secondary text-sm">
          {pendingQuit ? 'Save your changes, or discard them and quit.' : 'Your changes will be lost.'}
        </p>

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleCancel} data-testid="unsaved-changes-cancel">Cancel</Button>
          <Button
            data-testid="unsaved-changes-discard"
            variant="danger"
            onClick={handleDiscard}
            {...(pendingQuit ? {} : { 'data-primary-action': '' })}
          >
            {pendingQuit ? 'Discard & Quit' : 'Discard Changes'}
          </Button>
          {pendingQuit && (
            <Button variant="primary" onClick={handleSaveAndQuit} data-testid="unsaved-changes-save-quit">
              Save &amp; Quit
            </Button>
          )}
        </div>
      </div>
    </DialogWrapper>
  );
}
