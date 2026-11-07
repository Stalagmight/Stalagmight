import { useState, useMemo, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { isMoveCarryRefusal } from '../../store/moveCarry';
import { useUIStore } from '../../store/uiStore';
import {
  completeImportToSingleView,
  finalizeDeferredSaveWarnings,
  surfaceImportFailure,
} from '../../utils/saveFeedback';
import { Button } from '../common/Button';
import { Select } from '../common/Select';
import { DialogWrapper } from './DialogWrapper';
import { ImportProgress } from './ImportProgress';
import { ImportCancelButton } from './ImportCancelButton';
import type { GpxFile, GpxSectionPlacement } from '../../types/commands';
import { sectionColorPalette } from '../../utils/colors';
import { extractFilename } from '../../utils/path';
import { setSource } from '../../utils/telemetry';
import { useShallow } from 'zustand/react/shallow';

/** i18n keys for the section type column (rendered `capitalize`). */
const SECTION_TYPE_KEYS = {
  track: 'dialogs.gpxPlacement.types.track',
  route: 'dialogs.gpxPlacement.types.route',
} as const;

/**
 * Step 2 dialog for GPX import.
 *
 * Shows track/route sections that need placement configuration.
 * Each section can be configured with a zero datum station.
 * Waypoints are always auto-placed as zero datums and don't appear here.
 */
export default function GpxPlacementDialog() {
  const { t } = useTranslation();
  const { executeGpxImport, startLoading, setLoadingProgress, resetLoading } = useProjectStore(
    useShallow((s) => ({
      executeGpxImport: s.executeGpxImport,
      startLoading: s.startLoading,
      setLoadingProgress: s.setLoadingProgress,
      resetLoading: s.resetLoading,
    })),
  );
  const { closeDialog, openDialog, dialogData, setView, triggerFitToBounds, resolvedTheme, showToast } = useUIStore(
    useShallow((s) => ({
      closeDialog: s.closeDialog,
      openDialog: s.openDialog,
      dialogData: s.dialogData,
      setView: s.setView,
      triggerFitToBounds: s.triggerFitToBounds,
      resolvedTheme: s.resolvedTheme,
      showToast: s.showToast,
    })),
  );

  const gpxFile = dialogData.gpxFile as GpxFile;
  const filePath = dialogData.filePath as string;
  const fileHash = dialogData.fileHash as string | null;
  const importWaypoints = dialogData.importWaypoints as boolean;
  const importTracks = dialogData.importTracks as boolean;
  const importRoutes = dialogData.importRoutes as boolean;
  const depthInverted = (dialogData.depthInverted as boolean) ?? false;

  const [isImporting, setIsImporting] = useState(false);

  // Number of waypoint sections that come before track/route sections in the backend index
  const waypointSectionOffset = importWaypoints ? gpxFile.waypoints.length : 0;

  // Build list of sections that need placement config, with GPS point data for building placements
  const sections = useMemo(() => {
    const result: Array<{
      type: 'track' | 'route';
      name: string;
      pointCount: number;
      /** Backend section index (accounts for waypoint sections) */
      backendIndex: number;
      /** GPS coordinates per point for building zero datum placements */
      points: Array<{ latitude: number; longitude: number }>;
    }> = [];

    let idx = waypointSectionOffset;

    if (importTracks) {
      gpxFile.tracks.forEach((trk, ti) => {
        trk.segments.forEach((seg, si) => {
          const name = trk.name
            ? trk.segments.length > 1
              ? t('dialogs.gpxPlacement.trackSegmentName', { name: trk.name, segment: si + 1 })
              : trk.name
            : trk.segments.length > 1
              ? t('dialogs.gpxPlacement.trackSegmentFallbackName', { index: ti + 1, segment: si + 1 })
              : t('dialogs.gpxPlacement.trackFallbackName', { index: ti + 1 });
          result.push({
            type: 'track',
            name,
            pointCount: seg.points.length,
            backendIndex: idx,
            points: seg.points.map((p) => ({ latitude: p.latitude, longitude: p.longitude })),
          });
          idx++;
        });
      });
    }

    if (importRoutes) {
      gpxFile.routes.forEach((rte, ri) => {
        result.push({
          type: 'route',
          name: rte.name ?? t('dialogs.gpxPlacement.routeFallbackName', { index: ri + 1 }),
          pointCount: rte.points.length,
          backendIndex: idx,
          points: rte.points.map((p) => ({ latitude: p.latitude, longitude: p.longitude })),
        });
        idx++;
      });
    }

    return result;
  }, [gpxFile, importTracks, importRoutes, waypointSectionOffset, t]);

  // Per-section zero datum station index (0 = first station)
  const [zeroDatumIndices, setZeroDatumIndices] = useState<Record<number, number>>(() => {
    const initial: Record<number, number> = {};
    sections.forEach((_, i) => {
      initial[i] = 0;
    });
    return initial;
  });

  const handleZeroDatumChange = useCallback((sectionIdx: number, stationIdx: number) => {
    setZeroDatumIndices((prev) => ({ ...prev, [sectionIdx]: stationIdx }));
  }, []);

  const handleBack = () => {
    setSource('dialog');
    openDialog('gpx-selection', {
      gpxFile,
      filePath,
    });
  };

  const handleImport = async () => {
    setIsImporting(true);

    // Build GpxSectionPlacement objects with proper ImportPlacement zero_datum
    const palette = sectionColorPalette(resolvedTheme);
    const placements: GpxSectionPlacement[] = sections.map((sec, i) => {
      const stationIdx = zeroDatumIndices[i] ?? 0;
      const point = sec.points[stationIdx] ?? sec.points[0];
      const coords = point
        ? { latitude: point.latitude, longitude: point.longitude, depth_reference: 0 }
        : { latitude: 0, longitude: 0, depth_reference: 0 };

      return {
        section_index: sec.backendIndex,
        placement: {
          type: 'zero_datum' as const,
          coordinates: coords,
          station_index: stationIdx,
        },
        name: null,
        color: palette[i % palette.length] ?? '#4A90D9',
      };
    });

    const importUi = { closeDialog, setView, triggerFitToBounds, openDialog, showToast };
    // Show the loading overlay so the user sees Reading -> Converting ->
    // Applying -> Finalizing phase ticks from the backend ProgressEmitter,
    // as the other importers do (#576 C10).
    startLoading(gpxFile.name || extractFilename(filePath), filePath);
    setLoadingProgress({ phase: 'import_parsing' });
    try {
      const result = await executeGpxImport(
        filePath,
        fileHash,
        importWaypoints,
        importTracks,
        importRoutes,
        placements,
        depthInverted
      );
      resetLoading();

      if (!result.success) {
        // A move carry refused it (GHOST-2) and said so: keep the dialog for the retry.
        if (isMoveCarryRefusal(result)) setIsImporting(false);
        else surfaceImportFailure('gpx-placement', importUi, { phase: 'command', error: result.error, code: result.code });
        return;
      }

      // A partial-success import (new-project deferred save failed) returns
      // success WITH a "SYSTEM" save-failure warning. Surface ONLY that instead
      // of the receipt, so "unsaved - save manually" isn't buried (#110).
      if (finalizeDeferredSaveWarnings(result.warnings, 'gpx-placement', importUi, result.import_receipt)) {
        return;
      }

      completeImportToSingleView('gpx-placement', importUi, result.import_receipt);
    } catch (e) {
      resetLoading();
      setIsImporting(false);
      surfaceImportFailure('gpx-placement', importUi, { phase: 'transport', error: e, format: 'GPX' });
    }
  };

  return (
    <DialogWrapper title={t('dialogs.gpxPlacement.title')} onClose={closeDialog} size="lg" data-testid="gpx-placement-dialog">
      <div className="space-y-4">
        <div className="text-sm text-text-secondary">
          {t('dialogs.gpxPlacement.description')}
        </div>

        {/* Section list */}
        <div className="max-h-80 overflow-y-auto border border-border rounded">
          <table className="w-full text-sm">
            <thead className="bg-surface-alt sticky top-0">
              <tr className="text-left text-text-tertiary text-xs">
                <th className="px-3 py-1.5">{t('dialogs.gpxPlacement.table.section')}</th>
                <th className="px-3 py-1.5">{t('dialogs.gpxPlacement.table.type')}</th>
                <th className="px-3 py-1.5">{t('dialogs.gpxPlacement.table.points')}</th>
                <th className="px-3 py-1.5">{t('dialogs.gpxPlacement.table.zeroDatumStation')}</th>
              </tr>
            </thead>
            <tbody>
              {sections.map((sec, i) => (
                <tr key={i} className="border-t border-border">
                  <td className="px-3 py-1.5">{sec.name}</td>
                  <td className="px-3 py-1.5 capitalize text-text-secondary">{t(SECTION_TYPE_KEYS[sec.type])}</td>
                  <td className="px-3 py-1.5">{sec.pointCount}</td>
                  <td className="px-3 py-1.5">
                    <Select
                      value={String(zeroDatumIndices[i] ?? 0)}
                      data-testid={`gpx-placement-zero-datum-${i}`}
                      onChange={(v) => handleZeroDatumChange(i, parseInt(v, 10))}
                      options={Array.from({ length: sec.pointCount }, (_, si) => ({
                        value: String(si),
                        label: t(
                          si === 0
                            ? 'dialogs.gpxPlacement.stationOptionFirst'
                            : si === sec.pointCount - 1
                              ? 'dialogs.gpxPlacement.stationOptionLast'
                              : 'dialogs.gpxPlacement.stationOption',
                          { index: si },
                        ),
                      }))}
                      size="sm"
                      aria-label={t('dialogs.gpxPlacement.zeroDatumStationLabel', { name: sec.name })}
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        {sections.length === 0 && (
          <div className="text-sm text-text-secondary text-center py-4">
            {t('dialogs.gpxPlacement.empty')}
          </div>
        )}

        {/* In-dialog progress (#576 NF-W3P-1): an import into an open project runs in
            Single System View, where GlobalView's loading overlay is not mounted. */}
        {isImporting && <ImportProgress testIdPrefix="gpx-import" />}

        {/* Footer */}
        <div className="flex justify-between pt-4 border-t border-border">
          <Button onClick={handleBack} disabled={isImporting} data-testid="gpx-placement-back">
            {t('common.actions.back')}
          </Button>
          <div className="flex gap-2">
            {/* While the import runs, Cancel asks the backend to stop it (RV4-FE-MISC1). */}
            {isImporting ? (
              <ImportCancelButton data-testid="gpx-placement-cancel">{t('common.actions.cancel')}</ImportCancelButton>
            ) : (
              <Button onClick={closeDialog} data-testid="gpx-placement-cancel">
                {t('common.actions.cancel')}
              </Button>
            )}
            <Button
              data-primary-action=""
              data-testid="gpx-import-confirm"
              onClick={handleImport}
              disabled={isImporting}
            >
              {isImporting ? t('dialogs.gpxPlacement.importing') : t('common.actions.import')}
            </Button>
          </div>
        </div>
      </div>
    </DialogWrapper>
  );
}
