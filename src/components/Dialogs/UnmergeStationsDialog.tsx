import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import type { StationRef } from '../../types/project';
import { formatStationId } from '../../utils/formatters';
import { EntityBlob } from '../common/EntityBlob';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';
import { useShallow } from 'zustand/react/shallow';

function stationRefKey(ref: StationRef): string {
  return `${ref.section_id}:${ref.station_index}`;
}

function sameStationRef(a: StationRef, b: StationRef): boolean {
  return a.section_id === b.section_id && a.station_index === b.station_index;
}

export default function UnmergeStationsDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData } = useUIStore(
    useShallow((s) => ({ closeDialog: s.closeDialog, dialogData: s.dialogData })),
  );
  const { project, stations, unmergeStations, unmergeStationPair } = useProjectStore(
    useShallow((s) => ({
      project: s.project,
      stations: s.stations,
      unmergeStations: s.unmergeStations,
      unmergeStationPair: s.unmergeStationPair,
    })),
  );

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
    const section = project?.sections.find((item) => item.id === ref.section_id);
    const sectionLabel = section?.name?.trim() || t('dialogs.unmergeStations.sectionFallback', { id: ref.section_id });
    return (
      <span className="inline-flex items-center gap-1.5 align-middle">
        <EntityBlob entity="station">{formatStationId(ref.section_id, ref.station_index)}</EntityBlob>
        <EntityBlob entity="section">{sectionLabel}</EntityBlob>
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
        setError(result.error ?? t('dialogs.unmergeStations.unmergeFailed'));
        return;
      }
      trackDialogComplete('unmerge-stations');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.unmergeStations.unmergeFailed')));
    } finally {
      setIsLoading(false);
    }
  };

  if (!stationRef) {
    return (
      <DialogWrapper title={t('dialogs.unmergeStations.title')} onClose={closeDialog} size="md" data-testid="unmerge-stations-dialog">
        <div className="space-y-4">
          <p className="text-error text-sm">{t('dialogs.unmergeStations.notFound')}</p>
          <div className="flex justify-end">
            <Button onClick={closeDialog} data-testid="unmerge-stations-close">{t('common.actions.close')}</Button>
          </div>
        </div>
      </DialogWrapper>
    );
  }

  return (
    <DialogWrapper title={t('dialogs.unmergeStations.title')} onClose={closeDialog} size="md" data-testid="unmerge-stations-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        <p className="text-sm text-text-secondary">
          {t('dialogs.unmergeStations.intro')}
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
                  data-testid={`unmerge-station-option-${ref.section_id}-${ref.station_index}`}
                />
                <span>{getStationDisplayLabel(ref)}</span>
              </label>
            );
          })}
          {mergedStations.length === 0 && (
            <p className="text-sm text-text-secondary" data-testid="unmerge-stations-empty">
              {t('dialogs.unmergeStations.empty')}
            </p>
          )}
        </div>

        <div className="flex justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="unmerge-stations-cancel">{t('common.actions.cancel')}</Button>
          <Button
            variant="primary"
            onClick={handleConfirm}
            disabled={isLoading || !selectedStationRef}
            data-testid="unmerge-stations-confirm"
          >
            {isLoading ? t('dialogs.unmergeStations.unmerging') : t('dialogs.unmergeStations.unmerge')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
