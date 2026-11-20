import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { trackDialogComplete } from '../../utils/telemetry';
import { routeOpenedFile } from '../../utils/openOrImport';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';

/**
 * Raised when a DMP import is picked while a locally-fixable read-only state
 * blocks it (#243, reworded per #407 follow-up): Plan/Export mode
 * (`cause: 'mode'`) or the closure-adjusted view in Survey (`cause: 'view'`).
 * Not an error - the user made a valid request one switch away from working -
 * so it offers the fix instead of reporting a failure: the switch button flips
 * whatever is read-only (mode, and/or view) and CONTINUES the import of the
 * already-picked file; "Stay in ..." abandons it.
 *
 * A file made with a newer Stalagmight (`cause: 'file'`, plan 6.4) is
 * read-only whatever the mode or view, so that prompt explains why and offers
 * no switch: a switch would re-route into the same guard and reopen it.
 *
 * The other cause no local switch can fix - a missing SpeleoDB write lock -
 * keeps the plain error dialog (see routeOpenedFile's guard).
 */
export default function ImportBlockedDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const setAppMode = useUIStore((s) => s.setAppMode);
  const dialogData = useUIStore((s) => s.dialogData);
  const appMode = useUIStore((s) => s.appMode);

  const rawPath = dialogData.path;
  const path = typeof rawPath === 'string' ? rawPath : null;
  const cause = dialogData.cause === 'view' || dialogData.cause === 'file' ? dialogData.cause : 'mode';

  const handleSwitch = useCallback(() => {
    // Completing (not cancelling) keeps the entry-point telemetry source alive
    // for the DMP flow the re-route below hands off to.
    trackDialogComplete('import-blocked');
    closeDialog();
    // Fix everything a switch can fix, whichever cause raised the prompt:
    // leave Plan/Export, then drop a (restored or current) closure-adjusted
    // view to measured - the same setter as the DisplayMenu toggle, so the
    // switch persists as the remembered view and clears the geometry caches.
    // Read the store fresh after setAppMode: it restores the pre-Plan/Export
    // survey view, which may itself be closure-adjusted.
    if (useUIStore.getState().appMode !== 'survey') setAppMode('survey');
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
    cause === 'file'
      ? t('common.actions.close')
      : cause === 'view'
      ? t('dialogs.importBlocked.stayInAdjusted')
      : appMode === 'export'
        ? t('dialogs.importBlocked.stayInExport')
        : t('dialogs.importBlocked.stayInPlan');
  const switchLabel =
    cause === 'view'
      ? t('dialogs.importBlocked.switchToMeasured')
      : t('dialogs.importBlocked.switchToSurvey');
  const body =
    cause === 'file'
      ? t('dialogs.importBlocked.bodyFile')
      : cause === 'view'
        ? t('dialogs.importBlocked.bodyView')
        : t('dialogs.importBlocked.body');

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
          {cause !== 'file' && (
            <Button variant="primary" onClick={handleSwitch} data-testid="import-blocked-switch">
              {switchLabel}
            </Button>
          )}
        </div>
      </div>
    </DialogWrapper>
  );
}
