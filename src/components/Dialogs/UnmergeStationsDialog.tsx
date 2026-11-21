import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage, invokeIpc } from '../../utils/ipc';
import type { StationRef } from '../../types/project';
import type { DetachLinePreview } from '../../types/ipc.generated';
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
 * The slot to preselect (demo finding I-7): the clicked one (`slots[0]`),
 * unless its line would NOT get its own station back (it is the junction's
 * owner) and another line would: then the first such line. While the previews
 * are still out, or when none says, the clicked one.
 */
export function defaultDetachSlot(
  slots: StationRef[],
  previews: ReadonlyMap<number, DetachLinePreview>,
): StationRef | undefined {
  const [clicked, ...others] = slots;
  if (!clicked || previews.get(clicked.line_id)?.restored !== false) return clicked;
  return others.find((ref) => previews.get(ref.line_id)?.restored === true) ?? clicked;
}

/**
 * Detach a line from a station several lines share (plan 5.2; the dialog kept
 * its `unmerge-stations` name for telemetry).
 *
 * On the station graph a merge is ONE station that two or more lines pass. The
 * user picks which line leaves it: that line gets its own station at the same
 * place (no comment), and the datums and closures attached through it go
 * along; the other lines stay joined. A line that was merged there gets back
 * the Station ID and name it had (the junction's alias naming it); any other
 * gets the junction's name, made unique. The dialog says which, from the
 * backend's `preview_detach_line` (review finding UX-4), asked for every
 * line when the dialog opens. The clicked slot's line is listed first. It is
 * preselected unless it is the junction's owner (a line no alias names, so
 * detaching it would give it a NEW Station ID and take the station's comment
 * away from it) while another line was merged there: that line, which gets
 * its own station back, is then the likely intent and preselected (demo
 * finding I-7). The comment stays with the other lines; for a line that gets
 * its station back the dialog says so, since a merge (or the 0.6.0 upgrade)
 * may have joined that line's own comment into it and the alias does not keep
 * it. Its own line detaches with `unmerge_stations(slot)`; another line with
 * `unmerge_station_pair(slot, other)`, which detaches the PARTNER's line.
 */
export default function UnmergeStationsDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData } = useUIStore();
  const { stations, unmergeStations, unmergeStationPair } = useProjectStore();
  const labels = useIdentityLabels();

  const stationRef = dialogData.stationRef as StationRef | undefined;
  const mergedStationsFromDialog = dialogData.mergedStations as StationRef[] | undefined;
  const stationComment = stationRef
    ? stations.find((station) => sameStationRef(station.station_ref, stationRef))?.comment ?? ''
    : '';

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
  // What each line would get, by line id. A failed preview only hides that
  // line's sentence; the detach reports its own refusal.
  const [previews, setPreviews] = useState<ReadonlyMap<number, DetachLinePreview>>(new Map());
  // Once the user picks a line, the default stops following the previews.
  const pickedRef = useRef(false);

  useEffect(() => {
    pickedRef.current = false;
    setPreviews(new Map());
    setSelectedKey(slots.length > 0 ? stationRefKey(slots[0]!) : '');
    if (!stationRef || slots.length === 0) return;
    let live = true;
    void Promise.all(
      slots.map((ref) =>
        invokeIpc<DetachLinePreview>(
          'preview_detach_line',
          { stationRef, lineId: ref.line_id },
          { purpose: 'preview detach line' },
        )
          .then((preview): [number, DetachLinePreview] | null => [ref.line_id, preview])
          .catch(() => null),
      ),
    ).then((answers) => {
      if (!live) return;
      const byLine = new Map(answers.filter((a): a is [number, DetachLinePreview] => a !== null));
      setPreviews(byLine);
      const preferred = defaultDetachSlot(slots, byLine);
      if (!pickedRef.current && preferred) setSelectedKey(stationRefKey(preferred));
    });
    return () => {
      live = false;
    };
  }, [slots, stationRef]);

  const selected = useMemo(
    () => slots.find((ref) => stationRefKey(ref) === selectedKey) ?? null,
    [slots, selectedKey],
  );
  const naming = selected ? previews.get(selected.line_id) ?? null : null;

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
                  onChange={() => {
                    pickedRef.current = true;
                    setSelectedKey(key);
                  }}
                  data-testid={`unmerge-station-option-${ref.line_id}-${ref.station_index}`}
                />
                <EntityBlob entity="line">{i === 0 ? t('dialogs.detachLine.thisLine', { line }) : line}</EntityBlob>
              </label>
            );
          })}
        </div>

        {naming && selected && (
          <p className="text-sm text-text-primary" data-testid="unmerge-stations-naming">
            {naming.restored
              ? t('dialogs.detachLine.restores', {
                  line: labels.lineName(selected.line_id),
                  name: naming.name,
                  id: naming.station_id,
                })
              : t('dialogs.detachLine.copies', { line: labels.lineName(selected.line_id), name: naming.name })}
          </p>
        )}

        {naming?.restored && selected && stationComment.trim() !== '' && (
          <p className="text-sm text-text-secondary" data-testid="unmerge-stations-comment">
            {t('dialogs.detachLine.commentStays', { line: labels.lineName(selected.line_id) })}
          </p>
        )}

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
