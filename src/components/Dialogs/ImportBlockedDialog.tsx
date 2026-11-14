import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { isEditableMode, useUIStore } from '../../store/uiStore';
import { useExportStore } from '../../store/exportStore';
import { trackDialogComplete } from '../../utils/telemetry';
import { routeOpenedFile } from '../../utils/openOrImport';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';

/**
 * Raised when a DMP import is picked while a locally-fixable read-only state
 * blocks it (#243, reworded per #407 follow-up): a read-only mode, Plan or
 * Export (`cause: 'mode'`), or the closure-adjusted view in an editable mode,
 * Survey or Data (`cause: 'view'`).
 * Not an error - the user made a valid request one switch away from working -
 * so it offers the fix instead of reporting a failure: the switch button flips
 * whatever is read-only (mode, and/or view) and CONTINUES the import of the
 * already-picked file; "Stay in ..." abandons it. A Data user with the closure
 * view stays in Data: only the view switches.
 *
 * The one cause no local switch can fix - a missing SpeleoDB write lock -
 * keeps the plain error dialog (see routeOpenedFile's guard).
 */
export default function ImportBlockedDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const setAppMode = useUIStore((s) => s.setAppMode);
  const dialogData = useUIStore((s) => s.dialogData);
  const appMode = useUIStore((s) => s.appMode);
  // Export cannot be left while an export is being written (setAppMode
  // refuses), so the switch would only drop the view and then re-route into
  // the same block. Say why instead.
  const exportRunning = useExportStore((s) => s.exporting) && appMode === 'export';

  const rawPath = dialogData.path;
  const path = typeof rawPath === 'string' ? rawPath : null;
  const cause = dialogData.cause === 'view' ? 'view' : 'mode';

  const handleSwitch = useCallback(() => {
    // Completing (not cancelling) keeps the entry-point telemetry source alive
    // for the DMP flow the re-route below hands off to.
    trackDialogComplete('import-blocked');
    closeDialog();
    // Fix everything a switch can fix, whichever cause raised the prompt:
    // leave a read-only mode (Plan, Export) for Survey - an editable mode
    // (Survey, Data) stays where it is - then drop a (restored or current)
    // closure-adjusted view to measured - the same setter as the DisplayMenu
    // toggle, so the switch persists as the remembered view and clears the
    // geometry caches. Read the store fresh after setAppMode: it restores the
    // view from before the read-only mode, which may itself be closure-adjusted.
    if (!isEditableMode(useUIStore.getState().appMode)) setAppMode('survey');
    const ui = useUIStore.getState();
    if (ui.geometryMode === 'closureAdjusted') ui.setGeometryMode('measured');
    // Re-route the SAME picked file now that mode and view allow it. The
    // router re-checks every guard; the one cause a switch cannot fix (a
    // missing SpeleoDB write lock) surfaces as a TOAST, not a second modal -
    // the user just confirmed intent here, so no further click is owed.
    if (path) void routeOpenedFile(path, 'Import Failed', { residualReadOnlyToast: true });
  }, [closeDialog, setAppMode, path]);

  // Closing without switching abandons the import; closeDialog's cancel hook
  // drops the pending telemetry source.
  const handleStay = useCallback(() => {
    closeDialog();
  }, [closeDialog]);

  const stayLabel =
    cause === 'view'
      ? t('dialogs.importBlocked.stayInAdjusted')
      : appMode === 'export'
        ? t('dialogs.importBlocked.stayInExport')
        : t('dialogs.importBlocked.stayInPlan');
  const switchLabel =
    cause === 'view'
      ? t('dialogs.importBlocked.switchToMeasured')
      : t('dialogs.importBlocked.switchToSurvey');
  const body =
    cause === 'view' ? t('dialogs.importBlocked.bodyView') : t('dialogs.importBlocked.body');

  return (
    <DialogWrapper
      title={t('dialogs.importBlocked.title')}
      onClose={handleStay}
      size="md"
      data-testid="import-blocked-dialog"
    >
      <div className="space-y-4">
        <p className="text-text-secondary">{body}</p>
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleStay} data-testid="import-blocked-stay">
            {stayLabel}
          </Button>
          <Button
            variant="primary"
            onClick={handleSwitch}
            disabled={exportRunning}
            title={exportRunning ? t('exportPanel.inProgress') : undefined}
            data-testid="import-blocked-switch"
          >
            {switchLabel}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
