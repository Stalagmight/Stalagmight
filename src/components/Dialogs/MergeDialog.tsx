import { useState, useMemo } from 'react';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { notifyRemovedClosures } from '../../utils/closureRemovalToast';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { EntityBlob } from '../common/EntityBlob';
import { DialogWrapper } from './DialogWrapper';
import type { StationRef } from '../../types/project';
import { formatStationId } from '../../utils/formatters';
import type { BatchCommand } from '../../types/commands';

export default function MergeDialog() {
  const { closeDialog, dialogData, clearMergeStations } = useUIStore();
  const { mergeStations, executeBatch, project } = useProjectStore();

  const stationA = dialogData.stationA as StationRef | undefined;
  const stationB = dialogData.stationB as StationRef | undefined;

  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Get section names
  const sectionAName = useMemo(() => {
    if (!stationA) return 'Unknown';
    return project?.sections.find((s) => s.id === stationA.section_id)?.name ?? `Section ${stationA.section_id}`;
  }, [stationA, project]);

  const sectionBName = useMemo(() => {
    if (!stationB) return 'Unknown';
    return project?.sections.find((s) => s.id === stationB.section_id)?.name ?? `Section ${stationB.section_id}`;
  }, [stationB, project]);

  // Optional callbacks from snap-to-merge drag flow
  const onBeforeConfirm = dialogData.onBeforeConfirm as (() => Promise<void>) | undefined;
  const onCancel = dialogData.onCancel as (() => void) | undefined;
  // Move commands to batch with merge for single-undo snap-to-merge
  const preCommands = dialogData.preCommands as BatchCommand[] | undefined;

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
              <EntityBlob entity="station">{formatStationId(stationA.section_id, stationA.station_index)}</EntityBlob>
              <EntityBlob entity="section">{sectionAName}</EntityBlob>
            </span>
          </div>
          <div className="flex justify-between items-center py-2 border-b border-border">
            <span className="text-text-secondary">Station B:</span>
            <span className="inline-flex items-center gap-1.5">
              <EntityBlob entity="station">{formatStationId(stationB.section_id, stationB.station_index)}</EntityBlob>
              <EntityBlob entity="section">{sectionBName}</EntityBlob>
            </span>
          </div>
        </div>

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
