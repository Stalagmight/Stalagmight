// The Closures block of the station detail view (#576 D2 split it out of
// DetailPanel.tsx at this boundary): the loop closures a station takes part in,
// grouped by closure, with links to the other end and a Remove action.
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useEditingBlocked } from '../../hooks/useEditingBlocked';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { ChevronDownIcon, ChevronRightIcon, LinkOffIcon } from '../common/icons';
import { DetailLink } from '../common/DetailLink';
import type { Station, ClosureRef, LoopClosure } from '../../types/project';
import { formatStationId } from '../../utils/formatters';
import { formatFixed, formatPercent } from '../../i18n/format';
import { trackContextMenuAction } from '../../utils/telemetry';

interface ClosuresSectionProps {
  closures: ClosureRef[];
  loopClosures: LoopClosure[];
  stations: Station[];
  onSelectStation: (ref: { section_id: number; station_index: number }) => void;
  onAnnounce: (message: string) => void;
  onHoverStart: (ref: { section_id: number; station_index: number }) => void;
  onHoverEnd: () => void;
}

export function ClosuresSection({
  closures,
  loopClosures,
  stations,
  onSelectStation,
  onAnnounce,
  onHoverStart,
  onHoverEnd,
}: ClosuresSectionProps) {
  const { t } = useTranslation();
  const removeLoopClosure = useProjectStore((s) => s.removeLoopClosure);
  const editingBlocked = useEditingBlocked();
  const precision = useDecimalPrecision();
  const [expanded, setExpanded] = useState(true);
  const [removingClosureId, setRemovingClosureId] = useState<number | null>(null);
  const contentId = 'closures-section-content';

  // Precompute station existence set for O(1) lookups
  const stationKeySet = useMemo(() => {
    const keys = new Set<string>();
    for (const s of stations) {
      keys.add(`${s.station_ref.section_id}:${s.station_ref.station_index}`);
    }
    return keys;
  }, [stations]);

  const stationExists = (ref: { section_id: number; station_index: number }): boolean => {
    return stationKeySet.has(`${ref.section_id}:${ref.station_index}`);
  };

  // Precompute loop closures map for O(1) lookups
  const closureMap = useMemo(() => {
    const map = new Map<number, LoopClosure>();
    for (const lc of loopClosures) {
      map.set(lc.id, lc);
    }
    return map;
  }, [loopClosures]);

  // Group closures by closure_id and sort deterministically
  const sortedGroups = useMemo(() => {
    const grouped = new Map<number, ClosureRef[]>();
    for (const c of closures) {
      const existing = grouped.get(c.closure_id);
      if (existing) {
        existing.push(c);
      } else {
        grouped.set(c.closure_id, [c]);
      }
    }

    // Sort: direct closures first, then transitive; tiebreak by closure_id ascending
    return Array.from(grouped.entries()).sort(([idA, refsA], [idB, refsB]) => {
      const aHasDirect = refsA.some((r) => !r.is_transitive);
      const bHasDirect = refsB.some((r) => !r.is_transitive);
      if (aHasDirect && !bHasDirect) return -1;
      if (!aHasDirect && bHasDirect) return 1;
      if (idA < idB) return -1;
      if (idA > idB) return 1;
      return 0;
    });
  }, [closures]);

  const handleRemoveClosure = async (closureId: number) => {
    trackContextMenuAction('remove-closure', 'detail-panel');
    setRemovingClosureId(closureId);
    try {
      await removeLoopClosure(closureId);
    } finally {
      setRemovingClosureId(null);
    }
  };

  return (
    <div className="pt-2 border-t border-border" data-testid="closures-section">
      <button
        type="button"
        onClick={() => setExpanded(!expanded)}
        className="flex items-center gap-1 text-sm text-text-secondary hover:text-text-primary w-full text-left"
        data-testid="closures-section-toggle"
        aria-expanded={expanded}
        aria-controls={contentId}
      >
        <span className="text-xs">
          {expanded ? <ChevronDownIcon size={12} /> : <ChevronRightIcon size={12} />}
        </span>
        <span>{t('detailPanel.closures.title')}</span>
      </button>
      {expanded && (
        <div className="mt-1 space-y-2" id={contentId} data-testid="closures-section-content">
          {sortedGroups.map(([closureId, refs]) => {
            const closureIdLabel = closureId.toString();
            const closure = closureMap.get(closureId);
            const directRefs = refs.filter((r) => !r.is_transitive);
            const transitiveRefs = refs.filter((r) => r.is_transitive);

            const isTransitiveOnly = directRefs.length === 0 && transitiveRefs.length > 0;

            return (
              <div key={closureIdLabel} className={`space-y-1${isTransitiveOnly ? ' opacity-50' : ''}`}>
                <div className="flex items-start justify-between gap-2">
                  <div className="space-y-1 min-w-0">
                    {directRefs.length > 0 && (
                      <div className="text-sm" data-testid={`closure-direct-${closureIdLabel}`}>
                        <span className="text-text-secondary">{t('detailPanel.closures.direct')} </span>
                        {directRefs.map((ref, i) => (
                          <span key={i}>
                            {i > 0 && ', '}
                            <DetailLink
                              type="station"
                              target={{ sectionId: ref.station_ref.section_id, position: ref.station_ref.station_index }}
                              displayText={formatStationId(ref.station_ref.section_id, ref.station_ref.station_index)}
                              onSelect={() => onSelectStation(ref.station_ref)}
                              isMissing={!stationExists(ref.station_ref)}
                              onAnnounce={onAnnounce}
                              onHoverStart={() => onHoverStart(ref.station_ref)}
                              onHoverEnd={onHoverEnd}
                            />
                          </span>
                        ))}
                      </div>
                    )}
                    {transitiveRefs.length > 0 && (
                      <div className="text-sm" data-testid={`closure-transitive-${closureIdLabel}`}>
                        <span className="text-text-secondary">{t('detailPanel.closures.transitive')} </span>
                        {transitiveRefs.map((ref, i) => (
                          <span key={i}>
                            {i > 0 && ', '}
                            <DetailLink
                              type="station"
                              target={{ sectionId: ref.station_ref.section_id, position: ref.station_ref.station_index }}
                              displayText={formatStationId(ref.station_ref.section_id, ref.station_ref.station_index)}
                              onSelect={() => onSelectStation(ref.station_ref)}
                              isMissing={!stationExists(ref.station_ref)}
                              onAnnounce={onAnnounce}
                              onHoverStart={() => onHoverStart(ref.station_ref)}
                              onHoverEnd={onHoverEnd}
                            />
                            {ref.via_station && (
                              <span>
                                {t('detailPanel.closures.viaPrefix')}
                                <DetailLink
                                  type="station"
                                  target={{ sectionId: ref.via_station.section_id, position: ref.via_station.station_index }}
                                  displayText={formatStationId(ref.via_station.section_id, ref.via_station.station_index)}
                                  onSelect={() => onSelectStation(ref.via_station!)}
                                  isMissing={!stationExists(ref.via_station)}
                                  onAnnounce={onAnnounce}
                                  onHoverStart={() => onHoverStart(ref.via_station!)}
                                  onHoverEnd={onHoverEnd}
                                />
                                {t('detailPanel.closures.viaSuffix')}
                              </span>
                            )}
                          </span>
                        ))}
                      </div>
                    )}
                  </div>
                  <button
                    type="button"
                    onClick={() => {
                      void handleRemoveClosure(closureId);
                    }}
                    className="p-1 rounded text-text-secondary hover:text-error hover:bg-error-10 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset disabled:opacity-50 disabled:cursor-not-allowed"
                    title="Remove Closure"
                    aria-label={`Remove closure ${closureIdLabel}`}
                    data-testid={`detail-remove-closure-${closureIdLabel}`}
                    disabled={editingBlocked || !closure || removingClosureId === closureId}
                  >
                    <LinkOffIcon size={14} />
                  </button>
                </div>
                {closure ? (
                  <div className="text-xs text-text-secondary" data-testid={`closure-metrics-${closureIdLabel}`}>
                    {t('detailPanel.closures.metrics', {
                      path: `${formatFixed(closure.path_length, undefined, precision.length)}${t('common.units.meters')}`,
                      error: `${formatFixed(closure.misclosure_distance, undefined, precision.length)}${t('common.units.meters')}`,
                      percent: formatPercent(closure.error_percentage, undefined, 2),
                    })}
                  </div>
                ) : (
                  <div className="text-xs text-text-secondary" data-testid={`closure-metrics-${closureId}`}>
                    {t('detailPanel.closures.unavailable')}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
