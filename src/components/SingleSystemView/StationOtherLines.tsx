import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useEditingBlocked } from '../../hooks/useEditingBlocked';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { findStation } from '../../utils/findStation';
import { detachLineAsking } from '../../utils/keptStationActions';
import { setSource, trackContextMenuAction } from '../../utils/telemetry';
import type { Station, StationRef } from '../../types/project';
import type { ArchivedStation } from '../../types/ipc/ArchivedStation';
import { DetailLink } from '../common/DetailLink';
import { LinkOffIcon } from '../common/icons';

/**
 * The record of a node that names a line (RS 3: at most one does): that
 * line's own station, which a merge archived and Detach gives back.
 */
export function lineRecord(records: ReadonlyArray<ArchivedStation> | undefined, lineId: number): ArchivedStation | undefined {
  return records?.find((record) => record.lines.includes(lineId));
}

/**
 * The Station details' other lines through a junction, "Also on" (demo
 * findings N-3 and I-9, RS 6). One row per other line: the line, a link to
 * its slot. Every line shows the junction's one station (the kept one, RS
 * 4.1), so a row says no name of its own; the stations merges archived here
 * are listed under it (`StationRecords`). Each row has the button that
 * detaches that line; when the detach brings a station back, takes other
 * lines with it or would repeat a name, the button opens Detach Line from
 * Station on that line instead (RS 7).
 *
 * The heading is the same for every junction however it was made (a merge,
 * Split Line Here, a line started from the station, an import).
 *
 * A junction is ONE station several lines pass (plan 5.2), and any two of its
 * slots are a pair: the pair form detaches the PARTNER's line, which is the
 * row the user clicked.
 */
export function StationOtherLines({ station }: { station: Station }) {
  const { t } = useTranslation();
  const stations = useProjectStore((s) => s.stations);
  const { setSelection, announceSelection, setHoveredStation } = useUIStore(
    useShallow((s) => ({
      setSelection: s.setSelection,
      announceSelection: s.announceSelection,
      setHoveredStation: s.setHoveredStation,
    })),
  );
  const editingBlocked = useEditingBlocked();
  const labels = useIdentityLabels();
  const [detachingKey, setDetachingKey] = useState<string | null>(null);

  if (station.merged_with.length === 0) return null;

  const detach = async (ref: StationRef) => {
    trackContextMenuAction('unmerge', 'detail-panel');
    setDetachingKey(`${ref.line_id}:${ref.station_index}`);
    try {
      await detachLineAsking(station.station_ref, ref, station.merged_with);
    } finally {
      setDetachingKey(null);
    }
  };

  return (
    <div className="pt-2 border-t border-border" data-testid="station-detail-merged-with">
      <span className="text-sm text-text-secondary" title={t('detailPanel.station.alsoOnHint')}>
        {t('detailPanel.station.alsoOn')}:
      </span>
      <ul className="mt-1 space-y-1">
        {station.merged_with.map((ref) => {
          const key = `${ref.line_id}:${ref.station_index}`;
          const line = labels.lineName(ref.line_id);
          return (
            <li key={key} className="text-sm flex items-center justify-between gap-2">
              <span className="min-w-0 whitespace-nowrap">
                <DetailLink
                  type="station"
                  target={{ lineId: ref.line_id, position: ref.station_index }}
                  displayText={line}
                  fullText={labels.station(ref, true)}
                  pillLabel={line}
                  stationIdShown={false}
                  homeLineId={null}
                  onSelect={() => {
                    setSource('detail-panel');
                    setSelection({ type: 'station', stationRef: { line_id: ref.line_id, station_index: ref.station_index } });
                  }}
                  isMissing={findStation(stations, ref.line_id, ref.station_index) === undefined}
                  onAnnounce={announceSelection}
                  onHoverStart={() => setHoveredStation({ line_id: ref.line_id, station_index: ref.station_index })}
                  onHoverEnd={() => setHoveredStation(null)}
                />
              </span>
              <button
                type="button"
                onClick={() => {
                  void detach(ref);
                }}
                className="p-1 rounded text-text-secondary hover:text-error hover:bg-error-10 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset disabled:opacity-50 disabled:cursor-not-allowed"
                title={t('detailPanel.station.detachLine', { line })}
                aria-label={t('detailPanel.station.detachLine', { line })}
                data-testid={`detail-unmerge-${ref.line_id}-${ref.station_index}`}
                disabled={editingBlocked || detachingKey === key}
              >
                <LinkOffIcon size={14} />
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
