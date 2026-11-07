import { useState } from 'react';
import { Trans, useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useProject } from '../../hooks/useProject';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';
import { surfaceSaveFailure } from '../../utils/saveFeedback';
import { isPermissionError } from '../../utils/saveErrors';
import { useShallow } from 'zustand/react/shallow';

/**
 * [#576 RV6b-F4] The file name inside the message's `<name>` tag. It renders its
 * own `value` and ignores the children `<Trans>` hands it: Trans interpolates the
 * string, then each of its text nodes AGAIN with the same values, so a name that
 * itself contained "{{fileName}}" came out substituted twice.
 */
function FileName({ value }: { value: string }) {
  return <span className="font-medium text-text-primary">{value}</span>;
}

export default function OverwriteConfirmDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, openDialog: showDialog } = useUIStore(
    useShallow((s) => ({ closeDialog: s.closeDialog, dialogData: s.dialogData, openDialog: s.openDialog })),
  );
  const saveProject = useProjectStore((s) => s.saveProject);
  const { handleCloseProject } = useProject();
  const [isLoading, setIsLoading] = useState(false);

  const path = dialogData.path as string | undefined;
  const fileName = dialogData.fileName as string | undefined;
  // "Save and Close" defers here when the chosen file exists; once the overwrite
  // saves, finish the close it started (otherwise the close half is dropped).
  const closeAfterSave = dialogData.closeAfterSave === true;
  // [#576 E10-D1] Save As could not tell whether the file exists (the probe
  // failed), so it asks rather than saving over it unasked. Same question, but
  // it must not claim the file is there.
  const existenceUnknown = dialogData.existenceUnknown === true;

  const handleConfirm = async () => {
    if (!path) {
      trackDialogComplete('confirm-overwrite');
      closeDialog();
      return;
    }

    setIsLoading(true);
    try {
      const result = await saveProject(path);
      // Guard on `!success`, NOT on a present error: the save did NOT happen, so
      // it must never fall through to the success branch (which closes the
      // project on closeAfterSave and would silently discard unsaved edits). An
      // integrity block is signaled via integritySaveBlocked and surfaced by
      // surfaceSaveFailure below.
      if (result && !result.success) {
        // On a permission error, saveProject has already opened the folder-access
        // guidance dialog; don't close it, stack a generic error, or record the
        // overwrite as completed (it was superseded by the guidance dialog).
        if (isPermissionError(result.code)) {
          return;
        }
        trackDialogComplete('confirm-overwrite');
        closeDialog();
        surfaceSaveFailure(result, showDialog, useProjectStore.getState());
      } else {
        // Save succeeded.
        trackDialogComplete('confirm-overwrite');
        closeDialog();
        // A managed project's explicit-destination save writes a detached COPY;
        // the open project itself was NOT saved, so "Save and Close" must not
        // proceed to close it (that would discard the unsaved edits the save
        // appeared to persist). Unreachable today - saveAndClose only routes
        // through Save As when the project has no path, and a managed project
        // always has one - but the failure mode is silent data loss, so gate it.
        if (closeAfterSave && !result?.unlinked_copy_path) {
          // Project is now saved (not dirty), so this closes straight to GlobalView.
          void handleCloseProject();
        }
      }
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <DialogWrapper title={t('dialogs.overwriteConfirm.title')} onClose={closeDialog} size="md" data-testid="overwrite-confirm-dialog">
      <div className="space-y-4">
        {/* #576 RV2-FE-5: both variants' copy lives in en.json (D20-D1). */}
        <p
          className="text-text-secondary"
          data-testid={existenceUnknown ? 'overwrite-confirm-unknown' : 'overwrite-confirm-message'}
        >
          {/* [#576 NF-W6-1] The file name is user text: escaped before Trans parses
              the string for its tags, unescaped after, so "CaveA <B>.stalx" cannot
              open a tag (the DeleteProjectDialog rule). [#576 RV6b-F4] What shows is
              `FileName`'s own value, never the re-interpolated tag content. */}
          <Trans
            i18nKey={existenceUnknown ? 'dialogs.overwriteConfirm.messageUnknown' : 'dialogs.overwriteConfirm.message'}
            values={{ fileName }}
            tOptions={{ interpolation: { escapeValue: true } }}
            shouldUnescape
            components={{ name: <FileName value={fileName ?? ''} /> }}
          />
        </p>

        <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
          {existenceUnknown
            ? t('dialogs.overwriteConfirm.warningUnknown')
            : t('dialogs.overwriteConfirm.warning')}
        </div>

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="overwrite-confirm-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button variant="danger" onClick={handleConfirm} disabled={isLoading} data-primary-action="" data-testid="overwrite-confirm-accept">
            {isLoading ? t('dialogs.overwriteConfirm.saving') : t('dialogs.overwriteConfirm.overwrite')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
