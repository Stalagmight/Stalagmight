import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { notifyRemovedClosures } from '../../utils/closureRemovalToast';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { StationBlob } from '../common/EntityBlob';
import { DialogWrapper } from './DialogWrapper';
import type { StationRef } from '../../types/project';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { listNames } from '../../utils/listNames';
import type { BatchCommand } from '../../types/commands';
import type { MergeDatumShift } from '../../types/ipc/MergeDatumShift';
import type { MergePreview } from '../../types/ipc/MergePreview';
import { formatDatumMetres } from '../../utils/zeroDatumPreview';
import { formatFixed } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';

/** A move shorter than this (metres) is not worth a sentence: the two
 *  stations already sit on one point, as after a snap. */
const MOVE_SAID_FROM_M = 0.01;

export default function MergeDialog() {
  const { closeDialog, dialogData, clearMergeStations } = useUIStore();
  const { mergeStations, executeBatch } = useProjectStore();

  const stationA = dialogData.stationA as StationRef | undefined;
  const stationB = dialogData.stationB as StationRef | undefined;

  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Optional callbacks from snap-to-merge drag flow
  const onBeforeConfirm = dialogData.onBeforeConfirm as (() => Promise<void>) | undefined;
  const onCancel = dialogData.onCancel as (() => void) | undefined;
  // Move commands to batch with merge for single-undo snap-to-merge
  const preCommands = dialogData.preCommands as BatchCommand[] | undefined;
  // Both pieces placed by their own GPS (#741): the backend's answer to which
  // one moves, from check_merge_connectivity.
  const datumShift = dialogData.datumShift as MergeDatumShift | undefined;
  // What the merge does (demo issue I-4): the piece that moves, how far, and
  // the depths when the two stations disagree, from check_merge_connectivity.
  const mergePreview = dialogData.mergePreview as MergePreview | undefined;

  const handleClose = () => {
    onCancel?.();
    clearMergeStations();
    closeDialog();
  };

  const handleMerge = async () => {
    if (!stationA || !stationB) return;

    setIsLoading(true);
    setError(null);

    try {
      if (onBeforeConfirm) await onBeforeConfirm();
      // When preCommands exist (snap-to-merge), batch move + merge as single undo operation
      const mergeCmd: BatchCommand = { type: 'merge_stations', station_a: stationA, station_b: stationB };
      // The store's mergeStations toasts a closure the merge removed (#738/#741);
      // the batch reports it the same way, so toast it here too.
      const result = preCommands
        ? notifyRemovedClosures(await executeBatch([...preCommands, mergeCmd]))
        : await mergeStations(stationA, stationB);
      if (result.success) {
        clearMergeStations();
        trackDialogComplete('merge');
        closeDialog();
      } else {
        setError(result.error ?? 'Failed to merge stations');
      }
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to merge stations'));
    } finally {
      setIsLoading(false);
    }
  };

  if (!stationA || !stationB) {
    return null;
  }

  return (
    <DialogWrapper title="Merge Stations" onClose={handleClose} size="lg" data-testid="merge-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">{error}</div>
        )}

        {/* Station details */}
        <div className="space-y-2">
          <div className="flex justify-between items-center py-2 border-b border-border">
            <span className="text-text-secondary">Station A:</span>
            <span className="inline-flex items-center gap-1.5">
              <StationBlob station={stationA} withLine />
            </span>
          </div>
          <div className="flex justify-between items-center py-2 border-b border-border">
            <span className="text-text-secondary">Station B:</span>
            <span className="inline-flex items-center gap-1.5">
              <StationBlob station={stationB} withLine />
            </span>
          </div>
        </div>

        {datumShift && (
          <DatumShiftNotice
            shift={datumShift}
            // A snap-to-merge drag moves stations first, so the distance the
            // check measured before the drag is not what the merge will move.
            afterMove={preCommands !== undefined}
          />
        )}

        {mergePreview && (
          <MergePreviewNotice
            preview={mergePreview}
            // The datum notice already says what moves and how far; a snap
            // drag already moved it (the check measured before the drag).
            sayMove={!datumShift && preCommands === undefined}
          />
        )}

        {/* Actions */}
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleClose} disabled={isLoading} data-testid="merge-cancel">
            Cancel
          </Button>
          <Button variant="primary" onClick={handleMerge} disabled={isLoading} data-testid="merge-confirm">
            {isLoading ? 'Merging...' : 'Merge'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}

