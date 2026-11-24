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
import { markOf, PillParts, pillStyle, shotToneOf, useResolvePill } from './LinePill';
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
  /** The line a station pill's station belongs to, when not `lineId`: the
   *  pill takes its colour and names it as the context (RS-OWNERSHIP, see
   *  `LinePill.tsx`). */
  homeLineId?: number | null | undefined;
  /** A station pill's loop mark: a loop closure shows this station as its other end (RS 6). */
  loopEnd?: boolean | undefined;
  /** A shot pill's two ends, each with an optional home-line mark, loop mark
   *  and owner line (its half's colour); replaces `children` in the pill. */
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
  const resolve = useResolvePill();
  // A station of another line takes that line's colour and name (RS-OWNERSHIP).
  const owner = entity === 'station' && homeLineId !== null && homeLineId !== undefined && homeLineId !== lineId ? homeLineId : null;
  const pill = resolve(owner ?? lineId);
  const ownerName = owner !== null ? (pill.lineNameOf?.(owner) ?? null) : null;

  // Only the Full level paints pills; otherwise keep the plain text rendering.
  if (!syntacticLevelAtLeast(level, 'full')) {
    return (
      <>
        {plain ?? children}
        {trailing}
      </>
    );
  }

  const tone =
    entity === 'shot' && shotEnds
      ? shotToneOf(resolve, lineId, shotEnds.fromOwnerLineId, shotEnds.toOwnerLineId, (p) => pillStyle(p.colors))
      : null;
  // A foreign end's dot sits in its own half, so it is checked against that half's colours.
  const endPill = (end: number | null | undefined) =>
    tone && end !== null && end !== undefined && end !== lineId ? resolve(end) : pill;
  return (
    <span
      className={`sh-lpill${tone ? ' sh-lpill--split' : ''} ${className}`}
      style={tone ? undefined : pillStyle(pill.colors)}
      data-entity={entity}
      data-owner-line={owner ?? undefined}
      data-testid={testId}
      title={title ?? (ownerName !== null ? t('identity.homeMarkTitle', { line: ownerName }) : undefined)}
    >
      <PillParts
        testId={testId}
        context={owner !== null && context ? (ownerName ?? context) : context}
        main={children}
        shotEnds={
          shotEnds
            ? {
                from: shotEnds.from,
                to: shotEnds.to,
                fromMark: markOf(endPill(shotEnds.fromOwnerLineId), shotEnds.fromHomeLineId),
                toMark: markOf(endPill(shotEnds.toOwnerLineId), shotEnds.toHomeLineId),
                fromLoopEnd: shotEnds.fromLoopEnd,
                toLoopEnd: shotEnds.toLoopEnd,
              }
            : null
        }
        tone={tone}
        loopEnd={entity === 'station' && loopEnd}
        loopEndLabel={t('identity.loopEnd')}
        stationId={entity === 'station' ? stationId : null}
        stationIdLabel={t('identity.stationId')}
        ownerWords={
          ownerName === null
            ? null
            : context
              ? t('identity.onLineLabel', { line: pill.lineNameOf?.(lineId) ?? '' })
              : t('identity.homeMarkLabel', { line: ownerName })
        }
        trailing={trailing}
      />
    </span>
  );
}

/**
 * A station as a pill: its bare name, the Station ID it shows, its line's
 * colour and, with `withLine`, its line as the context (`Main Line: 17`). A
 * foreign slot, or a loop end whose kept end lives on another line, takes
 * the colour and name of the line its shown station belongs to
 * (RS-OWNERSHIP), unless `homeLineId` says otherwise; a loop end keeps the
 * loop mark (RS 6). Below Full it
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
      homeLineId={homeLineId === undefined ? labels.ownerLine(station) : homeLineId}
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
