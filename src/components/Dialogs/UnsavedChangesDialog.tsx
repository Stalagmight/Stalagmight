import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useProject } from '../../hooks/useProject';
import { abandonUpdateRestart, performQuit } from '../../utils/quit';
import { holdAutosave } from '../../utils/autosaveHold';
import { openPendingImport, readPendingImport } from '../../utils/pendingImport';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';
import { useShallow } from 'zustand/react/shallow';

export default function UnsavedChangesDialog() {
  const { t } = useTranslation();
  const { closeDialog, setView, openDialog, dialogData } = useUIStore(
    useShallow((s) => ({
      closeDialog: s.closeDialog,
      setView: s.setView,
      openDialog: s.openDialog,
      dialogData: s.dialogData,
    })),
  );
  const { closeProject, loadProject } = useProjectStore(
    useShallow((s) => ({ closeProject: s.closeProject, loadProject: s.loadProject })),
  );
  const { handleSaveProject } = useProject();
  const projectName = useProjectStore((s) => s.project?.name ?? '');

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
  // [#576 HFI-4-D2] Set by "Import as new project" after the file parsed: Discard
  // closes the project and opens the new-project dialog with the parsed file;
  // Cancel keeps the project and drops the file.
  const pendingImport = readPendingImport(dialogData?.pendingImport);
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
    // [#576 D34] This prompt holds autosave while it is open (utils/autosaveHold);
    // this hold carries that across the switch the discard starts, so the edits
    // just discarded are not written to the old file while it runs. The discard
    // takes effect only when the switch lands: a load that fails leaves the
    // project open with its edits, and the release re-arms autosave for them.
    const release = holdAutosave();
    try {
      closeDialog();
      if (pendingQuit) {
        // Discard the unsaved edits and quit the app.
        performQuit();
        return;
      }
      if (pendingNewProject) {
        // Discard by opening the new-project dialog over the current (dirty) one.
        // That dialog is itself a leave prompt, so the hold continues until it
        // creates the project or is cancelled.
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
        if (pendingImport) openPendingImport(pendingImport);
      }
    } finally {
      release();
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
    <DialogWrapper title={t('dialogs.unsavedChanges.title')} onClose={handleCancel} size="md" data-testid="unsaved-changes-dialog">
      <div className="space-y-4">
        <p>
          {pendingQuit
            ? t('dialogs.unsavedChanges.messageQuit')
            : pendingNewProject
            ? t('dialogs.unsavedChanges.messageNewProject')
            : pendingOpenPath
            ? t('dialogs.unsavedChanges.messageOpenProject')
            : pendingImport
            ? t('dialogs.unsavedChanges.messageImportAsNew')
            : t('dialogs.unsavedChanges.messageBack')}
        </p>
        <p className="text-text-secondary text-sm">
          {pendingQuit
            ? t('dialogs.unsavedChanges.noteQuit')
            : pendingImport
            ? t('dialogs.unsavedChanges.noteImportAsNew', { project: projectName })
            : t('dialogs.unsavedChanges.note')}
        </p>

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleCancel} data-testid="unsaved-changes-cancel">{t('common.actions.cancel')}</Button>
          <Button
            data-testid="unsaved-changes-discard"
            variant="danger"
            onClick={handleDiscard}
            {...(pendingQuit ? {} : { 'data-primary-action': '' })}
          >
            {pendingQuit ? t('dialogs.unsavedChanges.discardQuit') : t('dialogs.unsavedChanges.discard')}
          </Button>
          {pendingQuit && (
            <Button variant="primary" onClick={handleSaveAndQuit} data-testid="unsaved-changes-save-quit">
              {t('dialogs.unsavedChanges.saveQuit')}
            </Button>
          )}
        </div>
      </div>
    </DialogWrapper>
  );
}
