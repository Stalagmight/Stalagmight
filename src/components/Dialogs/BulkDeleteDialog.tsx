import { useEffect, useState } from 'react';
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
  const { closeDialog, dialogData, clearSelection, setSelection } = useUIStore();
  const { deleteShotRangeSplit, deleteShotRangeShift } = useProjectStore();

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
      invokeIpc<DeleteShotRangeImpact>(
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
          setImpactError(getErrorMessage(e, 'Failed to load deletion impact'));
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
      ? `Delete ${shotCount} shots (stations ${formatStationId(sectionId, startStationIndex)}-${formatStationId(sectionId, endStationIndex)} in ${sectionName ?? `Section ${sectionId}`}).`
      : `Delete ${shotCount ?? 0} shots.`;

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
        splitNameA.trim() || 'Section A',
        splitNameB.trim() || 'Section B'
      );
      if (result.success) {
        finish(true);
      } else {
        setError(result.error ?? 'Failed to split section');
      }
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to split section'));
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
        setError(result.error ?? 'Failed to delete shots');
      }
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to delete shots'));
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <DialogWrapper
      title="Delete Shots"
      onClose={isLoading ? () => {} : closeDialog}
      size="md"
      data-testid="bulk-delete-dialog"
    >
      <div className="space-y-4">
        <p className="text-text-secondary">{summary}</p>
        {isInterior && (
          <p className="text-text-secondary text-sm">
            These shots are in the middle of the section. Split keeps both ends in place; closing
            the gap shifts the downstream survey. The relationship changes below cover closing the
            gap - the split keeps the stations at each end and their connections.
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
                Section A name (before the range)
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
                Section B name (after the range)
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
            <p className="font-medium">Deleting these shots will remove anchored data.</p>
            {isInterior && removedDatums.length > 0 && (
              <p>Zero datums (anchor the survey&apos;s real-world position) on {removedDatums.map(fmt).join(', ')}.</p>
            )}
            {splitComments.length > 0 && (
              <p>Station comments on {splitComments.map(fmt).join(', ')}.</p>
            )}
            {gapComments.length > 0 && (
              <p>
                {isInterior
                  ? `Closing the gap also removes comments on ${gapComments.map(fmt).join(', ')} (splitting keeps them).`
                  : `Station comments on ${gapComments.map(fmt).join(', ')}.`}
              </p>
            )}
            <p>You can undo to restore them.</p>
          </div>
        )}

        {shiftBlocked && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
            <p className="font-medium">Closing the gap isn&apos;t available for this range.</p>
            {shiftBlockedReasons.length > 0 && (
              <ul className="list-disc list-inside">
                {shiftBlockedReasons.map((reason: string, index: number) => (
                  <li key={`shift-blocked-${index}`}>{reason}</li>
                ))}
              </ul>
            )}
            {!isInterior && removedDatums.length > 0 && (
              <p className="mt-1">Blocking zero datum on {removedDatums.map(fmt).join(', ')}.</p>
            )}
            {!isInterior && (
              <p className="mt-1">
                This range can only be removed by closing the gap, so it can&apos;t be deleted here.
                Remove the blocking zero datum or loop closure first, delete the shots one at a
                time, or delete the whole section.
              </p>
            )}
          </div>
        )}

        <DeleteShotImpactDetails
          impact={impact}
          title="Deleting these shots will update station relationships:"
          gapCloseAction={isInterior ? 'Close Gap & Delete' : 'Delete Shots'}
        />

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="bulk-delete-cancel">
            Cancel
          </Button>
          {isInterior && (
            <Button
              onClick={handleSplit}
              disabled={isLoading || !previewReady}
              data-testid="bulk-delete-split"
            >
              {isLoading ? 'Working…' : 'Split into Two Sections'}
            </Button>
          )}
          <Button
            variant="danger"
            onClick={handleShift}
            disabled={isLoading || shiftBlocked || !previewReady}
            data-primary-action=""
            data-testid="bulk-delete-shift"
          >
            {isLoading ? 'Deleting…' : isInterior ? 'Close Gap & Delete' : 'Delete Shots'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
