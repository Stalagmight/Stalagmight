import { useState } from 'react';
import { Trans, useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { useShallow } from 'zustand/react/shallow';

/**
 * Destructive confirm for permanently deleting a project from disk (#118).
 *
 * Distinct from "Remove from recent" (which only forgets the entry): this hands
 * the project's `.stalx` to the OS delete - the Trash where the platform provides
 * one, otherwise a permanent remove. Opened via
 * `openDialog('delete-project', { path, name })` from the GlobalView context menu
 * and the metadata panel.
 *
 * Mirrors OverwriteConfirmDialog to match the sibling confirm dialogs. Cancel
 * takes default focus (not Delete) since the action is irreversible.
 */
export default function DeleteProjectDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, showToast } = useUIStore(
    useShallow((s) => ({ closeDialog: s.closeDialog, dialogData: s.dialogData, showToast: s.showToast })),
  );
  const deleteProject = useProjectStore((s) => s.deleteProject);
  const [isDeleting, setIsDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const path = dialogData.path as string | undefined;
  // Prefer the display name passed by the opener; fall back to the file stem.
  const name =
    (dialogData.name as string | undefined) ||
    path?.split(/[\\/]/).pop()?.replace(/\.stalx$/i, '') ||
    t('dialogs.deleteProject.thisProject');

  // A remote (SpeleoDB) recent is only a local cache copy: deleting it leaves the
  // project on the server (re-openable via Open from SpeleoDB), so we frame it as a
  // local-copy delete rather than a permanent, unrecoverable one.
  const isRemote = Boolean(dialogData.isRemote);

  const handleConfirm = async () => {
    if (!path) {
      // No-op completion (the Delete button is disabled without a path, but be
      // explicit so cancel-vs-complete is reported accurately on every exit).
      trackDialogComplete('delete-project');
      closeDialog();
      return;
    }
    setIsDeleting(true);
    setError(null);
    const result = await deleteProject(path);
    if (result.success) {
      // A SUCCESSFUL delete can still carry warnings. Once the `.stalx` is
      // trashed the backend refuses to report the whole call as a failure (that
      // would strand a "File not found" zombie row), so everything after that
      // point degrades to a warning - today `delete_project_persist_failed`,
      // where the file is gone but the recent-projects list could not be
      // persisted, so the entry silently reappears on the next launch. This
      // dialog is the only thing watching the result, and it is about to close,
      // so a warning nobody surfaces here is a warning nobody ever sees. Toast
      // the backend's own wording rather than paraphrasing it - it carries the
      // OS error detail.
      const warnings = result.warnings ?? [];
      if (warnings.length > 0) {
        showToast(warnings.map((w) => w.message).join(' '), TOAST_KEYS.DELETE_PROJECT_WARNING, 'warning');
      }
      // Mark the dialog completed BEFORE closing: closeDialog() runs
      // checkDialogCancel('delete-project'), which would otherwise record a
      // successful delete as a cancel (mirrors OverwriteConfirmDialog).
      trackDialogComplete('delete-project');
      closeDialog();
      return;
    }
    // Keep the dialog open so the user sees why it failed and can retry/cancel.
    setIsDeleting(false);
    setError(result.error ?? t('dialogs.deleteProject.errors.deleteFailed'));
  };

  return (
    <DialogWrapper
      title={isRemote ? t('dialogs.deleteProject.titleRemote') : t('dialogs.deleteProject.title')}
      onClose={closeDialog}
      size="md"
      data-testid="delete-project-dialog"
    >
      <div className="space-y-4">
        <p className="text-text-secondary">
          {/* The name and path are user text: escaped before Trans parses the
              string for its tags, unescaped after, so "A <B>" renders as typed. */}
          <Trans
            i18nKey={isRemote ? 'dialogs.deleteProject.questionRemote' : 'dialogs.deleteProject.question'}
            values={{ name }}
            tOptions={{ interpolation: { escapeValue: true } }}
            shouldUnescape
            components={{ name: <span className="font-medium text-text-primary" /> }}
          />
        </p>

        <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
          <Trans
            i18nKey="dialogs.deleteProject.localFile"
            values={{ path: path ?? name }}
            tOptions={{ interpolation: { escapeValue: true } }}
            shouldUnescape
            components={{ path: <span className="font-medium break-all" /> }}
          />{' '}
          {isRemote
            ? t('dialogs.deleteProject.remoteNote')
            : t('dialogs.deleteProject.cannotUndo')}
        </div>

        {error && (
          <div className="text-error text-sm" data-testid="delete-project-error">
            {error}
          </div>
        )}

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={isDeleting} data-testid="delete-project-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button
            variant="danger"
            onClick={handleConfirm}
            disabled={isDeleting || !path}
            data-testid="delete-project-confirm"
          >
            {isDeleting ? t('dialogs.deleteProject.deleting') : isRemote ? t('dialogs.deleteProject.confirmRemote') : t('dialogs.deleteProject.confirm')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
