import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { notifyRemovedClosures } from '../../utils/closureRemovalToast';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { EntityBlob } from '../common/EntityBlob';
import { DialogWrapper } from './DialogWrapper';
import type { StationRef } from '../../types/project';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import type { BatchCommand } from '../../types/commands';
import type { MergeDatumShift } from '../../types/ipc/MergeDatumShift';
import { formatDatumMetres } from '../../utils/zeroDatumPreview';

export default function MergeDialog() {
  const { closeDialog, dialogData, clearMergeStations } = useUIStore();
  const { mergeStations, executeBatch } = useProjectStore();
  const labels = useIdentityLabels();

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
              <EntityBlob entity="station">{labels.station(stationA, true)}</EntityBlob>
            </span>
          </div>
          <div className="flex justify-between items-center py-2 border-b border-border">
            <span className="text-text-secondary">Station B:</span>
            <span className="inline-flex items-center gap-1.5">
              <EntityBlob entity="station">{labels.station(stationB, true)}</EntityBlob>
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

/** What a merge of two placed pieces does to the later-placed one (#741):
 *  which piece moves, how far, and that its datum becomes a GPS fix. */
function DatumShiftNotice({ shift, afterMove }: { shift: MergeDatumShift; afterMove: boolean }) {
  const { t } = useTranslation();
  const labels = useIdentityLabels();
  const [first, ...rest] = shift.moving_line_ids;
  const firstName = first === undefined ? '' : labels.lineName(first);
  const piece = rest.length === 0 ? firstName : t('dialogs.mergeStations.pieceMore', { name: firstName, count: rest.length });
  const station = labels.station(shift.datum_station, true);
  return (
    <p className="p-2 rounded bg-warning-10 text-warning-text text-sm" data-testid="merge-datum-shift">
      {afterMove
        ? t('dialogs.mergeStations.datumShiftAfterMove', { piece, station })
        : t('dialogs.mergeStations.datumShift', { piece, station, distance: formatDatumMetres(shift.shift_m) })}
    </p>
  );
}
