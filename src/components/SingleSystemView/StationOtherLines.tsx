import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useEditingBlocked } from '../../hooks/useEditingBlocked';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { displayStationName } from '../../utils/formatters';
import { findStation } from '../../utils/findStation';
import { setSource, trackContextMenuAction } from '../../utils/telemetry';
import type { Station, StationRef } from '../../types/project';
import type { ArchivedStation } from '../../types/ipc/ArchivedStation';
import { DetailLink } from '../common/DetailLink';
import { LinkOffIcon } from '../common/icons';

/**
 * The record of a node that names a line (RS 3: at most one does): that
 * line's own station, which a merge archived and Detach gives back.
 * RS-UI: the "Also on" rows and the records' list are redesigned (RS 6).
 */
export function lineRecord(records: ReadonlyArray<ArchivedStation> | undefined, lineId: number): ArchivedStation | undefined {
  return records?.find((record) => record.lines.includes(lineId));
}

/** The ids of the records the "Also on" rows show: each other line's own. */
export function recordsShownByOtherLines(station: Station): Set<number> {
  const shown = new Set<number>();
  for (const ref of station.merged_with) {
    const record = lineRecord(station.archived, ref.line_id);
    if (record) shown.add(record.id);
  }
  return shown;
}

/**
 * The Station details' other lines through a junction, "Also on" (demo
 * findings N-3 and I-9). One row per other line: the line (a link to its
 * slot), the name that line shows the station by when it is not the one the
 * Name row says, and, when that line knew it by another Station ID before a
 * merge (the alias naming it, which Detach gives back), that ID: `North
 * Passage 0 (Station ID 9)`. A line that shows the station as this one does
 * is just named. Each row has the button that detaches that line.
 *
 * Before, a junction said one fact three times: a Junction row (`0 on North
 * Passage`), "Merged with: North Passage" and "Also known as: 0 on North
 * Passage (Station ID 9)". Now the names are said here once, and "Also known
 * as" lists only the aliases no row here shows. The heading is the same for
 * every junction however it was made (a merge, Split Line Here, a line
 * started from the station, an import): "Merged with" read wrong on a split.
 *
 * A junction is ONE station several lines pass (plan 5.2), and any two of its
 * slots are a pair: the pair form detaches the PARTNER's line, which is the
 * row the user clicked.
 */
export function StationOtherLines({ station }: { station: Station }) {
  const { t } = useTranslation();
  const { stations, unmergeStationPair } = useProjectStore(
    useShallow((s) => ({ stations: s.stations, unmergeStationPair: s.unmergeStationPair })),
  );
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
  const stationId = labels.stationId(station.station_ref);
  const ownName = labels.stationName(station.station_ref);

  const detach = async (ref: StationRef) => {
    trackContextMenuAction('unmerge', 'detail-panel');
    setDetachingKey(`${ref.line_id}:${ref.station_index}`);
    try {
      await unmergeStationPair(station.station_ref, ref);
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
          // Only what differs from the Name and Station ID rows (UX-7: the
          // station's own name again read as "merged with itself").
          const name = labels.stationName(ref);
          const otherName = name === ownName ? null : displayStationName(name);
          const record = lineRecord(station.archived, ref.line_id);
          const otherId = record && record.id !== stationId ? record.id : null;
          const said =
            otherName !== null && otherId !== null
              ? t('detailPanel.station.otherLineName', { name: otherName, id: otherId })
              : otherName ?? (otherId !== null ? t('detailPanel.station.otherLineId', { id: otherId }) : null);
          return (
            <li key={key} className="text-sm flex items-center justify-between gap-2">
              {/* The pill never wraps; the name and Station ID wrap under it
                  in a narrow panel rather than being cut off. */}
              <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5">
                <span className="shrink-0 whitespace-nowrap">
                  <DetailLink
                    type="station"
                    target={{ lineId: ref.line_id, position: ref.station_index }}
                    displayText={line}
                    fullText={labels.station(ref, true)}
                    pillLabel={line}
                    stationIdShown={false}
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
                {said !== null && (
                  <span className="min-w-0 break-words" data-testid={`station-detail-other-name-${ref.line_id}-${ref.station_index}`}>
                    {said}
                  </span>
                )}
              </div>
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
