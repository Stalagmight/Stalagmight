import { useState, useEffect, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { extractFilename } from '../../utils/path';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import type { GpxFile } from '../../types/commands';
import { setSource } from '../../utils/telemetry';
import {
  completeImportToSingleView,
  finalizeDeferredSaveWarnings,
  surfaceImportFailure,
} from '../../utils/saveFeedback';
import { importButtonLabel } from '../../utils/importButtonLabel';

interface GpxSelectionDialogProps {
  gpxFile: GpxFile;
  filePath: string;
}

export default function GpxSelectionDialog({ gpxFile, filePath }: GpxSelectionDialogProps) {
  const { t } = useTranslation();
  const { calculateFileHash, checkDuplicateImport, executeGpxImport } = useProjectStore();
  const { closeDialog, openDialog, setView, triggerFitToBounds, showToast } = useUIStore();

  const [importWaypoints, setImportWaypoints] = useState(gpxFile.waypoints.length > 0);
  const [importTracks, setImportTracks] = useState(gpxFile.tracks.length > 0);
  const [importRoutes, setImportRoutes] = useState(gpxFile.routes.length > 0);
  const [depthInverted, setDepthInverted] = useState(false);
  const [isImporting, setIsImporting] = useState(false);
  const [fileHash, setFileHash] = useState<string | null>(null);
  const [isDuplicate, setIsDuplicate] = useState(false);
  const [hashChecked, setHashChecked] = useState(false);

  // Collapsible section state
  const [showWaypoints, setShowWaypoints] = useState(false);
  const [showTracks, setShowTracks] = useState(false);
  const [showRoutes, setShowRoutes] = useState(false);

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
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const fileName = useMemo(() => {
    return extractFilename(filePath);
  }, [filePath]);

  const totalTrackPoints = useMemo(() => {
    return gpxFile.tracks.reduce(
      (sum, trk) => sum + trk.segments.reduce((s, seg) => s + seg.points.length, 0),
      0
    );
  }, [gpxFile.tracks]);

  const totalRoutePoints = useMemo(() => {
    return gpxFile.routes.reduce((sum, rte) => sum + rte.points.length, 0);
  }, [gpxFile.routes]);

  const hasSelection = importWaypoints || importTracks || importRoutes;

  // Waypoints-only import can skip Step 2 (auto-placement as zero datums)
  const waypointsOnly = importWaypoints && !importTracks && !importRoutes;

  const handleContinue = () => {
    // Proceed to Step 2 (placement)
    setSource('dialog');
    openDialog('gpx-placement', {
      gpxFile,
      filePath,
      fileHash,
      importWaypoints,
      importTracks,
      importRoutes,
      depthInverted,
    });
  };

  const handleImportWaypointsOnly = async () => {
    setIsImporting(true);
    const importUi = { closeDialog, setView, triggerFitToBounds, openDialog, showToast };
    try {
      const result = await executeGpxImport(
        filePath,
        fileHash,
        importWaypoints,
        false,
        false,
        [],
        depthInverted
      );

      if (!result.success) {
        surfaceImportFailure('gpx-selection', importUi, { phase: 'command', error: result.error });
        return;
      }

      // A partial-success import (new-project deferred save failed) returns
      // success WITH a "SYSTEM" save-failure warning. Surface ONLY that instead
      // of the receipt, so "unsaved - save manually" isn't buried (#110).
      if (finalizeDeferredSaveWarnings(result.warnings, 'gpx-selection', importUi, result.import_receipt)) {
        return;
      }

      completeImportToSingleView('gpx-selection', importUi, result.import_receipt);
    } catch (e) {
      setIsImporting(false);
      surfaceImportFailure('gpx-selection', importUi, { phase: 'transport', error: e, format: 'GPX' });
    }
  };

  return (
    <DialogWrapper title={t('dialogs.gpxSelection.title')} onClose={closeDialog} size="lg" data-testid="gpx-selection-dialog">
      <div className="space-y-4">
        {/* Header info */}
        <div className="space-y-1">
          <div className="text-sm text-text-secondary">{fileName}</div>
          {gpxFile.name && (
            <div className="text-lg font-medium">{gpxFile.name}</div>
          )}
        </div>

        {/* Summary stats */}
        <div className="grid grid-cols-3 gap-3 p-3 bg-surface-alt rounded">
          <div>
            <div className="text-xs text-text-tertiary">{t('dialogs.gpxSelection.stats.waypoints')}</div>
            <div className="text-sm font-medium">{gpxFile.waypoints.length}</div>
          </div>
          <div>
            <div className="text-xs text-text-tertiary">{t('dialogs.gpxSelection.stats.tracks')}</div>
            <div className="text-sm font-medium">
              {gpxFile.tracks.length}
              {totalTrackPoints > 0 && (
                <span className="text-text-tertiary font-normal"> {t('dialogs.gpxSelection.points', { count: totalTrackPoints })}</span>
              )}
            </div>
          </div>
          <div>
            <div className="text-xs text-text-tertiary">{t('dialogs.gpxSelection.stats.routes')}</div>
            <div className="text-sm font-medium">
              {gpxFile.routes.length}
              {totalRoutePoints > 0 && (
                <span className="text-text-tertiary font-normal"> {t('dialogs.gpxSelection.points', { count: totalRoutePoints })}</span>
              )}
            </div>
          </div>
        </div>

        {/* Selection checkboxes */}
        <div className="space-y-2">
          {gpxFile.waypoints.length > 0 && (
            <label className="flex items-center gap-2 text-sm cursor-pointer">
              <input
                type="checkbox"
                checked={importWaypoints}
                onChange={(e) => setImportWaypoints(e.target.checked)}
                data-testid="gpx-import-waypoints"
              />
              {t('dialogs.gpxSelection.importWaypoints', { count: gpxFile.waypoints.length })}
            </label>
          )}
          {gpxFile.tracks.length > 0 && (
            <label className="flex items-center gap-2 text-sm cursor-pointer">
              <input
                type="checkbox"
                checked={importTracks}
                onChange={(e) => setImportTracks(e.target.checked)}
                data-testid="gpx-import-tracks"
              />
              {t('dialogs.gpxSelection.importTracks', { count: gpxFile.tracks.length })}
            </label>
          )}
          {gpxFile.routes.length > 0 && (
            <label className="flex items-center gap-2 text-sm cursor-pointer">
              <input
                type="checkbox"
                checked={importRoutes}
                onChange={(e) => setImportRoutes(e.target.checked)}
                data-testid="gpx-import-routes"
              />
              {t('dialogs.gpxSelection.importRoutes', { count: gpxFile.routes.length })}
            </label>
          )}
        </div>

        {/* Depth interpretation toggle */}
        {(importTracks || importRoutes) && (
          <label className="flex items-center gap-2 text-sm cursor-pointer p-2 bg-surface-alt rounded">
            <input
              type="checkbox"
              checked={depthInverted}
              onChange={(e) => setDepthInverted(e.target.checked)}
              data-testid="gpx-depth-inverted"
            />
            <span>
              {t('dialogs.gpxSelection.depthLabel')}
              <span className="text-text-tertiary ml-1">{t('dialogs.gpxSelection.depthHint')}</span>
            </span>
          </label>
        )}

        {/* Waypoints detail (collapsible) */}
        {gpxFile.waypoints.length > 0 && (
          <div className="border border-border rounded">
            <button
              data-testid="gpx-waypoints-toggle"
              className="w-full px-3 py-2 text-left text-sm font-medium flex items-center gap-1 hover:bg-surface-alt"
              onClick={() => setShowWaypoints(!showWaypoints)}
            >
              <span className="text-xs">{showWaypoints ? '\u25BC' : '\u25B6'}</span>
              {t('dialogs.gpxSelection.waypointsSection', { count: gpxFile.waypoints.length })}
            </button>
            {showWaypoints && (
              <div className="max-h-40 overflow-y-auto border-t border-border">
                <table className="w-full text-xs">
                  <thead className="bg-surface-alt sticky top-0">
                    <tr className="text-left text-text-tertiary">
                      <th className="px-3 py-1">{t('dialogs.gpxSelection.table.name')}</th>
                      <th className="px-3 py-1">{t('dialogs.gpxSelection.table.lat')}</th>
                      <th className="px-3 py-1">{t('dialogs.gpxSelection.table.lon')}</th>
                      <th className="px-3 py-1">{t('dialogs.gpxSelection.table.elev')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {gpxFile.waypoints.map((wpt, i) => (
                      <tr key={i} className="border-t border-border">
                        <td className="px-3 py-1">{wpt.name ?? '-'}</td>
                        <td className="px-3 py-1">{wpt.latitude.toFixed(6)}</td>
                        <td className="px-3 py-1">{wpt.longitude.toFixed(6)}</td>
                        <td className="px-3 py-1">{wpt.elevation != null ? `${wpt.elevation.toFixed(1)}m` : '-'}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        )}

        {/* Tracks detail (collapsible) */}
        {gpxFile.tracks.length > 0 && (
          <div className="border border-border rounded">
            <button
              data-testid="gpx-tracks-toggle"
              className="w-full px-3 py-2 text-left text-sm font-medium flex items-center gap-1 hover:bg-surface-alt"
              onClick={() => setShowTracks(!showTracks)}
            >
              <span className="text-xs">{showTracks ? '\u25BC' : '\u25B6'}</span>
              {t('dialogs.gpxSelection.tracksSection', { count: gpxFile.tracks.length })}
            </button>
            {showTracks && (
              <div className="max-h-40 overflow-y-auto border-t border-border">
                <table className="w-full text-xs">
                  <thead className="bg-surface-alt sticky top-0">
                    <tr className="text-left text-text-tertiary">
                      <th className="px-3 py-1">{t('dialogs.gpxSelection.table.name')}</th>
                      <th className="px-3 py-1">{t('dialogs.gpxSelection.table.segments')}</th>
                      <th className="px-3 py-1">{t('dialogs.gpxSelection.table.points')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {gpxFile.tracks.map((trk, i) => (
                      <tr key={i} className="border-t border-border">
                        <td className="px-3 py-1">{trk.name ?? t('dialogs.gpxSelection.trackFallbackName', { index: i + 1 })}</td>
                        <td className="px-3 py-1">{trk.segments.length}</td>
                        <td className="px-3 py-1">
                          {trk.segments.reduce((s, seg) => s + seg.points.length, 0)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        )}

        {/* Routes detail (collapsible) */}
        {gpxFile.routes.length > 0 && (
          <div className="border border-border rounded">
            <button
              data-testid="gpx-routes-toggle"
              className="w-full px-3 py-2 text-left text-sm font-medium flex items-center gap-1 hover:bg-surface-alt"
              onClick={() => setShowRoutes(!showRoutes)}
            >
              <span className="text-xs">{showRoutes ? '\u25BC' : '\u25B6'}</span>
              {t('dialogs.gpxSelection.routesSection', { count: gpxFile.routes.length })}
            </button>
            {showRoutes && (
              <div className="max-h-40 overflow-y-auto border-t border-border">
                <table className="w-full text-xs">
                  <thead className="bg-surface-alt sticky top-0">
                    <tr className="text-left text-text-tertiary">
                      <th className="px-3 py-1">{t('dialogs.gpxSelection.table.name')}</th>
                      <th className="px-3 py-1">{t('dialogs.gpxSelection.table.points')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {gpxFile.routes.map((rte, i) => (
                      <tr key={i} className="border-t border-border">
                        <td className="px-3 py-1">{rte.name ?? t('dialogs.gpxSelection.routeFallbackName', { index: i + 1 })}</td>
                        <td className="px-3 py-1">{rte.points.length}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        )}

        {/* Duplicate warning */}
        {isDuplicate && hashChecked && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-sm">
            {t('dialogs.gpxSelection.duplicateWarning')}
          </div>
        )}

        {/* Footer */}
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} data-testid="gpx-selection-cancel">{t('dialogs.gpxSelection.cancel')}</Button>
          {waypointsOnly ? (
            <Button
              data-primary-action=""
              data-testid="gpx-import-confirm"
              onClick={handleImportWaypointsOnly}
              disabled={isImporting || !hashChecked || !hasSelection}
            >
              {importButtonLabel(t, {
                isImporting,
                hashChecked,
                importingKey: 'dialogs.gpxSelection.importing',
                checkingKey: 'dialogs.gpxSelection.checkingDuplicates',
                defaultKey: 'dialogs.gpxSelection.import',
              })}
            </Button>
          ) : (
            <Button
              data-primary-action=""
              data-testid="gpx-continue-button"
              onClick={handleContinue}
              disabled={!hashChecked || !hasSelection}
            >
              {hashChecked ? t('dialogs.gpxSelection.continue') : t('dialogs.gpxSelection.checkingDuplicates')}
            </Button>
          )}
        </div>
      </div>
    </DialogWrapper>
  );
}
