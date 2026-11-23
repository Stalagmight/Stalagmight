import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { useProse } from '../../hooks/useProse';
import { setSource } from '../../utils/telemetry';
import type { StationRef } from '../../types/project';
import { DetailLink } from '../common/DetailLink';
import { Prose } from '../common/Prose';

/**
 * The Station details' "Connected" rows for the OTHER lines through a station.
 *
 * On the station graph a junction is ONE station that two or more lines pass
 * (plan 9.3), so it has three or more incident shots, not just a previous and a
 * next one. The panel lists this slot's line's two as before; these are the
 * shots that meet the station on each other line (`mergedWith`, the station's
 * other slots), one row per shot, named by the line it is on ("On [Side]:",
 * the line a prose pill, RS-PROSE).
 */
export function StationOtherLineShots({ mergedWith }: { mergedWith: ReadonlyArray<StationRef> }) {
  const { t } = useTranslation();
  const lines = useProjectStore((s) => s.project?.lines);
  const labels = useIdentityLabels();
  const prose = useProse();
  const { setSelection, announceSelection, setHoveredShot } = useUIStore(
    useShallow((s) => ({
      setSelection: s.setSelection,
      announceSelection: s.announceSelection,
      setHoveredShot: s.setHoveredShot,
    })),
  );

  const shots = mergedWith.flatMap((ref) => {
    const line = lines?.find((l) => l.id === ref.line_id);
    if (!line) return [];
    // Station i sits between shot i-1 (arriving) and shot i (leaving).
    return [ref.station_index - 1, ref.station_index]
      .filter((k) => k >= 0 && k < line.shots.length)
      .map((k) => ({ lineId: line.id, shotIndex: k, shotId: line.shots[k]!.id }));
  });

  return (
    <>
      {shots.map((s) => (
        <div
          key={`${s.lineId}:${s.shotIndex}`}
          className="flex justify-between gap-2"
          data-testid={`station-detail-other-shot-${s.lineId}-${s.shotIndex}`}
        >
          <span className="text-text-secondary truncate" data-testid={`station-detail-other-shot-${s.lineId}-${s.shotIndex}-label`}>
            <Prose text={t('detailPanel.station.otherLineShots', { line: prose.line(s.lineId) })} />:
          </span>
          <DetailLink
            type="shot"
            target={{ lineId: s.lineId, position: s.shotIndex + 1 }}
            displayText={labels.shotAt(s.lineId, s.shotIndex)}
            fullText={labels.shotAt(s.lineId, s.shotIndex, true)}
            onSelect={() => {
              setSource('detail-panel');
              setSelection({ type: 'shot', lineId: s.lineId, shotId: s.shotId });
            }}
            onAnnounce={announceSelection}
            onHoverStart={() => setHoveredShot({ lineId: s.lineId, shotId: s.shotId })}
            onHoverEnd={() => setHoveredShot(null)}
          />
        </div>
      ))}
    </>
  );
}
