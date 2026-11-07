import { useState, useEffect, useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { isMoveCarryRefusal } from '../../store/moveCarry';
import { useUIStore } from '../../store/uiStore';
import {
  completeImportToSingleView,
  finalizeDeferredSaveWarnings,
  finishImportWithoutOrphanSuggestions,
  suggestOrphanPlacementsAfterImport,
  surfaceImportFailure,
} from '../../utils/saveFeedback';
import { extractFilename } from '../../utils/path';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { ImportProgress } from './ImportProgress';
import { ImportCancelButton } from './ImportCancelButton';
import type { CompassFile, CompassSurveySummary, InvalidShotCounts } from '../../types/commands';
import { setSource } from '../../utils/telemetry';
import { importButtonLabel } from '../../utils/importButtonLabel';
import { useShallow } from 'zustand/react/shallow';

interface CompassImportDialogProps {
  compassFile: CompassFile;
  filePath: string;
}

/**
 * How many of `invalid_shot_counts`'s four counters apply under the given
 * LIVE toggles - the ONE counter matching both `includeExcluded` (which set
 * of shots reaches the length check) and `applyCorrections` (how each
 * shot's length resolves before that check).
 */
function invalidShotCountFor(
  counts: InvalidShotCounts,
  includeExcluded: boolean,
  applyCorrections: boolean
): number {
  const nonExcluded = applyCorrections ? counts.non_excluded_corrected : counts.non_excluded_uncorrected;
  const excluded = applyCorrections ? counts.excluded_corrected : counts.excluded_uncorrected;
  return nonExcluded + (includeExcluded ? excluded : 0);
}

/**
 * What the backend's converter actually lands for a shot_count/
 * excluded_shot_count/invalid_shot_counts triple, under the live toggles -
 * exact in all four `(includeExcluded, applyCorrections)` combinations,
 * mirroring pass 1 of `convert_compass_to_import_configs` (#437 follow-up,
 * review of #550). Shared by the header total and each per-survey row so
 * the rows always sum to the header.
 */
function landingShotCountFor(
  shotCount: number,
  excludedShotCount: number,
  invalidCounts: InvalidShotCounts,
  includeExcluded: boolean,
  applyCorrections: boolean
): number {
  return (
    shotCount -
    (includeExcluded ? 0 : excludedShotCount) -
    invalidShotCountFor(invalidCounts, includeExcluded, applyCorrections)
  );
}

function surveyLandingShotCount(
  survey: CompassSurveySummary,
  includeExcluded: boolean,
  applyCorrections: boolean
): number {
  return landingShotCountFor(
    survey.shot_count,
    survey.excluded_shot_count,
    survey.invalid_shot_counts,
    includeExcluded,
    applyCorrections
  );
}

export default function CompassImportDialog({ compassFile, filePath }: CompassImportDialogProps) {
  const { t } = useTranslation();
  const {
    executeCompassImport,
    calculateFileHash,
    checkDuplicateImport,
    startLoading,
    setLoadingProgress,
    resetLoading,
  } = useProjectStore(
    useShallow((s) => ({
      executeCompassImport: s.executeCompassImport,
      calculateFileHash: s.calculateFileHash,
      checkDuplicateImport: s.checkDuplicateImport,
      startLoading: s.startLoading,
      setLoadingProgress: s.setLoadingProgress,
      resetLoading: s.resetLoading,
    })),
  );
  const { closeDialog, openDialog, setView, triggerFitToBounds, showToast } = useUIStore(
    useShallow((s) => ({
      closeDialog: s.closeDialog,
      openDialog: s.openDialog,
      setView: s.setView,
      triggerFitToBounds: s.triggerFitToBounds,
      showToast: s.showToast,
    })),
  );

  const [isImporting, setIsImporting] = useState(false);
  const [fileHash, setFileHash] = useState<string | null>(null);
  const [isDuplicate, setIsDuplicate] = useState(false);
  const [hashChecked, setHashChecked] = useState(false);
  const [includeExcluded, setIncludeExcluded] = useState(false);
  const [applyCorrections, setApplyCorrections] = useState(true);
  const [commentsOnStations, setCommentsOnStations] = useState(true);

  // Check for duplicate import on mount
  useEffect(() => {
    (async () => {
      try {
        const hash = await calculateFileHash(filePath);
        setFileHash(hash);
        const dup = await checkDuplicateImport(hash);
        setIsDuplicate(dup);
      } catch {
        // Non-fatal: proceed without duplicate detection
      }
      setHashChecked(true);
    })();
  // eslint-disable-next-line react-hooks/exhaustive-deps -- mount-only: hash the file this dialog opened with, once (the store actions are stable)
  }, []);

  const fileName = useMemo(() => extractFilename(filePath), [filePath]);

  // What the backend's converter actually lands under the two LIVE toggles -
  // see `landingShotCountFor` / `invalidShotCountFor` above for the exact
  // arithmetic (#437, review of #550).
  const invalidShotCount = useMemo(
    () => invalidShotCountFor(compassFile.invalid_shot_counts, includeExcluded, applyCorrections),
    [compassFile.invalid_shot_counts, includeExcluded, applyCorrections]
  );

  const landingShotCount = landingShotCountFor(
    compassFile.shot_count,
    compassFile.excluded_shot_count,
    compassFile.invalid_shot_counts,
    includeExcluded,
    applyCorrections
  );

  // "(N excluded, M invalid)" - only the parts that are non-zero and, for
  // excluded, only while the toggle would actually drop them.
  const shotCountFootnote = useMemo(() => {
    const parts: string[] = [];
    if (!includeExcluded && compassFile.excluded_shot_count > 0) {
      parts.push(
        t('dialogs.compassImport.stats.excludedSuffix', { count: compassFile.excluded_shot_count })
      );
    }
    if (invalidShotCount > 0) {
      parts.push(t('dialogs.compassImport.stats.invalidSuffix', { count: invalidShotCount }));
    }
    return parts.join(', ');
  }, [includeExcluded, compassFile.excluded_shot_count, invalidShotCount, t]);

  const handleImport = useCallback(async () => {
    setIsImporting(true);
    // Show the loading overlay so the user sees Reading -> Converting ->
    // Applying -> Finalizing phase ticks from the backend ProgressEmitter.
    startLoading(compassFile.cave_name || fileName, filePath);
    setLoadingProgress({ phase: 'import_parsing' });
    const importUi = { closeDialog, setView, triggerFitToBounds, openDialog, showToast };
    try {
      const result = await executeCompassImport(filePath, fileHash, includeExcluded, applyCorrections, commentsOnStations);
      resetLoading();

      if (!result.success) {
        // A move carry refused it (GHOST-2) and said so: keep the dialog for the retry.
        // Nothing is running then, so the footer goes back to its idle Cancel.
        if (isMoveCarryRefusal(result)) setIsImporting(false);
        else surfaceImportFailure('compass-import', importUi, { phase: 'command', error: result.error, code: result.code });
        return;
      }

      // A partial-success import (new-project deferred save failed) returns
      // success WITH a "SYSTEM" save-failure warning. Surface ONLY that, before
      // the optional orphan step, so "unsaved - save manually" isn't buried.
      // Routine per-shot validation warnings (LONG_SHOT, …) also ride along on a
      // successful import but are surfaced via the data-table UI - they must not
      // modal here or skip orphan placement (#110).
      if (finalizeDeferredSaveWarnings(result.warnings, 'compass-import', importUi, result.import_receipt)) {
        return;
      }

      // `section_ids` used to be `skip_serializing_if = "Vec::is_empty"`, so an
      // import that created NO section omitted the key entirely while the
      // generated type said `Array<number>`. A bare `.length` here threw, the
      // catch below reported it as a transport failure, and the receipt
      // explaining the empty import was thrown away (#411). #441 removed the
      // skip, so the key is always on the wire: this `?? []` is defensive only
      // and unreachable in the shipped pairing (one binary, one frontend).
      // Same shape as WallsImportDialog.
      const sectionIds = result.section_ids ?? [];

      // Check for orphan placement opportunities. Only the lookup may fail
      // softly here (it skips the suggestion step, and says so - #576
      // NF-DMPS-1); `finishImportWithoutOrphanSuggestions` runs outside it and
      // completes the import exactly once, so a throw partway through IT can
      // never fall through to a duplicate completion.
      if (sectionIds.length > 0) {
        const suggestions = await suggestOrphanPlacementsAfterImport(
          sectionIds,
          'suggest orphan placements after Compass import',
          importUi
        );
        if (suggestions.length > 0) {
          setSource('dialog');
          openDialog('orphan-placement', { suggestions, receipt: result.import_receipt ?? undefined });
          return;
        }
        await finishImportWithoutOrphanSuggestions('compass-import', importUi, sectionIds, result.import_receipt);
        return;
      }

      completeImportToSingleView('compass-import', importUi, result.import_receipt);
    } catch (e) {
      resetLoading();
      setIsImporting(false);
      surfaceImportFailure('compass-import', importUi, { phase: 'transport', error: e, format: 'Compass' });
    }
  }, [executeCompassImport, filePath, fileHash, includeExcluded, applyCorrections, commentsOnStations, compassFile.cave_name, fileName, closeDialog, openDialog, setView, triggerFitToBounds, startLoading, setLoadingProgress, resetLoading]);

  // Escape / the close button do nothing while the import runs: closing would
  // orphan its result. The footer Cancel stops it instead (RV4-FE-MISC1).
  return (
    <DialogWrapper
      title={t('dialogs.compassImport.title')}
      onClose={isImporting ? () => {} : closeDialog}
      size="lg"
      data-testid="compass-import-dialog"
    >
      <div className="space-y-4">
        {/* Header info */}
        <div className="space-y-1">
          <div className="text-sm text-text-secondary">{fileName}</div>
          {compassFile.cave_name && (
            <div className="text-lg font-medium">{compassFile.cave_name}</div>
          )}
          <div className="text-xs text-text-tertiary">
            {compassFile.length_unit === 'meters'
              ? t('dialogs.compassImport.lengthUnit.meters')
              : compassFile.length_unit === 'mixed'
                ? t('dialogs.compassImport.lengthUnit.mixed')
                : t('dialogs.compassImport.lengthUnit.feet')}
          </div>
        </div>

        {/* Summary stats */}
        <div className="grid grid-cols-4 gap-3 p-3 bg-surface-alt rounded">
          <div>
            <div className="text-xs text-text-tertiary">{t('dialogs.compassImport.stats.surveys')}</div>
            <div className="text-sm font-medium">{compassFile.survey_count}</div>
          </div>
          <div>
            <div className="text-xs text-text-tertiary" title={t('dialogs.compassImport.stats.shotsTooltip')}>
              {t('dialogs.compassImport.stats.shots')}
            </div>
            <div className="text-sm font-medium" data-testid="compass-shot-count">
              {landingShotCount}
              {shotCountFootnote && (
                <span className="text-text-tertiary font-normal" data-testid="compass-shot-count-footnote">
                  {' '}
                  ({shotCountFootnote})
                </span>
              )}
            </div>
          </div>
          <div>
            <div className="text-xs text-text-tertiary">{t('dialogs.compassImport.stats.datum')}</div>
            <div className="text-sm font-medium">{compassFile.datum ?? '-'}</div>
          </div>
          <div>
            <div className="text-xs text-text-tertiary">{t('dialogs.compassImport.stats.georeference')}</div>
            <div className="text-sm font-medium">
              {compassFile.has_georeference
                ? `UTM ${compassFile.utm_zone ?? ''}`.trim()
                : t('dialogs.compassImport.stats.noGeoreference')}
            </div>
          </div>
        </div>

        {/* Survey list */}
        <div className="max-h-64 overflow-y-auto border border-border rounded">
          <table className="w-full text-sm">
            <thead className="bg-surface-alt sticky top-0">
              <tr className="text-left text-text-tertiary text-xs">
                <th className="px-3 py-1.5">{t('dialogs.compassImport.table.survey')}</th>
                <th className="px-3 py-1.5">{t('dialogs.compassImport.table.shots')}</th>
                <th className="px-3 py-1.5">{t('dialogs.compassImport.table.team')}</th>
                <th className="px-3 py-1.5">{t('dialogs.compassImport.table.date')}</th>
                <th className="px-3 py-1.5">{t('dialogs.compassImport.table.declination')}</th>
              </tr>
            </thead>
            <tbody>
              {compassFile.surveys.map((survey, i) => (
                <tr key={i} className="border-t border-border">
                  <td className="px-3 py-1.5">{survey.name}</td>
                  <td className="px-3 py-1.5" data-testid={`compass-survey-shots-${i}`}>
                    {surveyLandingShotCount(survey, includeExcluded, applyCorrections)}
                  </td>
                  <td className="px-3 py-1.5 text-text-secondary">{survey.team ?? '-'}</td>
                  <td className="px-3 py-1.5 text-text-secondary">{survey.date}</td>
                  <td className="px-3 py-1.5 text-text-secondary">{survey.declination}°</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        {/* Declination mode info */}
        <div className="text-xs text-text-secondary">
          {t('dialogs.compassImport.declinationMode', { mode: compassFile.declination_mode })}
          {compassFile.has_backsights && ` ${t('dialogs.compassImport.backsightsPresent')}`}
        </div>

        {/* Parser warnings */}
        {compassFile.warnings.length > 0 && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-sm space-y-1">
            {compassFile.warnings.map((w, i) => (
              <div key={i}>{w}</div>
            ))}
          </div>
        )}

        {/* Duplicate warning */}
        {isDuplicate && hashChecked && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-sm">
            {t('dialogs.compassImport.duplicateWarning')}
          </div>
        )}

        {/* Options */}
        <label className="flex items-center gap-2 text-sm text-text-secondary cursor-pointer">
          <input
            type="checkbox"
            checked={applyCorrections}
            onChange={(e) => setApplyCorrections(e.target.checked)}
            data-testid="compass-apply-corrections"
          />
          {t('dialogs.compassImport.options.applyCorrections')}
        </label>
        <label className="flex items-center gap-2 text-sm text-text-secondary cursor-pointer">
          <input
            type="checkbox"
            checked={commentsOnStations}
            onChange={(e) => setCommentsOnStations(e.target.checked)}
            data-testid="compass-comments-on-stations"
          />
          {t('dialogs.compassImport.options.commentsOnStations')}
        </label>
        <label className="flex items-center gap-2 text-sm text-text-secondary cursor-pointer">
          <input
            type="checkbox"
            checked={includeExcluded}
            onChange={(e) => setIncludeExcluded(e.target.checked)}
            data-testid="compass-include-excluded"
          />
          {t('dialogs.compassImport.options.includeExcluded')}
        </label>

        {/* In-dialog progress (#576 NF-W3P-1): an import into an open project runs in
            Single System View, where GlobalView's loading overlay is not mounted. */}
        {isImporting && <ImportProgress testIdPrefix="compass-import" />}

        {/* Footer - sticky at the bottom of the scrolling body (mirrors
            TmlImportDialog's idiom; see its comment for the assumptions this
            relies on, incl. DialogWrapper's default `p-4` body padding). */}
        <div className="sticky -bottom-4 -mx-4 -mb-4 flex justify-end gap-2 px-4 py-4 border-t border-border bg-panel">
          {isImporting ? (
            <ImportCancelButton data-testid="compass-import-cancel">{t('dialogs.compassImport.cancel')}</ImportCancelButton>
          ) : (
            <Button onClick={closeDialog} data-testid="compass-import-cancel">{t('dialogs.compassImport.cancel')}</Button>
          )}
          <Button
            data-primary-action=""
            data-testid="compass-import-confirm"
            onClick={handleImport}
            disabled={isImporting || !hashChecked}
          >
            {importButtonLabel(t, {
              isImporting,
              hashChecked,
              importingKey: 'dialogs.compassImport.importing',
              checkingKey: 'dialogs.compassImport.checkingDuplicates',
              defaultKey: 'dialogs.compassImport.import',
            })}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
