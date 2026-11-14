import { useMemo, useState } from 'react';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { EntityBlob } from '../common/EntityBlob';
import { DialogWrapper } from './DialogWrapper';
import { formatStationId } from '../../utils/formatters';
import { getErrorColor } from '../../utils/colors';
import { formatFixed, formatPercent } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import type { StationRef, ClosureStrategy } from '../../types/project';
import type { ClosurePreview, BatchCommand } from '../../types/commands';

export default function ClosureDialog() {
  const precision = useDecimalPrecision();
  const { closeDialog, dialogData, clearMergeStations } = useUIStore();
  const { createLoopClosure, executeBatch, project } = useProjectStore();

  const stationA = dialogData.stationA as StationRef | undefined;
  const stationB = dialogData.stationB as StationRef | undefined;
  const preview = (dialogData.preview as ClosurePreview | null) ?? undefined;
  const reason = dialogData.reason as string | undefined;
  // Why the backend would refuse this closure (#737): it would tie the two ends
  // of one shot together, or the pair is already tied. The dialog says so up
  // front and offers no Create; the backend refuses the pair regardless.
  const closureRefusal = dialogData.closureRefusal as string | undefined;

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
  // Move commands to batch with closure for single-undo snap-to-merge
  const preCommands = dialogData.preCommands as BatchCommand[] | undefined;
  const isFromMove = !!preCommands;

  const handleClose = () => {
    onCancel?.();
    clearMergeStations();
    closeDialog();
  };

  const handleCreate = async () => {
    if (!stationA || !stationB) return;

    setIsLoading(true);
    setError(null);

    try {
      if (onBeforeConfirm) await onBeforeConfirm();
      // When preCommands exist (snap-to-merge), batch move + closure as single undo operation
      const effectiveStrategy = isFromMove ? 'Move' as ClosureStrategy : 'Batch';
      const closureCmd: BatchCommand = { type: 'create_loop_closure', station_a: stationA, station_b: stationB, closure_strategy: effectiveStrategy };
      const result = preCommands
        ? await executeBatch([...preCommands, closureCmd])
        : await createLoopClosure(stationA, stationB, effectiveStrategy);
      if (result.success) {
        clearMergeStations();
        trackDialogComplete('closure');
        closeDialog();
      } else {
        setError(result.error ?? 'Failed to create loop closure');
      }
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to create loop closure'));
    } finally {
      setIsLoading(false);
    }
  };

  if (!stationA || !stationB) {
    return null;
  }

  return (
    <DialogWrapper title="Create Loop Closure" onClose={handleClose} size="lg" data-testid="closure-dialog">
      <div className="space-y-4">
        {reason && (
          <p className="text-sm text-text-secondary">{reason}</p>
        )}
        {closureRefusal && (
          <div
            role="alert"
            className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm"
            data-testid="closure-refusal"
          >
            {closureRefusal}
          </div>
        )}
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

        {/* Preview data */}
        {preview && (
          <div className="p-3 bg-background rounded space-y-1">
            <div className="flex justify-between">
              <span className="text-text-secondary">Error:</span>
              <span className="font-medium" style={{ color: getErrorColor(preview.error_percentage) }}>
                {formatPercent(preview.error_percentage, undefined, 1)}
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-text-secondary">Path length:</span>
              <span>{formatFixed(preview.path_length, undefined, precision.length)}m</span>
            </div>
            <div className="flex justify-between">
              <span className="text-text-secondary">Misclosure distance:</span>
              <span>{formatFixed(preview.misclosure_distance, undefined, precision.length)}m</span>
            </div>
            <div className="flex justify-between">
              <span className="text-text-secondary">Horizontal misclosure:</span>
              <span>{formatFixed(preview.horizontal_misclosure, undefined, precision.length)}m</span>
            </div>
            <div className="flex justify-between">
              <span className="text-text-secondary">Vertical misclosure:</span>
              <span>{formatFixed(preview.vertical_misclosure, undefined, precision.length)}m</span>
            </div>
          </div>
        )}

        {/* Actions */}
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleClose} disabled={isLoading} data-testid="closure-cancel">
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={handleCreate}
            disabled={isLoading || !!closureRefusal}
            title={closureRefusal}
            data-testid="closure-create"
          >
            {isLoading ? 'Creating...' : 'Create Closure'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
