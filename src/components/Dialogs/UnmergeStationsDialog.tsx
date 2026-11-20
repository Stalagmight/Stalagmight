import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import type { StationRef } from '../../types/project';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
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

/**
 * Detach a line from a station several lines share (plan 5.2; the dialog kept
 * its `unmerge-stations` name for telemetry).
 *
 * On the station graph a merge is ONE station that two or more lines pass. The
 * user picks which line leaves it: that line gets its own copy of the station
 * at the same place (the name copied, no comment), and the datums and closures
 * attached through it go along; the other lines stay joined. The clicked slot's
 * line is listed first and preselected. Its own line detaches with
 * `unmerge_stations(slot)`; another line with `unmerge_station_pair(slot,
 * other)`, which detaches the PARTNER's line.
 */
export default function UnmergeStationsDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData } = useUIStore();
  const { stations, unmergeStations, unmergeStationPair } = useProjectStore();
  const labels = useIdentityLabels();

  const stationRef = dialogData.stationRef as StationRef | undefined;
  const mergedStationsFromDialog = dialogData.mergedStations as StationRef[] | undefined;

  /** Every slot of the station: the clicked one first, then the other lines through it. */
  const slots = useMemo(() => {
    if (!stationRef) return [];
    const fromDialog = Array.isArray(mergedStationsFromDialog) ? mergedStationsFromDialog : [];
    const others =
      fromDialog.length > 0
        ? fromDialog
        : stations.find((station) => sameStationRef(station.station_ref, stationRef))?.merged_with ?? [];
    const deduped = new Map<string, StationRef>([[stationRefKey(stationRef), stationRef]]);
    for (const ref of others) deduped.set(stationRefKey(ref), ref);
    return Array.from(deduped.values());
  }, [mergedStationsFromDialog, stationRef, stations]);

  const [selectedKey, setSelectedKey] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setSelectedKey(slots.length > 0 ? stationRefKey(slots[0]!) : '');
  }, [slots]);

  const selected = useMemo(
    () => slots.find((ref) => stationRefKey(ref) === selectedKey) ?? null,
    [slots, selectedKey],
  );

  const handleConfirm = async () => {
    if (!stationRef || !selected) return;

    setIsLoading(true);
    setError(null);
    try {
      const result = sameStationRef(selected, stationRef)
        ? await unmergeStations(stationRef)
        : await unmergeStationPair(stationRef, selected);
      if (!result.success) {
        setError(result.error ?? t('dialogs.detachLine.failed'));
        return;
      }
      trackDialogComplete('unmerge-stations');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.detachLine.failed')));
    } finally {
      setIsLoading(false);
    }
  };

  if (!stationRef) {
    return (
      <DialogWrapper title={t('dialogs.detachLine.title')} onClose={closeDialog} size="md" data-testid="unmerge-stations-dialog">
        <div className="space-y-4">
          <p className="text-error text-sm">{t('dialogs.detachLine.notFound')}</p>
          <div className="flex justify-end">
            <Button onClick={closeDialog} data-testid="unmerge-stations-close">{t('dialogs.detachLine.close')}</Button>
          </div>
        </div>
      </DialogWrapper>
    );
  }

  return (
    <DialogWrapper title={t('dialogs.detachLine.title')} onClose={closeDialog} size="md" data-testid="unmerge-stations-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        <p className="text-sm text-text-secondary" data-testid="unmerge-stations-lead">
          {t('dialogs.detachLine.lead', { station: labels.station(stationRef), count: slots.length })}
        </p>

        <div className="space-y-2 max-h-56 overflow-y-auto border border-border rounded p-2">
          {slots.map((ref, i) => {
            const key = stationRefKey(ref);
            const line = labels.lineName(ref.line_id);
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
                <EntityBlob entity="line">{i === 0 ? t('dialogs.detachLine.thisLine', { line }) : line}</EntityBlob>
              </label>
            );
          })}
        </div>

        <p className="text-xs text-text-secondary">{t('dialogs.detachLine.explain')}</p>

        <div className="flex justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="unmerge-stations-cancel">
            {t('dialogs.detachLine.cancel')}
          </Button>
          <Button
            variant="primary"
            onClick={handleConfirm}
            disabled={isLoading || !selected}
            data-testid="unmerge-stations-confirm"
          >
            {isLoading ? t('dialogs.detachLine.working') : t('dialogs.detachLine.confirm')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
