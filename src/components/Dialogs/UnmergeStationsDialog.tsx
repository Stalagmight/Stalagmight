import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import type { StationRef } from '../../types/project';
import { formatStationId } from '../../utils/formatters';
import { EntityBlob } from '../common/EntityBlob';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';

function stationRefKey(ref: StationRef): string {
  return `${ref.line_id}:${ref.station_index}`;
}

function sameStationRef(a: StationRef, b: StationRef): boolean {
  return a.line_id === b.line_id && a.station_index === b.station_index;
}

export default function UnmergeStationsDialog() {
  const { closeDialog, dialogData } = useUIStore();
  const { project, stations, unmergeStations, unmergeStationPair } = useProjectStore();

  const stationRef = dialogData.stationRef as StationRef | undefined;
  const mergedStationsFromDialog = dialogData.mergedStations as StationRef[] | undefined;

  const mergedStations = useMemo(() => {
    if (!stationRef) return [];

    const fromDialog = Array.isArray(mergedStationsFromDialog) ? mergedStationsFromDialog : [];
    const fromStationState = stations.find((station) =>
      sameStationRef(station.station_ref, stationRef)
    )?.merged_with ?? [];

    const merged = fromDialog.length > 0 ? fromDialog : fromStationState;
    const deduped = new Map<string, StationRef>();
    for (const ref of merged) {
      deduped.set(stationRefKey(ref), ref);
    }
    return Array.from(deduped.values());
  }, [mergedStationsFromDialog, stationRef, stations]);

  const [selectedKey, setSelectedKey] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (mergedStations.length > 0) {
      setSelectedKey(stationRefKey(mergedStations[0]!));
    } else {
      setSelectedKey('');
    }
  }, [mergedStations]);

  const selectedStationRef = useMemo(() => (
    mergedStations.find((ref) => stationRefKey(ref) === selectedKey) ?? null
  ), [mergedStations, selectedKey]);

  const getStationDisplayLabel = (ref: StationRef): ReactNode => {
    const line = project?.lines.find((item) => item.id === ref.line_id);
    const lineLabel = line?.name?.trim() || `Line ${ref.line_id}`;
    return (
      <span className="inline-flex items-center gap-1.5 align-middle">
        <EntityBlob entity="station">{formatStationId(ref.line_id, ref.station_index)}</EntityBlob>
        <EntityBlob entity="line">{lineLabel}</EntityBlob>
      </span>
    );
  };

  const isDirectMerge = (a: StationRef, b: StationRef): boolean => {
    return project?.station_merges.some((merge) => {
      const direct = sameStationRef(merge.station_a, a) && sameStationRef(merge.station_b, b);
      const reverse = sameStationRef(merge.station_a, b) && sameStationRef(merge.station_b, a);
      return direct || reverse;
    }) ?? false;
  };

  const handleConfirm = async () => {
    if (!stationRef || !selectedStationRef) return;

    setIsLoading(true);
    setError(null);
    try {
      const result = isDirectMerge(stationRef, selectedStationRef)
        ? await unmergeStationPair(stationRef, selectedStationRef)
        : await unmergeStations(stationRef);
      if (!result.success) {
        setError(result.error ?? 'Failed to unmerge stations');
        return;
      }
      trackDialogComplete('unmerge-stations');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to unmerge stations'));
    } finally {
      setIsLoading(false);
    }
  };

  if (!stationRef) {
    return (
      <DialogWrapper title="Unmerge Stations" onClose={closeDialog} size="md" data-testid="unmerge-stations-dialog">
        <div className="space-y-4">
          <p className="text-error text-sm">Station not found.</p>
          <div className="flex justify-end">
            <Button onClick={closeDialog} data-testid="unmerge-stations-close">Close</Button>
          </div>
        </div>
      </DialogWrapper>
    );
  }

  return (
    <DialogWrapper title="Unmerge Stations" onClose={closeDialog} size="md" data-testid="unmerge-stations-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        <p className="text-sm text-text-secondary">
          This station is merged with multiple stations. Select one merge to remove.
        </p>

        <div className="space-y-2 max-h-56 overflow-y-auto border border-border rounded p-2">
          {mergedStations.map((ref) => {
            const key = stationRefKey(ref);
            return (
              <label key={key} className="flex items-center gap-2 text-sm p-1 rounded hover:bg-background-50">
                <input
                  type="radio"
                  name="unmerge-station"
                  value={key}
                  checked={selectedKey === key}
                  onChange={() => setSelectedKey(key)}
                  data-testid={`unmerge-station-option-${ref.line_id}-${ref.station_index}`}
                />
                <span>{getStationDisplayLabel(ref)}</span>
              </label>
            );
          })}
          {mergedStations.length === 0 && (
            <p className="text-sm text-text-secondary" data-testid="unmerge-stations-empty">
              No merged stations found.
            </p>
          )}
        </div>

        <div className="flex justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="unmerge-stations-cancel">Cancel</Button>
          <Button
            variant="primary"
            onClick={handleConfirm}
            disabled={isLoading || !selectedStationRef}
            data-testid="unmerge-stations-confirm"
          >
            {isLoading ? 'Unmerging...' : 'Unmerge'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
