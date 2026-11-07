import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { commitMoveCarry } from '../../store/moveCarry';
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
import { useShallow } from 'zustand/react/shallow';

export default function ClosureDialog() {
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  const { closeDialog, dialogData, clearMergeStations } = useUIStore(
    useShallow((s) => ({
      closeDialog: s.closeDialog,
      dialogData: s.dialogData,
      clearMergeStations: s.clearMergeStations,
    })),
  );
  const { createLoopClosure, executeBatch, project } = useProjectStore(
    useShallow((s) => ({ createLoopClosure: s.createLoopClosure, executeBatch: s.executeBatch, project: s.project })),
  );

  const stationA = dialogData.stationA as StationRef | undefined;
  const stationB = dialogData.stationB as StationRef | undefined;
  const preview = (dialogData.preview as ClosurePreview | null) ?? undefined;
  const reason = dialogData.reason as string | undefined;

  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Get section names
  const sectionAName = useMemo(() => {
    if (!stationA) return t('common.states.unknown');
    return project?.sections.find((s) => s.id === stationA.section_id)?.name ?? t('dialogs.closure.sectionFallback', { id: stationA.section_id });
  }, [stationA, project, t]);

  const sectionBName = useMemo(() => {
    if (!stationB) return t('common.states.unknown');
    return project?.sections.find((s) => s.id === stationB.section_id)?.name ?? t('dialogs.closure.sectionFallback', { id: stationB.section_id });
  }, [stationB, project, t]);

  // Optional callbacks from snap-to-merge drag flow
  const onBeforeConfirm = dialogData.onBeforeConfirm as (() => Promise<void>) | undefined;
  const onCancel = dialogData.onCancel as (() => void) | undefined;
  // Move commands to batch with closure for single-undo snap-to-merge
  const preCommands = dialogData.preCommands as BatchCommand[] | undefined;
  // The move carry those moves belong to, when a move tool opened this (GHOST-2):
  // confirming here IS that carry's commit. Absent when nothing is being carried.
  const moveCarryToken = dialogData.moveCarryToken as number | undefined;
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
        ? await (moveCarryToken === undefined
            ? executeBatch([...preCommands, closureCmd])
            : commitMoveCarry(moveCarryToken, () => executeBatch([...preCommands, closureCmd])))
        : await createLoopClosure(stationA, stationB, effectiveStrategy);
      if (result.success) {
        clearMergeStations();
        trackDialogComplete('closure');
        closeDialog();
      } else {
        setError(result.error ?? t('dialogs.closure.createFailed'));
      }
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.closure.createFailed')));
    } finally {
      setIsLoading(false);
    }
  };

  if (!stationA || !stationB) {
    return null;
  }

  return (
    <DialogWrapper title={t('dialogs.closure.title')} onClose={handleClose} size="lg" data-testid="closure-dialog">
      <div className="space-y-4">
        {reason && (
          <p className="text-sm text-text-secondary">{reason}</p>
        )}
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">{error}</div>
        )}

        {/* Station details */}
        <div className="space-y-2">
          <div className="flex justify-between items-center py-2 border-b border-border">
            <span className="text-text-secondary">{t('dialogs.closure.stationA')}</span>
            <span className="inline-flex items-center gap-1.5">
              <EntityBlob entity="station">{formatStationId(stationA.section_id, stationA.station_index)}</EntityBlob>
              <EntityBlob entity="section">{sectionAName}</EntityBlob>
            </span>
          </div>
          <div className="flex justify-between items-center py-2 border-b border-border">
            <span className="text-text-secondary">{t('dialogs.closure.stationB')}</span>
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
              <span className="text-text-secondary">{t('dialogs.closure.error')}</span>
              <span className="font-medium" style={{ color: getErrorColor(preview.error_percentage) }}>
                {formatPercent(preview.error_percentage, undefined, 1)}
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-text-secondary">{t('dialogs.closure.pathLength')}</span>
              <span>{formatFixed(preview.path_length, undefined, precision.length)}{t('common.units.meters')}</span>
            </div>
            <div className="flex justify-between">
              <span className="text-text-secondary">{t('dialogs.closure.misclosureDistance')}</span>
              <span>{formatFixed(preview.misclosure_distance, undefined, precision.length)}{t('common.units.meters')}</span>
            </div>
            <div className="flex justify-between">
              <span className="text-text-secondary">{t('dialogs.closure.horizontalMisclosure')}</span>
              <span>{formatFixed(preview.horizontal_misclosure, undefined, precision.length)}{t('common.units.meters')}</span>
            </div>
            <div className="flex justify-between">
              <span className="text-text-secondary">{t('dialogs.closure.verticalMisclosure')}</span>
              <span>{formatFixed(preview.vertical_misclosure, undefined, precision.length)}{t('common.units.meters')}</span>
            </div>
          </div>
        )}

        {/* Actions */}
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleClose} disabled={isLoading} data-testid="closure-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button variant="primary" onClick={handleCreate} disabled={isLoading} data-testid="closure-create">
            {isLoading ? t('dialogs.closure.creating') : t('dialogs.closure.create')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
