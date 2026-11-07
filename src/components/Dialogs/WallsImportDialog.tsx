import { useState, useEffect, useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { isMoveCarryRefusal } from '../../store/moveCarry';
import { useUIStore } from '../../store/uiStore';
import i18n from '../../i18n';
import { extractFilename } from '../../utils/path';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { ImportProgress } from './ImportProgress';
import { ImportCancelButton } from './ImportCancelButton';
import type { WallsFile } from '../../types/commands';
import { setSource } from '../../utils/telemetry';
import { importButtonLabel } from '../../utils/importButtonLabel';
import {
  finalizeDeferredSaveWarnings,
  finishImportWithoutOrphanSuggestions,
  suggestOrphanPlacementsAfterImport,
  surfaceImportFailure,
} from '../../utils/saveFeedback';
import { useShallow } from 'zustand/react/shallow';

interface WallsImportDialogProps {
  wallsFile: WallsFile;
  filePath: string;
}

export default function WallsImportDialog({ wallsFile, filePath }: WallsImportDialogProps) {
  const { t } = useTranslation();
  const {
    executeWallsImport,
    calculateFileHash,
    checkDuplicateImport,
    startLoading,
    setLoadingProgress,
    resetLoading,
  } = useProjectStore(
    useShallow((s) => ({
      executeWallsImport: s.executeWallsImport,
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
  const [commentsOnStations, setCommentsOnStations] = useState(true);
  const [includeSplays, setIncludeSplays] = useState(false);

  // Indices (into `wallsFile.surveys`, 1:1 with the backend's survey order) the
  // user has chosen to import. Only resolved surveys are selectable (an
  // unresolved one has no .SRV on disk). Default: everything resolved, so the
  // import matches prior behavior until the user deselects (e.g. a huge surface
  // terrain mesh). Reset whenever the previewed file changes.
  const resolvedIndices = useMemo(
    () => wallsFile.surveys.flatMap((s, i) => (s.resolved ? [i] : [])),
    [wallsFile.surveys]
  );
  const [selectedSurveys, setSelectedSurveys] = useState<Set<number>>(
    () => new Set(resolvedIndices)
  );
  useEffect(() => {
    setSelectedSurveys(new Set(resolvedIndices));
  }, [resolvedIndices]);

  const allSelected =
    resolvedIndices.length > 0 && resolvedIndices.every((i) => selectedSurveys.has(i));
  const toggleSurvey = useCallback((i: number) => {
    setSelectedSurveys((prev) => {
      const next = new Set(prev);
      if (next.has(i)) next.delete(i);
      else next.add(i);
      return next;
    });
  }, []);
  const toggleAll = useCallback(() => {
    setSelectedSurveys((prev) => {
      const allOn = resolvedIndices.length > 0 && resolvedIndices.every((i) => prev.has(i));
      return allOn ? new Set() : new Set(resolvedIndices);
    });
  }, [resolvedIndices]);

  // Importable shots in the CURRENTLY SELECTED surveys (each summary's
  // shot_count already excludes splays, same filter as the project total), so
  // the Shots stat and the Import gate track the selection rather than the
  // whole file - deselecting every shot-bearing survey disables Import.
  const selectedShotCount = useMemo(
    () =>
      wallsFile.surveys.reduce(
        (sum, s, i) => (selectedSurveys.has(i) ? sum + s.shot_count : sum),
        0
      ),
    [wallsFile.surveys, selectedSurveys]
  );

  // Splays / fixed stations in the SELECTED surveys, computed the same way as
  // `selectedShotCount`. The Import gate's splay branch and the Fixed-stations
  // stat/warning key off these so they track the selection rather than the whole
  // file - deselecting a splay-only or fix-heavy survey no longer leaves the gate
  // enabled or the anchor stat/warning stale.
  const selectedSplayCount = useMemo(
    () =>
      wallsFile.surveys.reduce(
        (sum, s, i) => (selectedSurveys.has(i) ? sum + s.splay_count : sum),
        0
      ),
    [wallsFile.surveys, selectedSurveys]
  );
  const selectedFixedStationCount = useMemo(
    () =>
      wallsFile.surveys.reduce(
        (sum, s, i) => (selectedSurveys.has(i) ? sum + s.fixed_station_count : sum),
        0
      ),
    [wallsFile.surveys, selectedSurveys]
  );

  // Re-check for a duplicate whenever the target file changes (the dialog may be
  // reused for a different import); reset the gate so Import stays disabled until
  // the new hash is verified, and ignore a stale in-flight result.
  useEffect(() => {
    let cancelled = false;
    setHashChecked(false);
    (async () => {
      try {
        const hash = await calculateFileHash(filePath);
        if (cancelled) return;
        setFileHash(hash);
        const dup = await checkDuplicateImport(hash);
        if (cancelled) return;
        setIsDuplicate(dup);
      } catch {
        // Non-fatal: proceed without duplicate detection
      }
      if (!cancelled) setHashChecked(true);
    })();
    return () => {
      cancelled = true;
    };
  }, [filePath, calculateFileHash, checkDuplicateImport]);

  const fileName = useMemo(() => extractFilename(filePath), [filePath]);

  // Walls stores a negative .REF zone for the southern hemisphere; render it as
  // "UTM 16 S" rather than the raw "UTM -16".
  const projectionLabel = useMemo(() => {
    if (!wallsFile.has_georeference) return '-';
    const z = wallsFile.utm_zone;
    const zoneLabel = z != null ? `${Math.abs(z)} ${z < 0 ? 'S' : 'N'}` : '';
    return `UTM ${zoneLabel}${wallsFile.datum ? ` · ${wallsFile.datum}` : ''}`.trim();
  }, [wallsFile.has_georeference, wallsFile.utm_zone, wallsFile.datum]);

  const handleImport = useCallback(async () => {
    setIsImporting(true);
    startLoading(wallsFile.project_name || fileName, filePath);
    setLoadingProgress({ phase: 'import_parsing' });
    const importUi = { closeDialog, setView, triggerFitToBounds, openDialog, showToast };
    try {
      // Pass the selection as the original survey indices; omit (null) when
      // everything resolved is selected so a full import stays unfiltered.
      const surveySelection = allSelected
        ? null
        : [...selectedSurveys].sort((a, b) => a - b);
      const result = await executeWallsImport(
        filePath,
        fileHash,
        commentsOnStations,
        includeSplays,
        surveySelection
      );
      resetLoading();

      if (!result.success) {
        // A move carry refused it (GHOST-2) and said so: keep the dialog for the retry.
        // Nothing is running then, so the footer goes back to its idle Cancel.
        if (isMoveCarryRefusal(result)) setIsImporting(false);
        else surfaceImportFailure('walls-import', importUi, { phase: 'command', error: result.error, code: result.code });
        return;
      }

      // A partial-success import (new-project deferred save failed) returns
      // success WITH a "SYSTEM" save-failure warning. Surface ONLY that - before
      // the optional orphan step - so "unsaved - save manually" isn't swallowed.
      if (finalizeDeferredSaveWarnings(result.warnings, 'walls-import', importUi, result.import_receipt)) {
        return;
      }

      // `section_ids` was omitted from the IPC payload when empty until #441
      // made the backend always send it; this default is now defensive only.
      const sectionIds = result.section_ids ?? [];

      // Nothing was imported - most commonly a `.wpj` whose `.srv` files aren't
      // next to it on disk. Tell the user why instead of silently "succeeding".
      //
      // DELIBERATE DIVERGENCE from the TML/Compass/GPX dialogs, which show the
      // backend's empty receipt (#411) for the same outcome: Walls has a
      // specific, actionable cause to name (missing sibling files), so it
      // routes to the error dialog and DISCARDS `result.import_receipt` -
      // including any converter warnings it carries. Change this knowingly.
      if (sectionIds.length === 0) {
        surfaceImportFailure('walls-import', importUi, {
          phase: 'empty',
          message:
            wallsFile.missing_file_count > 0
              ? i18n.t('dialogs.importFailure.wallsMissingFiles', {
                  count: wallsFile.missing_file_count,
                })
              : i18n.t('dialogs.importFailure.wallsNoShots'),
        });
        return;
      }

      // Check for orphan placement opportunities. Only the lookup may fail
      // softly here (it skips the suggestion step, and says so - #576
      // NF-DMPS-1); `finishImportWithoutOrphanSuggestions` runs outside it and
      // completes the import exactly once, so a throw partway through IT can
      // never fall through to a duplicate completion.
      const suggestions = await suggestOrphanPlacementsAfterImport(
        sectionIds,
        'suggest orphan placements after Walls import',
        importUi
      );
      if (suggestions.length > 0) {
        setSource('dialog');
        openDialog('orphan-placement', { suggestions, receipt: result.import_receipt ?? undefined });
        return;
      }
      await finishImportWithoutOrphanSuggestions('walls-import', importUi, sectionIds, result.import_receipt);
    } catch (e) {
      resetLoading();
      setIsImporting(false);
      surfaceImportFailure('walls-import', importUi, { phase: 'transport', error: e, format: 'Walls' });
    }
  }, [executeWallsImport, filePath, fileHash, commentsOnStations, includeSplays, selectedSurveys, allSelected, wallsFile.project_name, fileName, closeDialog, openDialog, setView, triggerFitToBounds, startLoading, setLoadingProgress, resetLoading]);

  // Escape / the close button do nothing while the import runs: closing would
  // orphan its result. The footer Cancel stops it instead (RV4-FE-MISC1).
  return (
    <DialogWrapper
      title={t('dialogs.wallsImport.title')}
      onClose={isImporting ? () => {} : closeDialog}
      size="lg"
      data-testid="walls-import-dialog"
    >
      <div className="space-y-4">
        {/* Header info */}
        <div className="space-y-1">
          <div className="text-sm text-text-secondary">{fileName}</div>
          {wallsFile.project_name && (
            <div className="text-lg font-medium">{wallsFile.project_name}</div>
          )}
        </div>

        {/* Summary stats */}
        <div className="grid grid-cols-4 gap-3 p-3 bg-surface-alt rounded">
          <div>
            <div className="text-xs text-text-tertiary">{t('dialogs.wallsImport.stats.surveys')}</div>
            <div className="text-sm font-medium">
              {wallsFile.resolved_survey_count}
              {wallsFile.survey_count !== wallsFile.resolved_survey_count
                ? ` / ${wallsFile.survey_count}`
                : ''}
            </div>
          </div>
          <div>
            <div className="text-xs text-text-tertiary">{t('dialogs.wallsImport.stats.shots')}</div>
            <div className="text-sm font-medium">
              {selectedShotCount}
              {!allSelected ? ` / ${wallsFile.shot_count}` : ''}
            </div>
          </div>
          <div>
            <div className="text-xs text-text-tertiary" title={t('dialogs.wallsImport.stats.fixedStationsTooltip')}>
              {t('dialogs.wallsImport.stats.fixedStations')}
            </div>
            <div className="text-sm font-medium">
              {selectedFixedStationCount}
              {!allSelected ? ` / ${wallsFile.fixed_station_count}` : ''}
            </div>
          </div>
          <div>
            <div className="text-xs text-text-tertiary" title={t('dialogs.wallsImport.stats.projectionTooltip')}>
              {t('dialogs.wallsImport.stats.projection')}
            </div>
            <div className="text-sm font-medium">{projectionLabel}</div>
          </div>
        </div>
        {wallsFile.has_georeference && selectedFixedStationCount === 0 && (
          <div className="text-xs text-text-secondary">
            {t('dialogs.wallsImport.noFixWarning.before')}{' '}
            <code>#Fix</code>{' '}
            {t('dialogs.wallsImport.noFixWarning.after')}
          </div>
        )}

        {/* Nothing-to-import warning: a .wpj resolves its .srv files by name from
            the same folder; if none are present there is nothing to import. */}
        {wallsFile.shot_count === 0 && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-sm">
            {wallsFile.missing_file_count > 0
              ? t('dialogs.wallsImport.nothingToImport.missing', {
                  count: wallsFile.missing_file_count,
                  fileName,
                })
              : wallsFile.splay_count > 0
                ? t('dialogs.wallsImport.nothingToImport.splaysOnly', {
                    count: wallsFile.splay_count,
                  })
                : t('dialogs.wallsImport.nothingToImport.none')}
          </div>
        )}

        {/* Missing-file / attachment notice (when there is still something to import) */}
        {wallsFile.shot_count > 0 &&
          (wallsFile.missing_file_count > 0 || wallsFile.skipped_attachment_count > 0) && (
            <div className="text-xs text-text-secondary">
              {wallsFile.missing_file_count > 0 && (
                <span>{t('dialogs.wallsImport.missingFiles', { count: wallsFile.missing_file_count })} </span>
              )}
              {wallsFile.skipped_attachment_count > 0 && (
                <span>{t('dialogs.wallsImport.skippedAttachments', { count: wallsFile.skipped_attachment_count })}</span>
              )}
            </div>
          )}

        {/* Survey list - choose which surveys to import */}
        <div className="space-y-1">
          <div className="flex items-center justify-between text-xs text-text-tertiary">
            <span>{t('dialogs.wallsImport.surveysToImport')}</span>
            <span data-testid="walls-selected-count">
              {t('dialogs.wallsImport.selectedCount', {
                selected: selectedSurveys.size,
                total: resolvedIndices.length,
              })}
            </span>
          </div>
          <div className="max-h-64 overflow-y-auto border border-border rounded">
            <table className="w-full text-sm">
              <thead className="bg-surface-alt sticky top-0">
                <tr className="text-left text-text-tertiary text-xs">
                  <th className="px-3 py-1.5 w-8">
                    <input
                      type="checkbox"
                      checked={allSelected}
                      onChange={toggleAll}
                      aria-label={t('dialogs.wallsImport.selectAllLabel')}
                      data-testid="walls-select-all"
                    />
                  </th>
                  <th className="px-3 py-1.5">{t('dialogs.wallsImport.table.survey')}</th>
                  <th className="px-3 py-1.5">{t('dialogs.wallsImport.table.shots')}</th>
                  <th className="px-3 py-1.5">{t('dialogs.wallsImport.table.segment')}</th>
                  <th className="px-3 py-1.5">{t('dialogs.wallsImport.table.date')}</th>
                </tr>
              </thead>
              <tbody>
                {wallsFile.surveys.map((survey, i) =>
                  survey.resolved ? (
                    <tr key={i} className="border-t border-border">
                      <td className="px-3 py-1.5">
                        <input
                          type="checkbox"
                          checked={selectedSurveys.has(i)}
                          onChange={() => toggleSurvey(i)}
                          aria-label={t('dialogs.wallsImport.importSurveyLabel', {
                            name: survey.title || survey.name,
                          })}
                          data-testid={`walls-survey-${i}`}
                        />
                      </td>
                      <td className="px-3 py-1.5">{survey.title || survey.name}</td>
                      <td className="px-3 py-1.5">{survey.shot_count}</td>
                      <td className="px-3 py-1.5 text-text-secondary">{survey.segment || '-'}</td>
                      <td className="px-3 py-1.5 text-text-secondary">{survey.date || '-'}</td>
                    </tr>
                  ) : null
                )}
              </tbody>
            </table>
          </div>
        </div>

        {/* Parser warnings */}
        {wallsFile.warnings.length > 0 && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-sm space-y-1 max-h-32 overflow-y-auto">
            {wallsFile.warnings.map((w, i) => (
              <div key={i}>{w}</div>
            ))}
          </div>
        )}

        {/* Duplicate warning */}
        {isDuplicate && hashChecked && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-sm">
            {t('dialogs.wallsImport.duplicateWarning')}
          </div>
        )}

        {/* Options */}
        <label className="flex items-center gap-2 text-sm text-text-secondary cursor-pointer">
          <input
            type="checkbox"
            checked={commentsOnStations}
            onChange={(e) => setCommentsOnStations(e.target.checked)}
            data-testid="walls-comments-on-stations"
          />
          {t('dialogs.wallsImport.options.commentsOnStations')}
        </label>
        <label className="flex items-center gap-2 text-sm text-text-secondary cursor-pointer">
          <input
            type="checkbox"
            checked={includeSplays}
            onChange={(e) => setIncludeSplays(e.target.checked)}
            data-testid="walls-include-splays"
          />
          {t('dialogs.wallsImport.options.includeSplays')}
        </label>

        {/* In-dialog progress (#576 NF-W3P-1): an import into an open project runs in
            Single System View, where GlobalView's loading overlay is not mounted. */}
        {isImporting && <ImportProgress testIdPrefix="walls-import" />}

        {/* Footer - sticky at the bottom of the scrolling body (mirrors
            TmlImportDialog's idiom; see its comment for the assumptions this
            relies on, incl. DialogWrapper's default `p-4` body padding). */}
        <div className="sticky -bottom-4 -mx-4 -mb-4 flex justify-end gap-2 px-4 py-4 border-t border-border bg-panel">
          {isImporting ? (
            <ImportCancelButton data-testid="walls-import-cancel">{t('dialogs.wallsImport.cancel')}</ImportCancelButton>
          ) : (
            <Button onClick={closeDialog} data-testid="walls-import-cancel">{t('dialogs.wallsImport.cancel')}</Button>
          )}
          <Button
            data-primary-action=""
            data-testid="walls-import-confirm"
            onClick={handleImport}
            disabled={
              isImporting ||
              !hashChecked ||
              selectedSurveys.size === 0 ||
              (selectedShotCount === 0 && !(includeSplays && selectedSplayCount > 0))
            }
          >
            {importButtonLabel(t, {
              isImporting,
              hashChecked,
              importingKey: 'dialogs.wallsImport.importing',
              checkingKey: 'dialogs.wallsImport.checkingDuplicates',
              defaultKey: 'dialogs.wallsImport.import',
            })}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
