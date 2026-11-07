import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useProject } from '../../hooks/useProject';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { clearProjectActionSource } from '../../utils/telemetry';
import { routeOpenedFile } from '../../utils/openOrImport';

/**
 * Confirm dialog (#391) shown instead of the old close-first dead end when a
 * new-project-only format (GPX/Ariane/Compass/Walls) is picked while a project
 * is already open.
 *
 * Confirming parses the file FIRST, with the current project still open and this
 * dialog showing the parse in progress (#576 HFI-4-D1). Only a successful parse
 * closes the project - through the SAME guarded path as File > Close - and the
 * new-project dialog then replaces this one. A failed parse shows its error and
 * leaves the project untouched. Cancelling while the file parses drops the late
 * result, as does a project switch under the dialog (`routeOpenedFile`'s
 * version guard). It used to close the project first and then parse with
 * nothing on screen, so a large file left an empty home screen for seconds and
 * the new-project dialog arrived late - over a project opened meanwhile, too.
 *
 * A close that must ask first (unsaved changes, SpeleoDB hand-back) asks only
 * after a good parse (#576 HFI-4-D2), replacing this dialog and carrying the
 * parsed file: its own close opens the new-project dialog with it, its Cancel
 * keeps the project and drops the file. A failed parse asks nothing.
 */
export default function ImportAsNewProjectDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  const projectName = useProjectStore((s) => s.project?.name);
  const { handleCloseProject } = useProject();
  const [parsing, setParsing] = useState(false);

  const path = dialogData.path as string | undefined;
  const fileName = dialogData.fileName as string | undefined;
  if (!path || !fileName) return null;

  // Cancel / backdrop-close / a non-continuing exit all abandon this import
  // outright, so the pending entry-point source (handed off by openOrImport's
  // `handedOff = true`) must be cleared here - otherwise it survives to
  // mis-attribute a later, unrelated project command. Cancelling while the file
  // parses is the same abandon: the router drops the parse when it settles.
  const handleAbandon = () => {
    clearProjectActionSource();
    closeDialog();
  };

  // Every way out of the router hands this dialog's slot on (the new-project
  // dialog, the error dialog, a close prompt) or takes it down, completing or
  // cancelling it through `openDialog` / `closeDialog`'s own bookkeeping. The
  // one that leaves it up - a close refused without a prompt - gets the confirm
  // back.
  const handleConfirm = async () => {
    setParsing(true);
    try {
      await routeOpenedFile(path, undefined, { importAsNew: { closeCurrent: handleCloseProject } });
    } finally {
      setParsing(false);
    }
  };

  return (
    <DialogWrapper
      title={t('dialogs.importAsNewProject.title')}
      onClose={handleAbandon}
      size="sm"
      data-testid="import-as-new-project-dialog"
    >
      <div className="space-y-3">
        <p className="text-sm text-text-secondary">
          {t('dialogs.importAsNewProject.body', { fileName })}
        </p>
        {projectName && (
          <p className="text-sm text-text-secondary">
            {t('dialogs.importAsNewProject.closeNotice', { projectName })}
          </p>
        )}
        {parsing && (
          <p
            className="flex items-center gap-2 text-sm text-text-secondary"
            role="status"
            data-testid="import-as-new-project-parsing"
          >
            <span
              className="h-4 w-4 shrink-0 animate-spin rounded-full border-2 border-accent border-t-transparent"
              aria-hidden="true"
            />
            {t('dialogs.importAsNewProject.parsing', { fileName })}
          </p>
        )}
        <div className="flex justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={handleAbandon} data-testid="import-as-new-project-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button
            variant="primary"
            onClick={() => void handleConfirm()}
            disabled={parsing}
            data-testid="import-as-new-project-confirm"
          >
            {t('dialogs.importAsNewProject.confirm')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
