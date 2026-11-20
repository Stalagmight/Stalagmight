import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';
import { saveProjectAs } from '../../utils/projectActions';
import { surfaceSaveFailure } from '../../utils/saveFeedback';
import { classifySaveError } from '../../utils/saveErrors';
import { extractFilename } from '../../utils/path';
import { getErrorMessage } from '../../utils/ipc';

type Choice = 'overwrite' | 'saveAs' | 'reload';

/**
 * The save prompt for a file another program changed since Stalagmight opened
 * it (`.stalx` v2, plan 6.5's external-modification guard): the save was
 * refused with `EXTERNALLY_MODIFIED` and nothing was written. The user decides
 * which version to keep, so this is a modal moment (docs/notifications.md:
 * data at risk), with three answers:
 *
 * - Keep my version: `save_project_overwriting` writes over the changed file.
 * - Save as a new file: the Save As flow, which leaves the changed file alone
 *   and moves the session to the new one.
 * - Use the file's version: reopen the file as it is now, discarding the
 *   unsaved changes. Not offered for a SpeleoDB project: its copy is managed
 *   by the sync, which has its own reload.
 *
 * "Decide later" keeps editing; autosave stays paused (the autosave banner
 * says why) until a save lands.
 */
export default function SaveConflictDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const openDialog = useUIStore((s) => s.openDialog);
  const filePath = useProjectStore((s) => s.project?.file_path ?? null);
  const linked = useSpeleoStore((s) => s.link !== null);
  const [busy, setBusy] = useState<Choice | null>(null);
  const [error, setError] = useState<string | null>(null);

  const fileName = filePath ? extractFilename(filePath) : t('dialogs.saveConflict.thisFile');

  const finish = () => {
    trackDialogComplete('save-conflict');
    closeDialog();
  };

  const run = async (choice: Choice) => {
    setBusy(choice);
    setError(null);
    try {
      if (choice === 'overwrite') {
        const result = await useProjectStore.getState().saveProjectOverwriting();
        if (result.success) {
          finish();
        } else if (classifySaveError(result.error) === 'externally-modified') {
          // Cannot happen (the overwrite skips the guard); say so rather than loop.
          setError(t('dialogs.saveConflict.failed', { error: result.error ?? '' }));
        } else {
          finish();
          surfaceSaveFailure(result.error, openDialog);
        }
        return;
      }
      if (choice === 'saveAs') {
        // Save As opens its own dialogs (the picker, the overwrite confirm, a
        // failure); this prompt steps aside first so they are not stacked on it.
        finish();
        await saveProjectAs();
        return;
      }
      const path = filePath;
      if (!path) return;
      // The path is the open one, so a plain load would be refused as already
      // open and change nothing. Close the session first: the user chose to
      // lose the unsaved changes, and the close drops them without saving.
      const closed = await useProjectStore.getState().closeProject();
      if (!closed.success) {
        setError(t('dialogs.saveConflict.failed', { error: closed.error ?? '' }));
        return;
      }
      finish();
      await useProjectStore.getState().loadProject(path);
    } catch (e) {
      setError(t('dialogs.saveConflict.failed', { error: getErrorMessage(e, String(e)) }));
    } finally {
      setBusy(null);
    }
  };

  return (
    <DialogWrapper
      title={t('dialogs.saveConflict.title')}
      onClose={closeDialog}
      size="md"
      closeOnBackdropClick={false}
      data-testid="save-conflict-dialog"
    >
      <div className="space-y-4">
        <p className="text-text-secondary" data-testid="save-conflict-lead">
          {t('dialogs.saveConflict.lead', { fileName })}
        </p>
        <p className="text-sm text-text-secondary">{t('dialogs.saveConflict.unsaved')}</p>

        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm" data-testid="save-conflict-error">
            {error}
          </div>
        )}

        <ul className="space-y-2">
          <li className="flex items-start justify-between gap-3">
            <span className="text-sm text-text-secondary pt-1">{t('dialogs.saveConflict.overwriteDetail')}</span>
            <Button
              variant="primary"
              onClick={() => void run('overwrite')}
              disabled={busy !== null}
              data-primary-action=""
              data-testid="save-conflict-overwrite"
            >
              {busy === 'overwrite' ? t('dialogs.saveConflict.working') : t('dialogs.saveConflict.overwrite')}
            </Button>
          </li>
          {!linked && (
            <li className="flex items-start justify-between gap-3">
              <span className="text-sm text-text-secondary pt-1">{t('dialogs.saveConflict.saveAsDetail')}</span>
              <Button onClick={() => void run('saveAs')} disabled={busy !== null} data-testid="save-conflict-save-as">
                {t('dialogs.saveConflict.saveAs')}
              </Button>
            </li>
          )}
          {!linked && filePath && (
            <li className="flex items-start justify-between gap-3">
              <span className="text-sm text-text-secondary pt-1">{t('dialogs.saveConflict.reloadDetail')}</span>
              <Button variant="danger" onClick={() => void run('reload')} disabled={busy !== null} data-testid="save-conflict-reload">
                {t('dialogs.saveConflict.reload')}
              </Button>
            </li>
          )}
        </ul>

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={busy !== null} data-testid="save-conflict-cancel">
            {t('dialogs.saveConflict.cancel')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
