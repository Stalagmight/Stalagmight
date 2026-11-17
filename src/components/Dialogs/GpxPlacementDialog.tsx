import { useState, useMemo, useCallback } from 'react';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import {
  completeImportToSingleView,
  finalizeDeferredSaveWarnings,
  surfaceImportFailure,
} from '../../utils/saveFeedback';
import { Button } from '../common/Button';
import { Select } from '../common/Select';
import { DialogWrapper } from './DialogWrapper';
import type { GpxFile, GpxLinePlacement } from '../../types/commands';
import { lineColorPalette } from '../../utils/colors';
import { setSource } from '../../utils/telemetry';

/**
 * Step 2 dialog for GPX import.
 *
 * Shows track/route lines that need placement configuration.
 * Each line can be configured with a zero datum station.
 * Waypoints are always auto-placed as zero datums and don't appear here.
 */
export default function GpxPlacementDialog() {
  const { executeGpxImport } = useProjectStore();
  const { closeDialog, openDialog, dialogData, setView, triggerFitToBounds, resolvedTheme, showToast } =
    useUIStore();

  const gpxFile = dialogData.gpxFile as GpxFile;
  const filePath = dialogData.filePath as string;
  const fileHash = dialogData.fileHash as string | null;
  const importWaypoints = dialogData.importWaypoints as boolean;
  const importTracks = dialogData.importTracks as boolean;
  const importRoutes = dialogData.importRoutes as boolean;
  const depthInverted = (dialogData.depthInverted as boolean) ?? false;

  const [isImporting, setIsImporting] = useState(false);

  // Number of waypoint lines that come before track/route lines in the backend index
  const waypointLineOffset = importWaypoints ? gpxFile.waypoints.length : 0;

  // Build list of lines that need placement config, with GPS point data for building placements
  const lines = useMemo(() => {
    const result: Array<{
      type: 'track' | 'route';
      name: string;
      pointCount: number;
      /** Backend line index (accounts for waypoint lines) */
      backendIndex: number;
      /** GPS coordinates per point for building zero datum placements */
      points: Array<{ latitude: number; longitude: number }>;
    }> = [];

    let idx = waypointLineOffset;

    if (importTracks) {
      gpxFile.tracks.forEach((trk, ti) => {
        trk.segments.forEach((seg, si) => {
          const name = trk.name
            ? trk.segments.length > 1
              ? `${trk.name} - Seg ${si + 1}`
              : trk.name
            : `Track ${ti + 1}${trk.segments.length > 1 ? ` Seg ${si + 1}` : ''}`;
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
          name: rte.name ?? `Route ${ri + 1}`,
          pointCount: rte.points.length,
          backendIndex: idx,
          points: rte.points.map((p) => ({ latitude: p.latitude, longitude: p.longitude })),
        });
        idx++;
      });
    }

    return result;
  }, [gpxFile, importTracks, importRoutes, waypointLineOffset]);

  // Per-line zero datum station index (0 = first station)
  const [zeroDatumIndices, setZeroDatumIndices] = useState<Record<number, number>>(() => {
    const initial: Record<number, number> = {};
    lines.forEach((_, i) => {
      initial[i] = 0;
    });
    return initial;
  });

  const handleZeroDatumChange = useCallback((lineIdx: number, stationIdx: number) => {
    setZeroDatumIndices((prev) => ({ ...prev, [lineIdx]: stationIdx }));
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

    // Build GpxLinePlacement objects with proper ImportPlacement zero_datum
    const palette = lineColorPalette(resolvedTheme);
    const placements: GpxLinePlacement[] = lines.map((sec, i) => {
      const stationIdx = zeroDatumIndices[i] ?? 0;
      const point = sec.points[stationIdx] ?? sec.points[0];
      const coords = point
        ? { latitude: point.latitude, longitude: point.longitude, depth_reference: 0 }
        : { latitude: 0, longitude: 0, depth_reference: 0 };

      return {
        line_index: sec.backendIndex,
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

      if (!result.success) {
        surfaceImportFailure('gpx-placement', importUi, { phase: 'command', error: result.error });
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
      setIsImporting(false);
      surfaceImportFailure('gpx-placement', importUi, { phase: 'transport', error: e, format: 'GPX' });
    }
  };

  return (
    <DialogWrapper title="GPX Import - Placement" onClose={closeDialog} size="lg" data-testid="gpx-placement-dialog">
      <div className="space-y-4">
        <div className="text-sm text-text-secondary">
          Configure zero datum placement for each track/route line.
          The selected station will be geo-referenced using its GPS coordinates.
        </div>

        {/* Line list */}
        <div className="max-h-80 overflow-y-auto border border-border rounded">
          <table className="w-full text-sm">
            <thead className="bg-surface-alt sticky top-0">
              <tr className="text-left text-text-tertiary text-xs">
                <th className="px-3 py-1.5">Line</th>
                <th className="px-3 py-1.5">Type</th>
                <th className="px-3 py-1.5">Points</th>
                <th className="px-3 py-1.5">Zero Datum Station</th>
              </tr>
            </thead>
            <tbody>
              {lines.map((sec, i) => (
                <tr key={i} className="border-t border-border">
                  <td className="px-3 py-1.5">{sec.name}</td>
                  <td className="px-3 py-1.5 capitalize text-text-secondary">{sec.type}</td>
                  <td className="px-3 py-1.5">{sec.pointCount}</td>
                  <td className="px-3 py-1.5">
                    <Select
                      value={String(zeroDatumIndices[i] ?? 0)}
                      data-testid={`gpx-placement-zero-datum-${i}`}
                      onChange={(v) => handleZeroDatumChange(i, parseInt(v, 10))}
                      options={Array.from({ length: sec.pointCount }, (_, si) => ({
                        value: String(si),
                        label: `Station ${si}${si === 0 ? ' (first)' : si === sec.pointCount - 1 ? ' (last)' : ''}`,
                      }))}
                      size="sm"
                      aria-label={`Zero datum station for ${sec.name}`}
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        {lines.length === 0 && (
          <div className="text-sm text-text-secondary text-center py-4">
            No track or route lines to configure.
          </div>
        )}

        {/* Footer */}
        <div className="flex justify-between pt-4 border-t border-border">
          <Button onClick={handleBack} disabled={isImporting} data-testid="gpx-placement-back">
            Back
          </Button>
          <div className="flex gap-2">
            <Button onClick={closeDialog} disabled={isImporting} data-testid="gpx-placement-cancel">
              Cancel
            </Button>
            <Button
              data-primary-action=""
              data-testid="gpx-import-confirm"
              onClick={handleImport}
              disabled={isImporting}
            >
              {isImporting ? 'Importing...' : 'Import'}
            </Button>
          </div>
        </div>
      </div>
    </DialogWrapper>
  );
}
