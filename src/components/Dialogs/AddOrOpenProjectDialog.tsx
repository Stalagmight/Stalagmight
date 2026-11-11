import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useEditingBlocked, isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete, clearProjectActionSource } from '../../utils/telemetry';
import { routeOpenedFile } from '../../utils/openOrImport';
import { runAddProjectFile } from '../../utils/addProjectFile';

/**
 * Raised when a `.stalx` is picked through an in-project Import door (#674):
 * either open it as a project (the existing guarded open, re-routed through
 * `routeOpenedFile` so the SpeleoDB hand-back and unsaved-changes prompts still
 * apply) or add its survey data to the open project as one undo step.
 *
 * Add is a mutation, so it is unavailable while editing is blocked; Open as
 * project stays available because it replaces the project rather than editing it.
 * Add keeps this dialog open and busy until it resolves; see `handleAdd`.
 */
export default function AddOrOpenProjectDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  const projectName = useProjectStore((s) => s.project?.name);
  const editingBlocked = useEditingBlocked();
  const [adding, setAdding] = useState(false);
  // State is read through the render closure, so two clicks dispatched in one JS
  // task both see `adding === false`; the ref is set synchronously and is the guard.
  const addingRef = useRef(false);
  const [addError, setAddError] = useState<string | null>(null);

  const path = dialogData.path as string | undefined;
  const fileName = dialogData.fileName as string | undefined;
  if (!path || !fileName) return null;

  const handleCancel = () => {
    clearProjectActionSource();
    closeDialog();
  };

  const handleOpen = () => {
    trackDialogComplete('add-or-open-project');
    closeDialog();
    void routeOpenedFile(path, 'Open Failed', { swapConfirmed: true });
  };

  // Stay open, busy, until the add resolves (like LaunchOpenConfirmDialog): an
  // active dialog is what makes the router, Ctrl+O and the launch channel refuse,
  // and the add copies and loads a whole project. `runAddProjectFile` closes or
  // replaces this dialog on success, so still being here means it did not land.
  const handleAdd = async () => {
    if (addingRef.current || isEditingBlockedNow()) return;
    addingRef.current = true;
    setAdding(true);
    setAddError(null);
    const result = await runAddProjectFile(path);
    if (useUIStore.getState().activeDialog !== 'add-or-open-project') return;
    if (result.success) {
      // Its project moved while the add ran, so the tail was dropped.
      closeDialog();
      return;
    }
    addingRef.current = false;
    setAdding(false);
    setAddError(result.error ?? t('editGate.commandFailed'));
  };

  const addLabel = adding
    ? t('dialogs.addOrOpenProject.adding')
    : t('dialogs.addOrOpenProject.addTo', { projectName: projectName ?? '' });

  return (
    <DialogWrapper
      title={t('dialogs.addOrOpenProject.title')}
      onClose={adding ? () => {} : handleCancel}
      size="md"
      data-testid="add-or-open-project-dialog"
    >
      <div className="space-y-3">
        <p className="text-sm text-text-secondary">
          {t('dialogs.addOrOpenProject.body', { fileName })}
        </p>
        {editingBlocked && (
          <p className="text-sm text-text-secondary" data-testid="add-or-open-project-blocked">
            {t('dialogs.addOrOpenProject.blockedNotice')}
          </p>
        )}
        {addError && (
          <p className="text-sm text-error" role="alert" data-testid="add-or-open-project-error">
            {addError}
          </p>
        )}
        <div className="flex flex-wrap justify-end gap-2 pt-2 border-t border-border">
          <Button
            onClick={handleCancel}
            disabled={adding}
            data-autofocus
            data-testid="add-or-open-project-cancel"
          >
            {t('common.actions.cancel')}
          </Button>
          <Button
            onClick={handleOpen}
            disabled={adding}
            className="whitespace-nowrap"
            data-testid="add-or-open-project-open"
          >
            {t('dialogs.addOrOpenProject.openAsProject')}
          </Button>
          <Button
            variant="primary"
            onClick={() => void handleAdd()}
            disabled={editingBlocked || adding}
            className="min-w-0 max-w-full"
            title={addLabel}
            data-testid="add-or-open-project-add"
          >
            {/* A long project name ellipsizes here; the full name is in the body and the tooltip. */}
            <span className="truncate" data-testid="add-or-open-project-add-label">
              {addLabel}
            </span>
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
