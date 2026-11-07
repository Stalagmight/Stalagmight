import { useState, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { commitMoveCarry } from '../../store/moveCarry';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { EntityBlob } from '../common/EntityBlob';
import { DialogWrapper } from './DialogWrapper';
import type { StationRef } from '../../types/project';
import { formatStationId } from '../../utils/formatters';
import type { BatchCommand } from '../../types/commands';
import { useShallow } from 'zustand/react/shallow';

export default function MergeDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, clearMergeStations } = useUIStore(
    useShallow((s) => ({
      closeDialog: s.closeDialog,
      dialogData: s.dialogData,
      clearMergeStations: s.clearMergeStations,
    })),
  );
  const { mergeStations, executeBatch, project } = useProjectStore(
    useShallow((s) => ({ mergeStations: s.mergeStations, executeBatch: s.executeBatch, project: s.project })),
  );

  const stationA = dialogData.stationA as StationRef | undefined;
  const stationB = dialogData.stationB as StationRef | undefined;

  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Get section names
  const sectionAName = useMemo(() => {
    if (!stationA) return t('common.states.unknown');
    return project?.sections.find((s) => s.id === stationA.section_id)?.name ?? t('dialogs.merge.sectionFallback', { id: stationA.section_id });
  }, [stationA, project, t]);

  const sectionBName = useMemo(() => {
    if (!stationB) return t('common.states.unknown');
    return project?.sections.find((s) => s.id === stationB.section_id)?.name ?? t('dialogs.merge.sectionFallback', { id: stationB.section_id });
  }, [stationB, project, t]);

  // Optional callbacks from snap-to-merge drag flow
  const onBeforeConfirm = dialogData.onBeforeConfirm as (() => Promise<void>) | undefined;
  const onCancel = dialogData.onCancel as (() => void) | undefined;
  // Move commands to batch with merge for single-undo snap-to-merge
  const preCommands = dialogData.preCommands as BatchCommand[] | undefined;
  // The move carry those moves belong to, when a move tool opened this (GHOST-2):
  // confirming here IS that carry's commit. Absent when nothing is being carried.
  const moveCarryToken = dialogData.moveCarryToken as number | undefined;

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
      const result = preCommands
        ? await (moveCarryToken === undefined
            ? executeBatch([...preCommands, mergeCmd])
            : commitMoveCarry(moveCarryToken, () => executeBatch([...preCommands, mergeCmd])))
        : await mergeStations(stationA, stationB);
      if (result.success) {
        clearMergeStations();
        trackDialogComplete('merge');
        closeDialog();
      } else {
        setError(result.error ?? t('dialogs.merge.mergeFailed'));
      }
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.merge.mergeFailed')));
    } finally {
      setIsLoading(false);
    }
  };

  if (!stationA || !stationB) {
    return null;
  }

  return (
    <DialogWrapper title={t('dialogs.merge.title')} onClose={handleClose} size="lg" data-testid="merge-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">{error}</div>
        )}

        {/* Station details */}
        <div className="space-y-2">
          <div className="flex justify-between items-center py-2 border-b border-border">
            <span className="text-text-secondary">{t('dialogs.merge.stationA')}</span>
            <span className="inline-flex items-center gap-1.5">
              <EntityBlob entity="station">{formatStationId(stationA.section_id, stationA.station_index)}</EntityBlob>
              <EntityBlob entity="section">{sectionAName}</EntityBlob>
            </span>
          </div>
          <div className="flex justify-between items-center py-2 border-b border-border">
            <span className="text-text-secondary">{t('dialogs.merge.stationB')}</span>
            <span className="inline-flex items-center gap-1.5">
              <EntityBlob entity="station">{formatStationId(stationB.section_id, stationB.station_index)}</EntityBlob>
              <EntityBlob entity="section">{sectionBName}</EntityBlob>
            </span>
          </div>
        </div>

        {/* Actions */}
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleClose} disabled={isLoading} data-testid="merge-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button variant="primary" onClick={handleMerge} disabled={isLoading} data-testid="merge-confirm">
            {isLoading ? t('dialogs.merge.merging') : t('dialogs.merge.merge')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