/** What the merge does before the user confirms it (demo issue I-4): which
 *  piece moves, how far and onto which station, that a picked station is
 *  already a junction and of which lines, and, when the two stations' depths
 *  differ by more than the backend's threshold, every depth reading the joined
 *  station's mean takes, by line, and that mean (as 0.6.0 computed it). At a
 *  junction the two stations' depths alone did not add up to it (demo finding
 *  V-2: "4.0 m and 12.5 m become 9.7 m" counted 12.5 m twice). */
function MergePreviewNotice({ preview, sayMove }: { preview: MergePreview; sayMove: boolean }) {
  const { t } = useTranslation();
  const labels = useIdentityLabels();
  const precision = useDecimalPrecision();
  const depth = (v: number) => `${formatFixed(v, undefined, precision.depth)} ${t('common.units.meters')}`;
  const piece = listNames(preview.moving_line_ids.map(labels.lineName), t);
  const moves = sayMove && preview.shift_m >= MOVE_SAID_FROM_M && preview.moving_line_ids.length > 0;
  if (!moves && !preview.depths && preview.junctions.length === 0) return null;
  return (
    <div className="space-y-2 text-sm">
      {preview.junctions.map((junction) => (
        <p
          key={`${junction.station.line_id}:${junction.station.station_index}`}
          className="p-2 rounded bg-accent-10 text-text-primary"
          data-testid={`merge-preview-junction-${junction.station.line_id}-${junction.station.station_index}`}
        >
          {t('dialogs.mergeStations.alreadyJoined', {
            station: labels.station(junction.station, true),
            lines: listNames(junction.line_ids.map(labels.lineName), t),
          })}
        </p>
      ))}
      {moves && (
        <p className="p-2 rounded bg-accent-10 text-text-primary" data-testid="merge-preview-move">
          {t('dialogs.mergeStations.moves', {
            // The verb agrees with the piece: "Upstream moves", "Upstream and
            // Upstream Side move" (the lines named, up to three, N-13).
            count: preview.moving_line_ids.length,
            piece,
            distance: formatDatumMetres(preview.shift_m),
            station: labels.station(preview.joins, true),
          })}
        </p>
      )}
      {preview.depths && (
        <p className="p-2 rounded bg-warning-10 text-warning-text" data-testid="merge-preview-depths">
          {t('dialogs.mergeStations.depths', {
            // Every reading, by line, none counted away: a list that stopped
            // at three would hide what the mean takes.
            readings: preview.depths.readings
              .map((r) => t('dialogs.mergeStations.depthReading', { depth: depth(r.depth_m), line: labels.lineName(r.line_id) }))
              .join(', '),
            joined: depth(preview.depths.joined_m),
          })}
        </p>
      )}
    </div>
  );
}

/** What a merge of two placed pieces does to the later-placed one (#741):
 *  which piece moves, how far, and that its datum becomes a GPS fix. */
function DatumShiftNotice({ shift, afterMove }: { shift: MergeDatumShift; afterMove: boolean }) {
  const { t } = useTranslation();
  const labels = useIdentityLabels();
  const piece = listNames(shift.moving_line_ids.map(labels.lineName), t);
  const station = labels.station(shift.datum_station, true);
  // The verb agrees with the lines named: "is placed", "are placed".
  const count = shift.moving_line_ids.length;
  return (
    <p className="p-2 rounded bg-warning-10 text-warning-text text-sm" data-testid="merge-datum-shift">
      {afterMove
        ? t('dialogs.mergeStations.datumShiftAfterMove', { piece, station, count })
        : t('dialogs.mergeStations.datumShift', { piece, station, count, distance: formatDatumMetres(shift.shift_m) })}
    </p>
  );
}
