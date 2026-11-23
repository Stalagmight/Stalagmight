/**
 * EntityBlob - a line pill that is not a link: the data table's row gutters
 * and line picker, and the station, shot and line names in dialogs. Same look
 * as the `DetailLink` pill (`LinePill.tsx`): the colour of its line, a dot,
 * an optional muted context, the main part and, for a station, its Station
 * ID as a muted superscript.
 *
 * Renders the pill only when the highlighting level is "full"; otherwise it
 * renders `plain` (or its children) as plain text, so callers can wrap a name
 * unconditionally.
 */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { useUIStore } from '../../store/uiStore';
import type { BlobEntity } from '../../theme/syntacticPalette';
import { syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import type { StationRef } from '../../types/project';
import { markOf, PillParts, pillStyle, usePillColors } from './LinePill';
import type { ShotEndMarks } from './shotEndMarks';

export interface EntityBlobProps {
  /** Which entity this names (`station`, `shot`, `line`). */
  entity: BlobEntity;
  /** The line the pill belongs to, whose colour it takes; null for a neutral pill. */
  lineId: number | null;
  /** The main part: a station name, a shot, a line name. */
  children: ReactNode;
  /** The muted, smaller context before the main part (a line name), shown with a colon. */
  context?: string | null | undefined;
  /** A station's Station ID, as a muted superscript at the end. */
  stationId?: number | null | undefined;
  /** A station pill's home-line mark: the line the station belongs to (see `LinePill.tsx`). */
  homeLineId?: number | null | undefined;
  /** A station pill's loop mark: a loop closure shows this station as its other end (RS 6). */
  loopEnd?: boolean | undefined;
  /** A shot pill's two ends, each with an optional home-line mark and loop mark; replaces `children` in the pill. */
  shotEnds?: ShotEndMarks | null | undefined;
  /** What the lower highlighting levels show; defaults to `children`. */
  plain?: ReactNode | undefined;
  /** After everything, inside the pill (the data table's picker chevron). */
  trailing?: ReactNode | undefined;
  /** Optional tooltip. */
  title?: string | undefined;
  /** Extra classes for the pill. */
  className?: string | undefined;
  testId?: string | undefined;
}

export function EntityBlob({
  entity,
  lineId,
  children,
  context,
  stationId,
  homeLineId,
  loopEnd = false,
  shotEnds,
  plain,
  trailing,
  title,
  className = '',
  testId,
}: EntityBlobProps) {
  const { t } = useTranslation();
  const level = useUIStore((s) => s.syntacticLevel);
  const pill = usePillColors(lineId);

  // Only the Full level paints pills; otherwise keep the plain text rendering.
  if (!syntacticLevelAtLeast(level, 'full')) {
    return (
      <>
        {plain ?? children}
        {trailing}
      </>
    );
  }

  return (
    <span
      className={`sh-lpill ${className}`}
      style={pillStyle(pill.colors)}
      data-entity={entity}
      data-testid={testId}
      title={title}
    >
      <PillParts
        testId={testId}
        context={context}
        main={children}
        shotEnds={
          shotEnds
            ? {
                from: shotEnds.from,
                to: shotEnds.to,
                fromMark: markOf(pill, shotEnds.fromHomeLineId),
                toMark: markOf(pill, shotEnds.toHomeLineId),
                fromLoopEnd: shotEnds.fromLoopEnd,
                toLoopEnd: shotEnds.toLoopEnd,
              }
            : null
        }
        mark={entity === 'station' ? markOf(pill, homeLineId) : null}
        loopEnd={entity === 'station' && loopEnd}
        loopEndLabel={t('identity.loopEnd')}
        stationId={entity === 'station' ? stationId : null}
        stationIdLabel={t('identity.stationId')}
        trailing={trailing}
      />
    </span>
  );
}

/**
 * A station as a pill: its bare name, the Station ID it shows, its line's
 * colour and, with `withLine`, its line as the context (`Main Line: 17`). A
 * foreign slot carries the dot of its shown station's home line, and a loop
 * end the loop mark (RS 6), unless `homeLineId` says otherwise. Below Full it
 * is the plain label (`17`, `Main Line / 17`; `12 (Main)`, `Side / 12 (Main)`
 * for a foreign slot; `3 (loop end on Side)` for a loop end).
 */
export function StationBlob({
  station,
  withLine = false,
  homeLineId,
  className,
  testId,
  title,
}: {
  station: StationRef;
  withLine?: boolean | undefined;
  homeLineId?: number | null | undefined;
  className?: string | undefined;
  testId?: string | undefined;
  title?: string | undefined;
}) {
  const labels = useIdentityLabels();
  return (
    <EntityBlob
      entity="station"
      lineId={station.line_id}
      context={withLine ? labels.lineName(station.line_id) : null}
      stationId={labels.shownStationId(station)}
      homeLineId={homeLineId === undefined ? labels.homeMark(station) : homeLineId}
      loopEnd={labels.shown(station)?.loop_end === true}
      plain={labels.station(station, withLine)}
      className={className}
      testId={testId}
      title={title}
    >
      {labels.stationPill(station)}
    </EntityBlob>
  );
}

export default EntityBlob;
