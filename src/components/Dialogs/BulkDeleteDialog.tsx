import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import i18n from '../../i18n';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { invokeIpc, getErrorMessage } from '../../utils/ipc';
import { formatStationId } from '../../utils/formatters';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import DeleteShotImpactDetails from './DeleteShotImpactDetails';
import type { DeleteShotRangeImpact } from '../../types/commands';
import type { StationRef } from '../../types/project';
import { setSource, trackDialogComplete } from '../../utils/telemetry';
import { useShallow } from 'zustand/react/shallow';

/**
 * Confirm + impact dialog for deleting a contiguous range of shots (#95).
 * Opened by the data table when >1 real shot is selected. Interior ranges offer
 * a choice - split the section in two (keeps both ends in place) or close the
 * gap (shift the downstream survey). Prefix/suffix ranges only shift. Splitting
 * drops any zero datum / comment on a dropped interior station (warned below);
 * a gap-close that would drop a datum is blocked instead.
 * (Whole-section selections route to the section-delete confirm instead.)
 */
export default function BulkDeleteDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, clearSelection, setSelection } = useUIStore(
    useShallow((s) => ({
      closeDialog: s.closeDialog,
      dialogData: s.dialogData,
      clearSelection: s.clearSelection,
      setSelection: s.setSelection,
    })),
  );
  const { deleteShotRangeSplit, deleteShotRangeShift } = useProjectStore(
    useShallow((s) => ({ deleteShotRangeSplit: s.deleteShotRangeSplit, deleteShotRangeShift: s.deleteShotRangeShift })),
  );

  const sectionId = dialogData.sectionId as number | undefined;
  const startShotId = dialogData.startShotId as number | undefined;
  const endShotId = dialogData.endShotId as number | undefined;
  const shotCount = dialogData.shotCount as number | undefined;
  const kind = dialogData.kind as 'prefix' | 'suffix' | 'interior' | undefined;
  const sectionName = dialogData.sectionName as string | undefined;
  const startStationIndex = dialogData.startStationIndex as number | undefined;
  const endStationIndex = dialogData.endStationIndex as number | undefined;

  const isInterior = kind === 'interior';

  const [splitNameA, setSplitNameA] = useState('');
  const [splitNameB, setSplitNameB] = useState('');
  const [impact, setImpact] = useState<DeleteShotRangeImpact | null>(null);
  const [impactError, setImpactError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);

  const shiftBlocked = impact?.shift_blocked ?? false;
  const shiftBlockedReasons = impact?.shift_blocked_reasons ?? [];
  const removedDatums = impact?.removed_datums ?? [];
  const removedComments = impact?.removed_comments ?? [];
  // `removed_comments` covers every station the gap-close removes (the superset).
  // Frame it per action: an interior SPLIT keeps both boundary stations, so it
  // only drops comments on the strictly-interior stations (index <= end_pos);
  // the tail boundary (index == end_pos+1) falls only to the gap-close. For
  // prefix/suffix there is no split, so the gap-close drops all of them.
  const endPos = impact?.end_pos ?? -1;
  const splitComments = isInterior
    ? removedComments.filter((c: StationRef) => c.station_index <= endPos)
    : [];
  const gapComments = shiftBlocked
    ? []
    : isInterior
      ? removedComments.filter((c: StationRef) => c.station_index > endPos)
      : removedComments;
  const fmt = (s: StationRef) => formatStationId(s.section_id, s.station_index);
  // The preview is the only source of the datum/comment/shift-blocked warnings,
  // so keep the destructive actions disabled until it resolves (loaded or
  // errored) - otherwise a fast click could confirm before the warning renders.
  const previewReady = impact !== null || impactError !== null;

  // Prefill split names for interior ranges (mirrors the single-shot dialog).
  useEffect(() => {
    if (isInterior && sectionName) {
      setSplitNameA(`${sectionName}-A`);
      setSplitNameB(`${sectionName}-B`);
    }
  }, [isInterior, sectionName]);

  useEffect(() => {
    if (sectionId !== undefined && startShotId !== undefined && endShotId !== undefined) {
      invokeIpc(
        'preview_delete_shot_range',
        { sectionId, startShotId, endShotId },
        { purpose: 'preview shot range deletion' }
      )
        .then((result) => {
          setImpact(result);
          setImpactError(null);
        })
        .catch((e) => {
          setImpact(null);
          // i18n.t, not the hook's t: this IPC effect must not re-fetch on a language change.
          setImpactError(getErrorMessage(e, i18n.t('dialogs.bulkDelete.errors.loadImpactFailed')));
        });
    } else {
      setImpact(null);
      setImpactError(null);
    }
  }, [sectionId, startShotId, endShotId]);

  const summary =
    shotCount !== undefined &&
    startStationIndex !== undefined &&
    endStationIndex !== undefined &&
    sectionId !== undefined
      ? t('dialogs.bulkDelete.summaryRange', {
          count: shotCount,
          from: formatStationId(sectionId, startStationIndex),
          to: formatStationId(sectionId, endStationIndex),
          section: sectionName ?? t('dialogs.bulkDelete.sectionFallback', { id: sectionId }),
        })
      : t('dialogs.bulkDelete.summary', { count: shotCount ?? 0 });

  const finish = (split: boolean) => {
    // Split keeps the original section as "A" - re-select it; otherwise clear.
    if (split && sectionId !== undefined) {
      setSelection({ type: 'section', sectionId });
    } else {
      clearSelection();
    }
    trackDialogComplete('bulk-delete');
    closeDialog();
  };

  const handleSplit = async () => {
    if (sectionId === undefined || startShotId === undefined || endShotId === undefined) return;
    setSource('dialog');
    setIsLoading(true);
    setError(null);
    try {
      const result = await deleteShotRangeSplit(
        sectionId,
        startShotId,
        endShotId,
        splitNameA.trim() || t('dialogs.bulkDelete.defaultSplitNameA'),
        splitNameB.trim() || t('dialogs.bulkDelete.defaultSplitNameB')
      );
      if (result.success) {
        finish(true);
      } else {
        setError(result.error ?? t('dialogs.bulkDelete.errors.splitSection'));
      }
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.bulkDelete.errors.splitSection')));
    } finally {
      setIsLoading(false);
    }
  };

  const handleShift = async () => {
    if (sectionId === undefined || startShotId === undefined || endShotId === undefined) return;
    setSource('dialog');
    setIsLoading(true);
    setError(null);
    try {
      const result = await deleteShotRangeShift(sectionId, startShotId, endShotId);
      if (result.success) {
        finish(false);
      } else {
        setError(result.error ?? t('dialogs.bulkDelete.errors.deleteShots'));
      }
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.bulkDelete.errors.deleteShots')));
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <DialogWrapper
      title={t('dialogs.bulkDelete.title')}
      onClose={isLoading ? () => {} : closeDialog}
      size="md"
      data-testid="bulk-delete-dialog"
    >
      <div className="space-y-4">
        <p className="text-text-secondary">{summary}</p>
        {isInterior && (
          <p className="text-text-secondary text-sm">
            {t('dialogs.bulkDelete.interiorIntro')}
          </p>
        )}

        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}
        {impactError && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
            {impactError}
          </div>
        )}

        {isInterior && (
          <div className="space-y-3">
            <div>
              <label className="block text-sm text-text-secondary mb-1">
                {t('dialogs.bulkDelete.splitNameA')}
              </label>
              <input
                type="text"
                value={splitNameA}
                onChange={(e) => setSplitNameA(e.target.value)}
                className="w-full"
                data-testid="split-name-a"
              />
            </div>
            <div>
              <label className="block text-sm text-text-secondary mb-1">
                {t('dialogs.bulkDelete.splitNameB')}
              </label>
              <input
                type="text"
                value={splitNameB}
                onChange={(e) => setSplitNameB(e.target.value)}
                className="w-full"
                data-testid="split-name-b"
              />
            </div>
          </div>
        )}

        {((isInterior && removedDatums.length > 0) ||
          splitComments.length > 0 ||
          gapComments.length > 0) && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
            <p className="font-medium">{t('dialogs.bulkDelete.anchoredHeading')}</p>
            {isInterior && removedDatums.length > 0 && (
              <p>{t('dialogs.bulkDelete.zeroDatums', { stations: removedDatums.map(fmt).join(', ') })}</p>
            )}
            {splitComments.length > 0 && (
              <p>{t('dialogs.bulkDelete.stationComments', { stations: splitComments.map(fmt).join(', ') })}</p>
            )}
            {gapComments.length > 0 && (
              <p>
                {isInterior
                  ? t('dialogs.bulkDelete.gapCommentsInterior', { stations: gapComments.map(fmt).join(', ') })
                  : t('dialogs.bulkDelete.stationComments', { stations: gapComments.map(fmt).join(', ') })}
              </p>
            )}
            <p>{t('dialogs.bulkDelete.undoHint')}</p>
          </div>
        )}

        {shiftBlocked && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
            <p className="font-medium">{t('dialogs.bulkDelete.shiftUnavailable')}</p>
            {shiftBlockedReasons.length > 0 && (
              <ul className="list-disc list-inside">
                {shiftBlockedReasons.map((reason: string, index: number) => (
                  <li key={`shift-blocked-${index}`}>{reason}</li>
                ))}
              </ul>
            )}
            {!isInterior && removedDatums.length > 0 && (
              <p className="mt-1">{t('dialogs.bulkDelete.blockingDatum', { stations: removedDatums.map(fmt).join(', ') })}</p>
            )}
            {!isInterior && (
              <p className="mt-1">
                {t('dialogs.bulkDelete.edgeBlocked')}
              </p>
            )}
          </div>
        )}

        <DeleteShotImpactDetails
          impact={impact}
          title={t('dialogs.bulkDelete.impactTitle')}
        />

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="bulk-delete-cancel">
            {t('common.actions.cancel')}
          </Button>
          {isInterior && (
            <Button
              onClick={handleSplit}
              disabled={isLoading || !previewReady}
              data-testid="bulk-delete-split"
            >
              {isLoading ? t('dialogs.bulkDelete.working') : t('dialogs.bulkDelete.split')}
            </Button>
          )}
          <Button
            variant="danger"
            onClick={handleShift}
            disabled={isLoading || shiftBlocked || !previewReady}
            data-primary-action=""
            data-testid="bulk-delete-shift"
          >
            {isLoading ? t('dialogs.bulkDelete.deleting') : isInterior ? t('dialogs.bulkDelete.closeGapDelete') : t('dialogs.bulkDelete.deleteShots')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
